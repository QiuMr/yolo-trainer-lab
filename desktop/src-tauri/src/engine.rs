//! The engine sidecar: locating `montgomery.exe`, running it, and fanning its output out.
//!
//! Deliberately free of any Tauri type. The desktop shell is one consumer of this module, not
//! part of it, so the local control service planned for AI-driven use (see DECISIONS D011) can
//! start runs and subscribe to the same event stream without duplicating a line of the spawn
//! logic. The only thing it has to add is its own transport.
//!
//! Output contract: stdout is NDJSON (docs/API.md), stderr is human narration. Both are forwarded
//! verbatim; the shell only ever *adds* the `exit` event, because a process cannot report its own
//! exit code.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// On Windows a child process without this flag pops a console window behind the app.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub(crate) fn hidden_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// How a run starts.
///
/// `montgomery train` accepts **exactly one** of `--architecture` / `--model` / `--resume` and clap
/// rejects any combination, so this is modelled as a choice. Three optional strings that can
/// disagree would push that rule onto every caller and leave the failure to clap's error text.
///
/// The distinction between the last two is the one people get wrong, and it is not cosmetic:
/// upstream's README says *"A Burnpack initializes a new run; it is not a resumable optimizer
/// checkpoint."* Finetune silently restarts the optimizer, EMA and epoch counter; only Resume
/// continues them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum TrainInit {
    /// Random weights from a named architecture.
    Scratch { architecture: String },
    /// Start from a finished `.bpk`. A new run: optimizer/EMA/epoch restart. Use when the goal is
    /// "keep what it learned, teach it my data".
    Finetune { model: String },
    /// Continue an interrupted run exactly. The checkpoint owns the dataset path, batch size and
    /// epoch budget — those must not be passed, and are ignored if they are.
    Resume { checkpoint: String },
}

/// What the shell may ask the engine to do. Mirrors the subset of `montgomery train` the UI drives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainRequest {
    pub init: TrainInit,
    /// Dataset manifest for `scratch` and `finetune`. Empty and unused for `resume`.
    #[serde(default)]
    pub dataset: String,
    pub epochs: u32,
    pub batch: u32,
    pub imgsz: Option<u32>,
    /// `auto` | `cpu` | `gpu`. Anything else is coerced to `auto` before it reaches clap.
    pub device: String,
}

/// The parts of a training checkpoint that live in its `manifest.json`.
///
/// Read on the shell side only because of an upstream quirk: `train_inner` keeps the epoch budget
/// from the checkpoint but emits the `start` event with `request.epochs`, which is clap's default
/// of 100 when the caller passed nothing. A 300-epoch run resumed without `--epochs` would therefore
/// report "0/100" while actually training to 300. Passing the checkpoint's own numbers makes the
/// event honest again, and they are otherwise ignored by the resume path.
struct ResumeInfo {
    dir: PathBuf,
    epochs: u32,
    batch: u32,
    dataset: String,
}

impl TrainRequest {
    fn device(&self, fallback: &str) -> String {
        super::settings::normalize_device(&self.device, fallback)
    }
}

/// Accept either a dataset directory or a manifest file, and insist the result exists.
///
/// The engine would ultimately fail on a bad path too, but twenty seconds later and with its own
/// wording; this is the same check the UI already made, repeated where an HTTP caller can reach it.
fn resolve_manifest(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("缺少数据集路径。".into());
    }
    let path = PathBuf::from(trimmed);
    if !path.exists() {
        return Err(format!("数据集路径不存在：{trimmed}"));
    }
    if path.is_dir() {
        let manifest = path.join("dataset.yaml");
        if !manifest.is_file() {
            return Err(format!("目录里没有 dataset.yaml：{trimmed}"));
        }
        return Ok(manifest);
    }
    Ok(path)
}

/// Read `<checkpoint>/manifest.json` and verify the files the engine will actually need.
fn read_checkpoint(raw: &str) -> Result<ResumeInfo, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("缺少 checkpoint 目录。".into());
    }
    let dir = PathBuf::from(trimmed);
    if !dir.is_dir() {
        return Err(format!(
            "checkpoint 目录不存在：{trimmed}（应指向 runs/<name>/checkpoints/last）"
        ));
    }
    for required in ["manifest.json", "model.bin", "optimizer.bin"] {
        if !dir.join(required).is_file() {
            return Err(format!(
                "checkpoint 不完整，缺少 {required}：{trimmed}。\
                 应指向 runs/<name>/checkpoints/last，不是 exports/ 里的 .bpk。"
            ));
        }
    }
    let bytes = std::fs::read(dir.join("manifest.json"))
        .map_err(|error| format!("读取 checkpoint 清单失败：{error}"))?;
    let manifest: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("checkpoint 清单不是合法 JSON：{error}"))?;
    let config = manifest
        .get("config")
        .ok_or("checkpoint 清单缺少 config 段")?;

    let epochs = config
        .get("epochs")
        .and_then(Value::as_u64)
        .unwrap_or(100) as u32;
    let batch = config
        .get("batch_size")
        .and_then(Value::as_u64)
        .unwrap_or(8) as u32;
    let dataset = config
        .get("data")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();

    // The checkpoint stores the dataset path verbatim, and the engine resolves it against the
    // child's working directory — which for a packaged app is the install folder. A relative path
    // recorded by a CLI run in the repository root therefore cannot resolve here. Saying so now
    // beats letting the engine die with "No such file" halfway through startup.
    if dataset.is_empty() {
        return Err("checkpoint 清单里没有数据集路径（config.data）。".into());
    }
    let data_path = PathBuf::from(&dataset);
    if !data_path.exists() {
        return Err(format!(
            "checkpoint 记录的数据集已不存在：{dataset}。\
             如果是相对路径，它按引擎的工作目录解析，续训要求该路径仍可用；\
             否则请改用「用已有模型微调」。"
        ));
    }

    Ok(ResumeInfo {
        dir,
        epochs,
        batch,
        dataset,
    })
}

/// Candidate locations for `montgomery.exe`, most specific first.
///
/// A bundled build ships it next to the app; a development build finds it in the build tree. Both
/// are probed rather than making the user configure a path before they can do anything.
pub fn resolve(configured: Option<&str>) -> Result<PathBuf, String> {
    if let Some(explicit) = configured.filter(|value| !value.trim().is_empty()) {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Ok(path);
        }
        // Do not silently fall back: the user set this path on purpose and needs to know it is
        // wrong, otherwise the app looks like it cannot find an engine that is clearly installed.
        return Err(format!("设置的引擎路径不存在：{}", path.display()));
    }

    let mut candidates = Vec::new();
    if let Ok(explicit) = std::env::var("MONTGOMERY_ENGINE") {
        candidates.push(PathBuf::from(explicit));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("montgomery.exe"));
            candidates.push(dir.join("montgomery"));
        }
    }
    // Development layout: <repo>/desktop/src-tauri -> <repo>/artifacts/target-dist/release
    if let Ok(cwd) = std::env::current_dir() {
        let root = cwd.parent().and_then(Path::parent).unwrap_or(&cwd);
        candidates.push(
            root.join("artifacts")
                .join("target-dist")
                .join("release")
                .join("montgomery.exe"),
        );
        candidates.push(
            root.join("artifacts")
                .join("target")
                .join("release")
                .join("montgomery.exe"),
        );
    }

    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err("找不到 montgomery 引擎。请在设置里指定路径，或设置 MONTGOMERY_ENGINE 环境变量。".into())
}

/// One thing the engine said.
#[derive(Clone, Debug)]
pub enum Message {
    /// A JSON line from stdout, forwarded verbatim.
    Event(Value),
    /// A human line: stderr, or stdout that was not JSON.
    Log(String),
}

/// Fan-out of engine output: one publisher (the reader threads), N subscribers added at runtime.
///
/// A `std::sync::mpsc` per subscriber rather than a broadcast channel because there is no async
/// runtime in this process and no need for backpressure semantics.
#[derive(Default)]
pub struct Hub {
    subscribers: Mutex<Vec<mpsc::Sender<Message>>>,
}

impl Hub {
    pub fn subscribe(&self) -> mpsc::Receiver<Message> {
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut list) = self.subscribers.lock() {
            list.push(sender);
        }
        receiver
    }

    fn publish(&self, message: Message) {
        let Ok(mut list) = self.subscribers.lock() else {
            return;
        };
        // Prune receivers that are gone (window reloaded, service stopped), otherwise every run
        // would keep publishing into a dead channel.
        list.retain(|sender| sender.send(message.clone()).is_ok());
    }
}

struct Inner {
    child: Mutex<Option<Child>>,
    hub: Hub,
}

/// Handle to the engine process. Cheap to clone; all clones share one child and one hub.
#[derive(Clone)]
pub struct Engine(Arc<Inner>);

impl Default for Engine {
    fn default() -> Self {
        Self(Arc::new(Inner {
            child: Mutex::new(None),
            hub: Hub::default(),
        }))
    }
}

impl Engine {
    pub fn subscribe(&self) -> mpsc::Receiver<Message> {
        self.0.hub.subscribe()
    }

    /// A finished-but-unreaped child counts as not running: the reaper collects it on its next tick.
    pub fn is_running(&self) -> bool {
        let Ok(mut guard) = self.0.child.lock() else {
            return false;
        };
        match guard.as_mut() {
            Some(child) => child.try_wait().map(|status| status.is_none()).unwrap_or(false),
            None => false,
        }
    }

    pub fn start(
        &self,
        exe: &Path,
        output_dir: &Path,
        request: &TrainRequest,
        default_device: &str,
    ) -> Result<(), String> {
        // One lock across check → spawn → store. The window command and `POST /train` can arrive
        // together; a standalone `is_running()` followed by a separate store let both spawn, and
        // the overwritten `Child` handle became an orphan that `stop()` could never see.
        let mut guard = self.0.child.lock().map_err(|_| "状态锁损坏".to_string())?;
        if let Some(child) = guard.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => *guard = None, // already finished and reaped
                Ok(None) => return Err("已有训练在运行。".into()),
            }
        }

        let mut command = hidden_command(exe);
        command.arg("train");
        // Everything mode-specific is decided here, before a process exists, so a bad path is a
        // readable message rather than an engine that starts and dies on its own.
        match &request.init {
            TrainInit::Scratch { architecture } => {
                let manifest = resolve_manifest(&request.dataset)?;
                command
                    .arg("--architecture")
                    .arg(architecture)
                    .arg("--data")
                    .arg(&manifest)
                    .arg("--epochs")
                    .arg(request.epochs.to_string())
                    .arg("--batch")
                    .arg(request.batch.to_string());
            }
            TrainInit::Finetune { model } => {
                let manifest = resolve_manifest(&request.dataset)?;
                let model_path = PathBuf::from(model.trim());
                if !model_path.is_file() {
                    return Err(format!("模型文件不存在：{}", model_path.display()));
                }
                command
                    .arg("--model")
                    .arg(&model_path)
                    .arg("--data")
                    .arg(&manifest)
                    .arg("--epochs")
                    .arg(request.epochs.to_string())
                    .arg("--batch")
                    .arg(request.batch.to_string());
            }
            TrainInit::Resume { checkpoint } => {
                let info = read_checkpoint(checkpoint)?;
                // Only the checkpoint's own numbers: the dataset, batch and epoch budget all come
                // from it. Passing them makes the start event report the real epoch budget (see
                // `ResumeInfo`) and keeps the engine from re-deriving anything.
                command
                    .arg("--resume")
                    .arg(&info.dir)
                    .arg("--epochs")
                    .arg(info.epochs.to_string())
                    .arg("--batch")
                    .arg(info.batch.to_string());
                // Surface what is being continued. The UI cannot read the checkpoint itself, and an
                // assistant watching the log should see which dataset is about to be walked and how
                // many epochs it is signing up for.
                self.0.hub.publish(Message::Log(format!(
                    "续训：从 {} 恢复 · 数据集 {} · 计划 {} 轮",
                    info.dir.display(),
                    info.dataset,
                    info.epochs
                )));
            }
        }
        command
            .arg("--device")
            .arg(request.device(default_device))
            // Without this the engine writes `runs/` into the current directory, which in a
            // packaged build is the install folder.
            .arg("--project")
            .arg(output_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        // Resume restores the input size from the checkpoint's model spec, so a value here would be
        // ignored; leaving it out keeps the shell from implying it has an effect.
        if let Some(size) = request.imgsz {
            if !matches!(request.init, TrainInit::Resume { .. }) {
                command.arg("--imgsz").arg(size.to_string());
            }
        }

        let mut child = command
            .spawn()
            .map_err(|error| format!("引擎启动失败：{error}"))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        // Still holding `guard`: no second caller can slip a spawn in between.
        *guard = Some(child);
        drop(guard);

        if let Some(stdout) = stdout {
            let engine = self.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Value>(trimmed) {
                        Ok(value) => engine.0.hub.publish(Message::Event(value)),
                        // Not an event: keep it visible rather than silently dropping it. A
                        // panic line arrives this way and is the only clue when a run dies.
                        Err(_) => engine.0.hub.publish(Message::Log(trimmed.to_owned())),
                    }
                }
            });
        }

        if let Some(stderr) = stderr {
            let engine = self.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if !line.trim().is_empty() {
                        engine.0.hub.publish(Message::Log(line));
                    }
                }
            });
        }

        // Reap and announce the exit code: neither a closed pipe nor a killed process tells the
        // UI *how* the run ended.
        let reaper = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let finished = {
                let Ok(mut guard) = reaper.0.child.lock() else {
                    return;
                };
                let Some(child) = guard.as_mut() else {
                    return;
                };
                match child.try_wait() {
                    Ok(Some(status)) => Some(status.code()),
                    Ok(None) => None,
                    Err(_) => Some(None),
                }
            };
            if let Some(code) = finished {
                if let Ok(mut guard) = reaper.0.child.lock() {
                    *guard = None;
                }
                reaper
                    .0
                    .hub
                    .publish(Message::Event(json!({ "type": "exit", "code": code })));
                return;
            }
        });

        Ok(())
    }

    /// Ask the run to stop. The child stays in the slot on purpose: the reaper collects it and
    /// publishes the `exit` event, so callers see exactly one termination path whether the run
    /// ended by itself or was killed.
    pub fn stop(&self) -> Result<(), String> {
        let mut guard = self.0.child.lock().map_err(|_| "状态锁损坏".to_string())?;
        match guard.as_mut() {
            Some(child) => child.kill().map_err(|error| error.to_string()),
            None => Ok(()),
        }
    }

    /// Unconditional cleanup for process exit: no event, nobody is listening any more.
    pub fn kill_now(&self) {
        let Ok(mut guard) = self.0.child.lock() else {
            return;
        };
        if let Some(child) = guard.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(device: &str) -> TrainRequest {
        TrainRequest {
            init: TrainInit::Scratch {
                architecture: "yolo26n".into(),
            },
            dataset: "data.yaml".into(),
            epochs: 1,
            batch: 1,
            imgsz: None,
            device: device.into(),
        }
    }

    /// A device string the engine does not know must never reach clap: the run would abort with a
    /// usage error instead of training.
    #[test]
    fn unknown_device_is_coerced() {
        assert_eq!(request("tpu").device("auto"), "auto");
        assert_eq!(request("").device("gpu"), "gpu");
        assert_eq!(request("cpu").device("auto"), "cpu");
    }

    /// A configured-but-missing path is an error, not a silent fallback: the user needs to know
    /// their setting is wrong.
    #[test]
    fn misconfigured_engine_path_is_reported() {
        let error = resolve(Some("C:/definitely/not/here/montgomery.exe")).unwrap_err();
        assert!(error.contains("不存在"), "unexpected message: {error}");
    }

    #[test]
    fn hub_drops_dead_subscribers() {
        let hub = Hub::default();
        let first = hub.subscribe();
        let second = hub.subscribe();
        drop(first);
        hub.publish(Message::Log("hello".into()));
        match second.recv().unwrap() {
            Message::Log(line) => assert_eq!(line, "hello"),
            other => panic!("unexpected message: {other:?}"),
        }
    }

    /// End to end through the new hub: spawn the real engine on the repository's tiny dataset and
    /// expect a structured event to come out the other side.
    ///
    /// Ignored by default because it spends about a minute of CPU training. Run it explicitly after
    /// touching the spawn path, the reader threads or the hub — it is the only test that proves
    /// `spawn -> stdout pipe -> JSON parse -> publish -> subscriber` still holds together.
    ///
    /// ```text
    /// cargo test -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "spawns a real CPU training run (~1 min)"]
    fn real_run_reaches_the_hub() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let exe = repo
            .join("artifacts")
            .join("target-dist")
            .join("release")
            .join("montgomery.exe");
        let dataset = repo.join("spikes").join("tiny-dataset");
        if !exe.is_file() || !dataset.is_dir() {
            return; // Nothing built in this checkout; the test has nothing to spawn.
        }

        let engine = Engine::default();
        let receiver = engine.subscribe();
        let output = std::env::temp_dir().join("yolo-engine-smoke");
        let request = TrainRequest {
            init: TrainInit::Scratch {
                architecture: "yolo26n".into(),
            },
            dataset: dataset.to_string_lossy().into_owned(),
            epochs: 1,
            batch: 2,
            imgsz: Some(320),
            device: "cpu".into(),
        };
        engine
            .start(&exe, &output, &request, "auto")
            .expect("engine should spawn");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        let mut first_event = None;
        while std::time::Instant::now() < deadline {
            match receiver.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(Message::Event(value)) => {
                    first_event = Some(value);
                    break;
                }
                // stderr chatter is expected before the first event; keep waiting.
                Ok(Message::Log(_)) | Err(_) => continue,
            }
        }

        engine.stop().expect("engine should accept a stop request");

        let event = first_event.expect("no structured event arrived within 180s");
        let kind = event.get("type").and_then(|value| value.as_str());
        assert!(
            matches!(kind, Some("start") | Some("epoch")),
            "unexpected first event: {event}"
        );
    }
}
