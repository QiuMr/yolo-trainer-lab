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

/// What the shell may ask the engine to do. Mirrors the subset of `montgomery train` the UI drives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainRequest {
    pub architecture: String,
    pub dataset: String,
    pub epochs: u32,
    pub batch: u32,
    pub imgsz: Option<u32>,
    /// `auto` | `cpu` | `gpu`. Anything else is coerced to `auto` before it reaches clap.
    pub device: String,
}

impl TrainRequest {
    fn device(&self, fallback: &str) -> String {
        let requested = self.device.trim();
        if requested.is_empty() {
            return fallback.to_owned();
        }
        if super::settings::DEVICES.contains(&requested) {
            requested.to_owned()
        } else {
            "auto".to_owned()
        }
    }
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

        let data_path = PathBuf::from(&request.dataset);
        let manifest = if data_path.is_dir() {
            data_path.join("dataset.yaml")
        } else {
            data_path
        };

        let mut command = hidden_command(exe);
        command
            .arg("train")
            .arg("--architecture")
            .arg(&request.architecture)
            .arg("--data")
            .arg(&manifest)
            .arg("--epochs")
            .arg(request.epochs.to_string())
            .arg("--batch")
            .arg(request.batch.to_string())
            .arg("--device")
            .arg(request.device(default_device))
            // Without this the engine writes `runs/` into the current directory, which in a
            // packaged build is the install folder.
            .arg("--project")
            .arg(output_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        if let Some(size) = request.imgsz {
            command.arg("--imgsz").arg(size.to_string());
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
            architecture: "yolo26n".into(),
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
            architecture: "yolo26n".into(),
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
