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

use std::io::{BufRead, BufReader, Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One rollover point. Engine stderr is a few hundred lines per run, so 8 MiB holds many runs;
/// the number is only here so a pathological run cannot fill the disk.
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

/// Append-only engine log.
///
/// Why this exists: stderr used to be forwarded to the in-memory hub and nowhere else, so the only
/// record of a GPU `device lost`, a panic, or an ONNX shape error lived in the window's log panel.
/// Close the window — or let the app restart — and the evidence was gone. A user reporting "it just
/// failed" could not be helped, because nothing had been written down.
///
/// Every line is flushed immediately. Buffering would be faster, but a crash is precisely the case
/// where the last few lines matter most, and those are the ones a buffer would lose.
pub(crate) struct LogFile {
    path: PathBuf,
    state: Mutex<(Option<std::fs::File>, u64)>,
}

impl LogFile {
    pub fn new(path: PathBuf) -> Self {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Self {
            path,
            state: Mutex::new((None, 0)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&self, line: &str) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.0.is_none() {
            let existing = std::fs::metadata(&self.path).map(|meta| meta.len()).unwrap_or(0);
            if existing > MAX_LOG_BYTES {
                // One generation is kept: a crash shortly after rollover still has the previous
                // file next to it.
                let rotated = self.path.with_extension("log.1");
                let _ = std::fs::rename(&self.path, &rotated);
                state.1 = 0;
            } else {
                state.1 = existing;
            }
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                Ok(file) => state.0 = Some(file),
                // A log that cannot be opened must not stop training. The run is the product; the
                // log is diagnostics.
                Err(_) => return,
            }
        }
        let Some(file) = state.0.as_mut() else {
            return;
        };
        let stamp = stamp_now();
        let _ = writeln!(file, "{stamp} {line}");
        let _ = file.flush();
        state.1 += line.len() as u64 + stamp.len() as u64 + 2;
        if state.1 > MAX_LOG_BYTES {
            // Roll on the next write rather than here, so the caller never sees a half-written file.
            state.0 = None;
        }
    }
}

/// `HH:MM:SS.mmm` in local time is not available without a time crate; UTC seconds since the
/// Unix epoch plus a monotonic-ish counter is enough to order lines, and the report header carries
/// the human-readable date.
fn stamp_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or(0);
    format!("{millis}")
}

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

/// What asking the engine "can this device actually compute?" produced.
///
/// The distinction the UI has to make is not "which GPU is installed" — Windows answers that — but
/// "will a training run reach the first epoch on it". Those differ: this project's own reference
/// machine has an adapter that Windows reports happily, that wgpu opens, and that dies on the first
/// buffer readback. Only executing a kernel and reading the answer back settles it, which is what
/// `__device-probe` does in a child process so a poisoned driver cannot take the app with it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeviceReport {
    /// What the caller asked for: `auto`, `gpu` or `cpu`.
    pub device: String,
    pub ok: bool,
    /// The adapter as the runtime names itself, e.g. `AMD Radeon(TM) Graphics (spirv_4098_5688)`.
    pub adapter: Option<String>,
    /// `vulkan` | `dx12` | `gl` | `cpu`, or `auto` when nothing pinned a backend — the runtime
    /// picks one and does not report which, and inventing an answer here would tell a user
    /// something false about their own machine.
    pub backend: Option<String>,
    /// Why it is not usable, in the engine's own words. Clamped, because a panic backtrace is
    /// thousands of characters and the UI has one line for it.
    pub reason: Option<String>,
    pub elapsed_ms: u64,
}

/// How long a probe may take. Opening a device is the step that hangs instead of failing on a bad
/// driver, and it hangs before writing anything, so there is nothing to poll on — only a deadline.
/// A working adapter answers in about a second; a wedged one never answers.
const PROBE_TIMEOUT: Duration = Duration::from_secs(40);

/// Ask the engine whether `device` can run one kernel and read the result back.
///
/// Runs in a child process on purpose (see `__device-probe` in the engine): the failure mode being
/// detected poisons the device and takes the process down with a panic, which nothing in-process can
/// survive. The parent only reads an exit status and a line of output.
pub fn probe(engine: &Path, device: &str) -> DeviceReport {
    // `auto` asks the GPU question, because that is the branch `auto` will take if the answer is
    // yes; the CPU fallback is the shell's own guarantee and needs no probe.
    let target = if device.eq_ignore_ascii_case("cpu") {
        "cpu"
    } else {
        "gpu"
    };
    let started = Instant::now();
    let mut report = probe_once(engine, target, true);
    // An engine older than the `--json` flag rejects the argument, and clap's rejection is
    // indistinguishable from a real failure without looking for it. Retry once without it so a
    // packaged engine from before this change still answers — the human line carries the adapter
    // too, just less conveniently.
    if report
        .reason
        .as_deref()
        .is_some_and(|reason| reason.contains("--json"))
    {
        report = probe_once(engine, target, false);
    }
    report.device = device.to_owned();
    report.elapsed_ms = started.elapsed().as_millis() as u64;
    report
}

fn probe_once(engine: &Path, target: &str, json: bool) -> DeviceReport {
    let mut command = hidden_command(engine);
    command.arg("__device-probe").arg("--device").arg(target);
    if json {
        command.arg("--json");
    }
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return DeviceReport {
                ok: false,
                reason: Some(format!("引擎无法启动：{error}")),
                ..DeviceReport::default()
            }
        }
    };
    // Drain both pipes on their own threads before waiting. An engine that panics writes a
    // backtrace, and a full pipe blocks the child — which would turn "crashed" into "timed out".
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);

    let deadline = Instant::now() + PROBE_TIMEOUT;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                timed_out = true;
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(_) => {
                let _ = child.kill();
                break None;
            }
        }
    };
    let out = stdout.and_then(|handle| handle.join().ok()).unwrap_or_default();
    let err = stderr.and_then(|handle| handle.join().ok()).unwrap_or_default();

    // The engine prints exactly one JSON line, so take the last parseable one: a stray warning on
    // stdout before it must not shadow the answer.
    let parsed = out
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .filter(|value| value.get("ok").and_then(Value::as_bool).is_some())
        .next_back();
    if let Some(value) = parsed {
        let ok = value["ok"].as_bool().unwrap_or(false);
        return DeviceReport {
            ok,
            adapter: text(&value, "adapter"),
            backend: text(&value, "backend"),
            reason: if ok {
                None
            } else {
                text(&value, "error").or_else(|| Some(explain(&err, timed_out, status)))
            },
            ..DeviceReport::default()
        };
    }

    // No JSON: a panic before the line could be printed, or an engine that predates the flag. The
    // exit status is then the only verdict, and the adapter comes from the human line.
    let succeeded = status.is_some_and(|status| status.success());
    DeviceReport {
        ok: succeeded,
        adapter: if succeeded {
            stderr_adapter(&err)
        } else {
            None
        },
        // `cpu` is known even on the legacy path: it is what was asked for and what the engine
        // reported. A GPU's backend stays unknown rather than being guessed.
        backend: (target == "cpu").then(|| "cpu".to_owned()),
        reason: if succeeded {
            None
        } else {
            Some(explain(&err, timed_out, status))
        },
        ..DeviceReport::default()
    }
}

fn drain(mut stream: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buffer = String::new();
        let _ = stream.read_to_string(&mut buffer);
        buffer
    })
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// Turn a failed probe's stderr into the one line a user can act on.
///
/// A panic's message is on the line *after* its `panicked at …` frame, so a reader that only keeps
/// lines matching a pattern gets the frame and loses the reason. The clean-failure path is simpler:
/// the engine prints `error [CODE]: message`.
fn explain(stderr: &str, timed_out: bool, status: Option<std::process::ExitStatus>) -> String {
    if timed_out {
        return format!(
            "探测 {} 秒没有返回，按显卡不可用处理（驱动卡住时不会报错，只会挂住）",
            PROBE_TIMEOUT.as_secs()
        );
    }
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if let Some(line) = lines.iter().find(|line| line.starts_with("error [")) {
        return clamp(line);
    }
    // clap's usage error is `error: unexpected argument '--json' found`, then a blank line, then a
    // usage block ending in "For more information, try '--help'." Keeping the last line would tell
    // the user to read the help text and nothing about what was wrong.
    if let Some(line) = lines.iter().find(|line| line.starts_with("error:")) {
        return clamp(line);
    }
    if let Some(index) = lines.iter().position(|line| line.contains("panicked at")) {
        if let Some(message) = lines.get(index + 1) {
            return clamp(message);
        }
        return clamp(lines[index]);
    }
    if let Some(line) = lines.last() {
        return clamp(line);
    }
    match status {
        Some(status) => format!("引擎退出码 {}", status.code().unwrap_or(-1)),
        None => "引擎没有返回任何信息".to_owned(),
    }
}

/// The `Device probe: <adapter> ok (sum N)` line, for engines that predate `--json`.
fn stderr_adapter(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .find_map(|line| {
            let rest = line.trim().strip_prefix("Device probe: ")?;
            Some(rest.split(" ok (").next().unwrap_or(rest).trim().to_owned())
        })
        .filter(|name| !name.is_empty())
}

fn clamp(line: &str) -> String {
    // Enough to identify the failure, short enough for a one-line status row. The full text stays
    // in the engine log, which is what the diagnostic report attaches.
    const LIMIT: usize = 300;
    if line.chars().count() <= LIMIT {
        return line.to_owned();
    }
    let mut clamped: String = line.chars().take(LIMIT).collect();
    clamped.push('…');
    clamped
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
    /// `None` in unit tests: they assert on the hub, and a file would only make them order-
    /// dependent on the filesystem.
    log: Option<Arc<LogFile>>,
}

impl Inner {
    /// Publish a human line *and* persist it. Every path that produces [`Message::Log`] goes
    /// through here, otherwise the file would silently miss exactly the lines a crash produces.
    fn note(&self, line: String) {
        if let Some(log) = &self.log {
            log.write(&line);
        }
        self.hub.publish(Message::Log(line));
    }
}

/// Handle to the engine process. Cheap to clone; all clones share one child and one hub.
#[derive(Clone)]
pub struct Engine(Arc<Inner>);

impl Default for Engine {
    fn default() -> Self {
        Self(Arc::new(Inner {
            child: Mutex::new(None),
            hub: Hub::default(),
            log: None,
        }))
    }
}

impl Engine {
    /// Same as [`Default`], but every human line is also appended to `<dir>/engine.log`.
    /// The app uses this; tests use `default()`.
    pub fn with_log_dir(dir: impl AsRef<Path>) -> Self {
        Self(Arc::new(Inner {
            child: Mutex::new(None),
            hub: Hub::default(),
            log: Some(Arc::new(LogFile::new(dir.as_ref().join("engine.log")))),
        }))
    }

    pub fn log_path(&self) -> Option<PathBuf> {
        self.0.log.as_ref().map(|log| log.path().to_path_buf())
    }

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
                self.0.note(format!(
                    "续训：从 {} 恢复 · 数据集 {} · 计划 {} 轮",
                    info.dir.display(),
                    info.dataset,
                    info.epochs
                ));
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
                        Err(_) => engine.0.note(trimmed.to_owned()),
                    }
                }
            });
        }

        if let Some(stderr) = stderr {
            let engine = self.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if !line.trim().is_empty() {
                        engine.0.note(line);
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

    /// A crash must not lose the lines just before it, which is why every write flushes.
    #[test]
    fn the_log_file_appends_flushes_and_keeps_a_path() {
        let dir = std::env::temp_dir().join(format!("yt-logfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = LogFile::new(dir.join("engine.log"));
        log.write("first line");
        log.write("second line");
        let text = std::fs::read_to_string(log.path()).expect("log file must exist");
        assert!(text.contains("first line"), "{text}");
        assert!(text.contains("second line"), "{text}");
        assert_eq!(text.lines().count(), 2, "one write must be one line");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The file is not an afterthought to the hub: the same call that publishes a line persists it.
    #[test]
    fn an_engine_with_a_log_dir_persists_the_human_lines_it_publishes() {
        let dir = std::env::temp_dir().join(format!("yt-enginelog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let engine = Engine::with_log_dir(&dir);
        engine.0.note("device lost on adapter Vega 6".into());
        let path = engine.log_path().expect("a log dir was given");
        let text = std::fs::read_to_string(path).expect("file written");
        assert!(text.contains("device lost on adapter Vega 6"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

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

    /// The engine puts a panic's reason on the line *after* its `panicked at …` frame. Keeping only
    /// the frame would show the user a source location and nothing about what went wrong.
    #[test]
    fn a_panic_reason_is_the_message_under_the_frame() {
        let stderr = "thread 'DSD-4-0' panicked at cubecl-wgpu/src/compute/timings.rs:510:22:\n\
                      Failed to map buffer: BufferAsyncError\n\n\
                      thread 'main' panicked at src/main.rs:730:36:\n\
                      Error while reading data: the device is poisoned\n";
        let reason = explain(stderr, false, None);
        assert_eq!(reason, "Failed to map buffer: BufferAsyncError", "{reason}");
    }

    /// A clean failure is the easier case and must not be shadowed by the panic rule.
    #[test]
    fn a_clean_failure_uses_the_error_line() {
        let stderr = "error [DEVICE_UNAVAILABLE]: no compatible GPU adapter\nhint: use --device cpu\n";
        assert_eq!(
            explain(stderr, false, None),
            "error [DEVICE_UNAVAILABLE]: no compatible GPU adapter"
        );
    }

    /// A wedged driver prints nothing at all, so the deadline is the only signal. Saying "退出码"
    /// here would suggest the engine had a verdict when it never spoke.
    #[test]
    fn a_silent_timeout_says_so_instead_of_inventing_an_exit_code() {
        let reason = explain("", true, None);
        assert!(reason.contains("没有返回"), "{reason}");
        assert!(!reason.contains("退出码"), "{reason}");
    }

    /// Old packaged engines answer with this line instead of JSON; the adapter is still in it.
    #[test]
    fn the_human_line_still_yields_the_adapter_name() {
        let stderr = "Device probe: AMD Radeon(TM) Graphics (spirv_4098_5688) ok (sum 32768)\n";
        assert_eq!(
            stderr_adapter(stderr).as_deref(),
            Some("AMD Radeon(TM) Graphics (spirv_4098_5688)")
        );
        assert_eq!(stderr_adapter("no probe output"), None);
    }

    /// One line in the UI, but a backtrace can be thousands of characters in the log.
    #[test]
    fn a_reason_is_clamped_but_not_mangled() {
        let long = "x".repeat(500);
        let clamped = clamp(&long);
        assert_eq!(clamped.chars().count(), 301, "300 plus the ellipsis");
        assert!(clamped.ends_with('…'));
        assert_eq!(clamp("short"), "short");
        // Clamping counts characters, not bytes: a multi-byte message must not be cut mid-codepoint.
        let wide = "显".repeat(400);
        assert_eq!(clamp(&wide).chars().count(), 301);
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

    /// End to end through a real child process. The point is not that this machine's GPU works —
    /// it does not — but that a probe always *answers*: an ok, a reason, or the timeout sentence,
    /// and never a panic escaping into the shell or an empty reply with no explanation.
    #[test]
    #[ignore = "spawns the real engine"]
    fn probing_the_real_engine_answers_for_both_devices() {
        let Ok(engine) = resolve(None) else {
            return;
        };

        let cpu = probe(&engine, "cpu");
        assert!(cpu.ok, "the cpu probe must succeed: {:?}", cpu.reason);
        assert_eq!(cpu.device, "cpu", "the report echoes what was asked");
        assert_eq!(cpu.backend.as_deref(), Some("cpu"));

        let gpu = probe(&engine, "gpu");
        assert!(
            gpu.ok || gpu.reason.is_some(),
            "a failed probe must say why: {gpu:?}"
        );
        // A working adapter answers in about a second; a broken one spends the whole deadline.
        assert!(gpu.elapsed_ms < 60_000, "probe outlived its own timeout");

        // The packaged sidecar is the engine that speaks `--json`; `resolve` above may well have
        // found a stale build from the repository layout instead. Both dialects must answer, which
        // is the whole reason the retry exists.
        let sidecar = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join("montgomery-x86_64-pc-windows-msvc.exe");
        if sidecar.is_file() {
            let report = probe(&sidecar, "cpu");
            assert!(report.ok, "the sidecar probe failed: {:?}", report.reason);
            assert_eq!(report.backend.as_deref(), Some("cpu"));
            assert!(
                report.reason.is_none(),
                "a successful probe carries no excuse: {report:?}"
            );
        }
    }
}
