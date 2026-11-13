//! Everything the shell and the local control service both need.
//!
//! Before this existed, settings lived in a `tauri::State` and the paths were resolved from an
//! `AppHandle`, which meant an HTTP handler could not read them without dragging Tauri into the
//! service. The two front ends (window, HTTP) now share one `Runtime`; both are thin adapters over
//! it. See DECISIONS D011 (the split) and D017 (why the service is a thread, not a runtime).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;

use crate::engine::{Engine, TrainRequest};
// Only `record` (test-only) names this; production folding lives in the subscriber thread.
#[cfg(test)]
use crate::engine::Message;
use crate::runstate::{Delta, RunState};
use crate::settings::{self, Settings};

/// What the settings page renders, and what `GET /settings` returns. Identical on purpose: one
/// shape to document, one place to fix when it changes.
#[derive(Serialize)]
pub struct SettingsView {
    /// What the user configured; `None` means auto-detect.
    pub engine_path: Option<String>,
    /// What auto-detection actually found, if anything.
    pub engine_resolved: Option<String>,
    /// Why the configured path is unusable, if it is.
    pub engine_error: Option<String>,
    pub default_device: String,
    /// Always absolute and ready to pass to `--project`.
    pub output_dir: String,
    /// True when `output_dir` is the app's own default rather than a user choice.
    pub output_dir_is_default: bool,
    pub config_dir: String,
    pub engine_running: bool,
    /// The port the user configured (`0` = let the OS pick). This is **not** `control.port`,
    /// which is the port actually bound after a fallback. Saving `control.port` back as the
    /// configured value used to pin a random port into `settings.json` and restart the service
    /// on every unrelated settings change.
    pub control_port: u16,
    /// State of the AI-facing HTTP service, including the port it actually got — which is not
    /// necessarily the one that was asked for (see `control::start`).
    pub control: ControlInfo,
    /// Where a failure summary is pushed (`wecom` | `pushplus` | `generic`).
    pub feedback_kind: String,
    /// Empty means reports are written locally and not sent anywhere.
    pub feedback_webhook: String,
    /// Where the local failure reports live. Shown so a user can find the file to send.
    pub diagnostics_dir: String,
}

/// Live state of the control service. Written once at startup and again whenever the user toggles
/// it, read by both front ends.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ControlInfo {
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    /// Bearer token. Shown in the UI so it can be copied into a prompt; never logged.
    pub token: String,
    pub base_url: String,
    pub error: Option<String>,
}

struct Inner {
    engine: Engine,
    store: settings::Store,
    settings: Mutex<Settings>,
    /// App config directory (`settings.json`, control token live here).
    config_dir: PathBuf,
    /// Where `runs/` goes when the user has not picked a folder.
    default_output_dir: PathBuf,
    /// App data directory. `logs/` and `diagnostics/` live here, next to `runs/`.
    data_dir: PathBuf,
    run: Mutex<RunState>,
    control: Mutex<ControlInfo>,
    /// The live service, when one is listening. Holding the handle is what makes the toggle
    /// immediate: `stop()` flips a flag the accept loop polls.
    control_handle: Mutex<Option<crate::control::Handle>>,
    /// Dedupe key of the last mid-run failure that was written up. The engine emits an `error`
    /// event and then an `exit` event, and both leave `status = failed`; without this a single
    /// crash would file two reports and burn two of the three pushes allowed per minute.
    reported_failure: Mutex<Option<String>>,
}

#[derive(Clone)]
pub struct Runtime(Arc<Inner>);

/// Write up a run that died, off the folding thread.
///
/// Runs on its own thread because `diagnose` may do a network call with a 5 s timeout, and the
/// caller is the thread that folds every engine event — blocking it would stall the very progress
/// reporting a user looking at a failed run is watching.
fn report_failure(inner: &Arc<Inner>, run_dir: Option<String>, error: Option<Value>) {
    let code = error
        .as_ref()
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("run_failed")
        .to_owned();
    {
        let Ok(mut seen) = inner.reported_failure.lock() else {
            return;
        };
        let key = format!("{}|{code}", run_dir.clone().unwrap_or_default());
        if seen.as_deref() == Some(key.as_str()) {
            return;
        }
        *seen = Some(key);
    }
    let message = error
        .as_ref()
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("训练失败，引擎没有给出原因。")
        .to_owned();
    let hint = error
        .as_ref()
        .and_then(|value| value.get("hint"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let handle = Runtime(inner.clone());
    std::thread::spawn(move || {
        let draft = crate::diagnostics::Draft {
            kind: "train_failed",
            code: &code,
            message: &message,
            hint: hint.as_deref(),
            context: serde_json::json!({ "run_dir": run_dir }),
        };
        let _ = handle.diagnose(&draft, None);
    });
}

impl Runtime {
    pub fn new(config_dir: PathBuf, app_data_dir: PathBuf) -> Self {
        let store = settings::Store::new(config_dir.join("settings.json"));
        let current = store.load();
        let runtime = Self(Arc::new(Inner {
            // Logs live beside `runs/` so a user told "send me %APPDATA%\com.yolotrainer.desktop"
            // finds both in one place.
            engine: Engine::with_log_dir(app_data_dir.join("logs")),
            store,
            settings: Mutex::new(current),
            config_dir,
            default_output_dir: app_data_dir.join("runs"),
            data_dir: app_data_dir,
            run: Mutex::new(RunState::default()),
            control: Mutex::new(ControlInfo::default()),
            control_handle: Mutex::new(None),
            reported_failure: Mutex::new(None),
        }));

        // Fold engine messages here, not in the Tauri setup. The hub fans out to every subscriber
        // (`Vec<mpsc::Sender>`), so the window gets its own copy in `lib.rs`; putting the fold
        // there instead made the HTTP contract depend on the UI thread being alive. A process
        // with no window — a test, or a future headless mode — would then answer `idle` forever
        // while the engine trained happily.
        //
        // `Weak`, not `Arc`: a recorder holding a strong reference would keep this Runtime alive
        // for the life of the process, so dropping the last real owner would never end the thread.
        let weak = Arc::downgrade(&runtime.0);
        let receiver = runtime.0.engine.subscribe();
        std::thread::spawn(move || {
            let mut was_running = false;
            for message in receiver {
                let Some(inner) = weak.upgrade() else { break };
                let (status, run_dir, error) = {
                    let Ok(mut run) = inner.run.lock() else {
                        continue;
                    };
                    run.apply(&message);
                    // Only the three fields the report needs: cloning the whole `RunState` here
                    // would copy the event ring buffer on every single log line.
                    (run.status.clone(), run.run_dir.clone(), run.error.clone())
                };
                // A run that dies mid-training is the case most worth a report: the user was
                // watching it, and the `error` event that explains why flashes past in the log
                // panel. Handled on a separate thread because writing the file and pushing the
                // summary can take seconds, and this thread folds the event stream.
                if status == "failed" && was_running {
                    report_failure(&inner, run_dir, error);
                }
                was_running = status == "running";
            }
        });

        runtime
    }

    pub fn engine(&self) -> &Engine {
        &self.0.engine
    }

    pub fn config_dir(&self) -> &Path {
        &self.0.config_dir
    }

    pub fn settings(&self) -> Settings {
        self.0
            .settings
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    pub fn output_root(&self) -> PathBuf {
        self.settings().output_dir_or(&self.0.default_output_dir)
    }

    pub fn view(&self) -> SettingsView {
        let current = self.settings();
        let resolved = crate::engine::resolve(current.engine_path.as_deref());
        let (engine_resolved, engine_error) = match &resolved {
            Ok(path) => (Some(path.to_string_lossy().into_owned()), None),
            Err(error) => (None, Some(error.clone())),
        };
        let default_dir = &self.0.default_output_dir;
        SettingsView {
            engine_path: current.engine_path.clone(),
            engine_resolved,
            engine_error,
            default_device: current.default_device.clone(),
            output_dir: current
                .output_dir_or(default_dir)
                .to_string_lossy()
                .into_owned(),
            output_dir_is_default: current.output_dir.is_none(),
            config_dir: self.0.config_dir.to_string_lossy().into_owned(),
            engine_running: self.0.engine.is_running(),
            control_port: current.control_port,
            control: self.control(),
            feedback_kind: current.feedback_kind.clone(),
            feedback_webhook: current.feedback_webhook.clone(),
            diagnostics_dir: self.diagnostics_root().to_string_lossy().into_owned(),
        }
    }

    /// Persist, then swap the cache. The order matters: a failed write must not leave the UI
    /// showing a value that will be gone after a restart.
    pub fn save_settings(&self, next: Settings) -> Result<SettingsView, String> {
        let normalized = next.normalize();
        let before = self.settings();
        self.0.store.save(&normalized)?;
        if let Ok(mut guard) = self.0.settings.lock() {
            *guard = normalized;
        }
        self.restart_control_if_needed(&before);
        Ok(self.view())
    }

    pub fn start_training(&self, request: &TrainRequest) -> Result<(), String> {
        let current = self.settings();
        let exe = crate::engine::resolve(current.engine_path.as_deref())?;
        let output = self.output_root();
        // Announce the run before spawning. Between this call and the engine's first `start`
        // event there is a multi-second tensor-initialisation window, and a client polling in it
        // must see `running` — `idle` reads as "the request was ignored".
        if let Ok(mut run) = self.0.run.lock() {
            run.begin(request.epochs);
        }
        match self
            .0
            .engine
            .start(&exe, &output, request, &current.default_device)
        {
            Ok(()) => Ok(()),
            Err(error) => {
                // Nothing started, so `running` would be a lie that outlives the response body.
                if let Ok(mut run) = self.0.run.lock() {
                    run.fail(&error);
                }
                Err(error)
            }
        }
    }

    pub fn stop_training(&self) -> Result<(), String> {
        self.0.engine.stop()
    }

    /// Fold one engine message into the snapshot without going through the hub.
    ///
    /// Production folding happens on the subscriber thread started in `new`; this exists so a
    /// test can drive the snapshot from a known message sequence without starting an engine.
    #[cfg(test)]
    pub fn record(&self, message: &Message) {
        if let Ok(mut run) = self.0.run.lock() {
            run.apply(message);
        }
    }

    pub fn run_snapshot(&self) -> Value {
        match self.0.run.lock() {
            Ok(run) => serde_json::to_value(&*run).unwrap_or_else(|_| Value::Null),
            Err(_) => Value::Null,
        }
    }

    /// `(cursor, events newer than since)` for `GET /events`, plus the flags that tell the caller
    /// why it may be seeing more or less than it asked for.
    pub fn events_since(&self, since: u64) -> Delta {
        match self.0.run.lock() {
            Ok(run) => run.since(since),
            // A poisoned lock must not take the service down. An empty delta is a truthful "I
            // cannot see any events right now", and the client retries on its next poll.
            Err(_) => Delta {
                cursor: since,
                reset: false,
                truncated: false,
                events: Vec::new(),
            },
        }
    }

    pub fn control(&self) -> ControlInfo {
        self.0
            .control
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    pub fn set_control(&self, info: ControlInfo) {
        if let Ok(mut guard) = self.0.control.lock() {
            *guard = info;
        }
    }

    /// Start the service if settings ask for it, otherwise record that it is off. Called once at
    /// startup and again whenever the control settings change.
    pub fn start_control(&self) -> ControlInfo {
        self.stop_control();
        let current = self.settings();
        let mut info = ControlInfo {
            enabled: current.control_enabled,
            running: false,
            port: current.control_port,
            token: String::new(),
            base_url: String::new(),
            error: None,
        };
        if current.control_enabled {
            match crate::control::start(self.clone(), current.control_port) {
                Ok(handle) => {
                    info.running = true;
                    info.port = handle.port;
                    info.token = handle.token.clone();
                    info.base_url = format!("http://127.0.0.1:{}", handle.port);
                    if let Ok(mut guard) = self.0.control_handle.lock() {
                        *guard = Some(handle);
                    }
                }
                Err(error) => info.error = Some(error),
            }
        }
        self.set_control(info.clone());
        info
    }

    /// Stop the service. Idempotent: the settings page calls it through a toggle that may already
    /// be off.
    pub fn stop_control(&self) {
        if let Ok(mut guard) = self.0.control_handle.lock() {
            if let Some(handle) = guard.take() {
                handle.stop();
            }
        }
        let mut info = self.control();
        info.running = false;
        info.base_url.clear();
        info.error = None;
        self.set_control(info);
    }

    /// Restart only when something that affects the listener changed. Calling `start_control`
    /// unconditionally would drop in-flight requests every time the user saved an unrelated field.
    fn restart_control_if_needed(&self, before: &Settings) {
        let after = self.settings();
        if before.control_enabled == after.control_enabled
            && before.control_port == after.control_port
        {
            return;
        }
        if after.control_enabled {
            self.start_control();
        } else {
            self.stop_control();
            let mut info = self.control();
            info.enabled = false;
            self.set_control(info);
        }
    }

    /// Where failure reports are written.
    pub fn diagnostics_root(&self) -> PathBuf {
        self.0.data_dir.join("diagnostics")
    }

    /// Everything a maintainer needs to judge a report without asking a follow-up question.
    ///
    /// The engine's own size and mtime are the most valuable entries here: this project has more
    /// than once "fixed" a bug in a patch while the packaged engine was still the old binary, and
    /// that is invisible in every other field.
    pub fn environment(&self) -> Value {
        let current = self.settings();
        let snapshot = self.run_snapshot();
        let resolved = crate::engine::resolve(current.engine_path.as_deref()).ok();
        let (engine_bytes, engine_modified) = resolved
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|meta| {
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|since| since.as_secs())
                    .unwrap_or(0);
                (meta.len(), modified)
            })
            .unwrap_or((0, 0));
        serde_json::json!({
            "version": crate::version::VERSION,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "cores": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
            "backend": snapshot.get("adapter").cloned().unwrap_or(Value::Null),
            "device_setting": current.default_device,
            "status": snapshot.get("status").cloned().unwrap_or(Value::Null),
            "epoch": snapshot.get("epoch").cloned().unwrap_or(Value::Null),
            "total_epochs": snapshot.get("total_epochs").cloned().unwrap_or(Value::Null),
            "engine": resolved.map(|path| path.to_string_lossy().into_owned()),
            "engine_bytes": engine_bytes,
            "engine_modified_unix": engine_modified,
            "output_dir": self.output_root().to_string_lossy(),
        })
    }

    /// Write a report for `draft`, then push its summary if a channel is configured.
    ///
    /// Returns `None` only when the report file itself could not be written. Every other problem —
    /// no channel, dead webhook, rate limited — is reported through the returned struct, because
    /// the caller is already handling a failure and must not be handed a second one.
    pub fn diagnose(
        &self,
        draft: &crate::diagnostics::Draft<'_>,
        dataset: Option<&str>,
    ) -> Option<Diagnostic> {
        let home = std::env::var("USERPROFILE").ok();
        let tail = self
            .0
            .engine
            .log_path()
            .map(|path| crate::diagnostics::log_tail(&path, 120))
            .unwrap_or_default();
        let written = crate::diagnostics::write(
            &self.diagnostics_root(),
            draft,
            &self.environment(),
            &tail,
            home.as_deref(),
            dataset,
        )
        .ok()?;

        let current = self.settings();
        let mut pushed = false;
        let mut push_error = None;
        if !current.feedback_webhook.is_empty() {
            let snapshot = self.run_snapshot();
            let env_line = format!(
                "{} {} · {} 核 · 后端 {}",
                std::env::consts::OS,
                std::env::consts::ARCH,
                std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
                snapshot
                    .get("adapter")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            );
            let body = crate::feedback::compose(
                crate::version::VERSION,
                &written.created,
                draft.code,
                &written.summary,
                &written.id,
                &env_line,
            );
            let title = format!("YOLO Trainer {} {}", crate::version::VERSION, draft.code);
            match crate::feedback::push(
                &current.feedback_kind,
                &current.feedback_webhook,
                &title,
                &body,
                &written.id,
            ) {
                Ok(()) => {
                    pushed = true;
                    crate::diagnostics::mark_pushed(Path::new(&written.path));
                }
                Err(error) => push_error = Some(error),
            }
        }

        Some(Diagnostic {
            id: written.id,
            path: written.path,
            summary: written.summary,
            pushed,
            push_error,
        })
    }
}

/// What a caller gets back about a failure they just caused.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub id: String,
    /// Absolute path to the local report. Always present when this struct exists.
    pub path: String,
    pub summary: String,
    /// True only when the summary actually reached the configured channel.
    pub pushed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub push_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Message;

    fn temp_runtime() -> (Runtime, PathBuf) {
        // A unique directory per test: settings are file-backed, and two tests sharing one file
        // would race over the same JSON.
        let root = std::env::temp_dir().join(format!(
            "yolo-runtime-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let runtime = Runtime::new(root.join("config"), root.join("data"));
        (runtime, root)
    }

    #[test]
    fn settings_round_trip_through_disk() {
        let (runtime, root) = temp_runtime();
        let mut next = runtime.settings();
        next.default_device = "cpu".into();
        next.output_dir = Some("D:/runs".into());
        runtime.save_settings(next).expect("save should succeed");

        // A second runtime over the same file is what a restart looks like.
        let reopened = Runtime::new(root.join("config"), root.join("data"));
        assert_eq!(reopened.settings().default_device, "cpu");
        assert_eq!(reopened.output_root(), PathBuf::from("D:/runs"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn view_always_reports_an_absolute_default_output_dir() {
        let (runtime, root) = temp_runtime();
        let view = runtime.view();
        assert!(view.output_dir_is_default);
        assert!(view.output_dir.ends_with("runs"), "{}", view.output_dir);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The snapshot must be cheap to clone and safe to read while a run is publishing.
    #[test]
    fn run_snapshot_reflects_recorded_events() {
        let (runtime, root) = temp_runtime();
        runtime.record(&Message::Event(serde_json::json!({
            "type": "epoch", "epoch": 3, "epochs": 10, "loss": 2.5, "elapsed_s": 0.4
        })));
        let snapshot = runtime.run_snapshot();
        assert_eq!(snapshot["epoch"], 3);
        assert_eq!(snapshot["last_loss"], 2.5);
        let _ = std::fs::remove_dir_all(&root);
    }
}
