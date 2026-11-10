//! A snapshot of the current (or most recent) training run.
//!
//! The engine emits a stream; the UI keeps its own copy in TypeScript. The control service has no
//! UI, so it needs the same facts in Rust: what is running, how far along, what came out. This
//! module turns the event stream into one readable object.
//!
//! Kept in Rust rather than asking the webview for its state: the service must answer with the
//! window closed to the tray, and a closed window has no JavaScript running.

use std::collections::VecDeque;

use serde::Serialize;
use serde_json::{json, Value};

use crate::engine::Message;

/// How many events to retain. A long run emits one `epoch` per epoch plus narration; 400 is a few
/// hundred epochs of context, which is more than any client polls back for.
const MAX_EVENTS: usize = 400;

#[derive(Debug, Clone, Serialize)]
pub struct RunState {
    /// `idle` | `running` | `done` | `failed`
    pub status: String,
    pub epoch: u32,
    pub total_epochs: u32,
    pub last_loss: Option<f64>,
    pub last_epoch_seconds: Option<f64>,
    /// What the engine actually used, not what was requested.
    pub backend: Option<String>,
    pub adapter: Option<String>,
    pub run_dir: Option<String>,
    /// Exported weights, present only after a successful `end`.
    pub best: Option<String>,
    pub last_model: Option<String>,
    /// The `error` event verbatim: `{code, message, hint}`.
    pub error: Option<Value>,
    pub exit_code: Option<i32>,
    /// Monotonic counter of everything published since the last reset. Clients poll with
    /// `?since=` to get only what is new, which is what makes polling cheap enough to skip SSE.
    pub seq: u64,
    #[serde(skip)]
    events: VecDeque<(u64, Value)>,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            status: "idle".into(),
            epoch: 0,
            total_epochs: 0,
            last_loss: None,
            last_epoch_seconds: None,
            backend: None,
            adapter: None,
            run_dir: None,
            best: None,
            last_model: None,
            error: None,
            exit_code: None,
            seq: 0,
            events: VecDeque::new(),
        }
    }
}

impl RunState {
    /// Clear the previous run. Called when a new one is accepted, not when the first event
    /// arrives: a client that starts a run and immediately polls must not see the last run's
    /// numbers and conclude it already finished.
    pub fn reset(&mut self) {
        let seq = self.seq;
        *self = Self {
            seq,
            ..Self::default()
        };
    }

    /// Fold one hub message in. Log lines are kept as `{type:"log"}` events so that a client
    /// reading `/events` sees the same narrative the UI does — a panic line on stderr is often
    /// the only explanation for a run that dies without an `error` event.
    pub fn apply(&mut self, message: &Message) {
        let value = match message {
            Message::Event(value) => value.clone(),
            Message::Log(line) => json!({ "type": "log", "line": line }),
        };

        match value.get("type").and_then(Value::as_str) {
            Some("start") => {
                self.status = "running".into();
                self.epoch = 0;
                self.total_epochs = number(&value, "epochs").unwrap_or(0.0) as u32;
                self.backend = value
                    .get("backend")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                self.adapter = value
                    .get("adapter")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                self.run_dir = value.get("run").and_then(Value::as_str).map(str::to_owned);
            }
            Some("epoch") => {
                self.status = "running".into();
                self.epoch = number(&value, "epoch").unwrap_or(self.epoch as f64) as u32;
                if let Some(total) = number(&value, "epochs") {
                    self.total_epochs = total as u32;
                }
                self.last_loss = number(&value, "loss");
                self.last_epoch_seconds = number(&value, "elapsed_s");
            }
            Some("end") => {
                self.status = "done".into();
                self.best = value.get("best").and_then(Value::as_str).map(str::to_owned);
                self.last_model = value.get("last").and_then(Value::as_str).map(str::to_owned);
            }
            Some("error") => {
                self.status = "failed".into();
                self.error = Some(value.clone());
            }
            Some("exit") => {
                self.exit_code = value.get("code").and_then(Value::as_i64).map(|v| v as i32);
                // A process that dies without saying why must not stay "running" forever.
                if self.status == "running" {
                    self.status = if self.exit_code == Some(0) {
                        "done".into()
                    } else {
                        "failed".into()
                    };
                }
            }
            _ => {}
        }

        self.seq += 1;
        self.events.push_back((self.seq, value));
        while self.events.len() > MAX_EVENTS {
            self.events.pop_front();
        }
    }

    /// Everything newer than `since`, plus the cursor the caller should send next.
    ///
    /// A caller that has fallen off the back of the ring buffer gets whatever is still retained
    /// rather than an error: a gap is tolerable, a 500 is not.
    pub fn since(&self, since: u64) -> (u64, Vec<&Value>) {
        let events = self
            .events
            .iter()
            .filter(|(seq, _)| *seq > since)
            .map(|(_, value)| value)
            .collect();
        (self.seq, events)
    }
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(value: Value) -> Message {
        Message::Event(value)
    }

    #[test]
    fn folds_a_complete_run() {
        let mut state = RunState::default();
        state.apply(&event(json!({
            "type": "start", "run": "runs/train", "backend": "cpu",
            "adapter": "Device<Flex(Cpu)>", "epochs": 2
        })));
        assert_eq!(state.status, "running");
        assert_eq!(state.total_epochs, 2);

        state.apply(&event(json!({
            "type": "epoch", "epoch": 1, "epochs": 2, "loss": 12.5, "elapsed_s": 1.25
        })));
        assert_eq!(state.epoch, 1);
        assert_eq!(state.last_loss, Some(12.5));

        state.apply(&event(json!({ "type": "end", "best": "out/best.bpk", "last": "out/last.bpk" })));
        assert_eq!(state.status, "done");
        assert_eq!(state.best.as_deref(), Some("out/best.bpk"));

        state.apply(&event(json!({ "type": "exit", "code": 0 })));
        assert_eq!(state.exit_code, Some(0));
        assert_eq!(state.status, "done");
    }

    /// A run that panics never emits `error`; only the exit code says it failed. Leaving the
    /// status at "running" would make every client poll forever.
    #[test]
    fn silent_death_becomes_failure() {
        let mut state = RunState::default();
        state.apply(&event(json!({ "type": "start", "epochs": 10 })));
        state.apply(&event(json!({ "type": "exit", "code": 101 })));
        assert_eq!(state.status, "failed");
        assert_eq!(state.exit_code, Some(101));
    }

    #[test]
    fn since_returns_only_new_events() {
        let mut state = RunState::default();
        state.apply(&event(json!({ "type": "start", "epochs": 1 })));
        state.apply(&Message::Log("hello".into()));
        let (cursor, events) = state.since(1);
        assert_eq!(cursor, 2);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "log");
    }

    #[test]
    fn reset_keeps_the_cursor() {
        let mut state = RunState::default();
        state.apply(&event(json!({ "type": "start", "epochs": 1 })));
        let cursor = state.seq;
        state.reset();
        assert_eq!(state.seq, cursor, "cursor must not go backwards");
        assert_eq!(state.status, "idle");
        assert_eq!(state.epoch, 0);
    }
}
