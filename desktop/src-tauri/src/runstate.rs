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
    /// Validation fitness from the last epoch. This is the number that actually says whether the
    /// model is any good — `last_loss` alone cannot separate "learning" from "memorising". `None`
    /// when running with `--no-val`, where the engine emits no validation at all.
    pub last_fitness: Option<f64>,
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
            last_fitness: None,
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

/// What `GET /events?since=N` answers with.
#[derive(Debug, Clone, Serialize)]
pub struct Delta {
    /// Send this back as `since` next time.
    pub cursor: u64,
    /// The caller's cursor was **ahead** of this process's counter. `seq` is per-process and
    /// starts at 0, so this is what a cursor saved before a restart looks like: `since=500` while
    /// the fresh process is at `seq=3`. Rather than returning an empty list until the counter
    /// climbs past 500 — which a client that only advances its cursor on a non-empty response
    /// would never escape — the events are replayed from the start of the buffer.
    pub reset: bool,
    /// At least one event between the caller's cursor and the oldest retained one was dropped.
    /// The buffer holds `MAX_EVENTS`; engine log lines are events too, so a chatty run can push
    /// old epochs out. Worth surfacing: a client that misses an `end` event and is told "nothing
    /// new" polls forever.
    pub truncated: bool,
    pub events: Vec<Value>,
}

impl RunState {
    /// Announce a run before the engine has said anything about it.
    ///
    /// `reset` alone leaves the status at `idle`, and there is a real gap between `POST /train`
    /// returning `202` and the engine's `start` event — initialising the tensors takes tens of
    /// seconds on this hardware. A client polling in that window sees `idle`, concludes the
    /// request was ignored, and re-sends it, which now fails with `409`. `running` is the honest
    /// answer: the work has been accepted.
    pub fn begin(&mut self, epochs: u32) {
        let seq = self.seq;
        *self = Self {
            seq,
            status: "running".into(),
            total_epochs: epochs,
            ..Self::default()
        };
    }

    /// The engine never started — bad path, spawn refused. Recorded through `apply` so the failure
    /// also shows up in `/events`; leaving `running` up would make a client poll until its own
    /// timeout with no explanation.
    pub fn fail(&mut self, message: &str) {
        self.apply(&Message::Event(json!({
            "type": "error",
            "code": "spawn_failed",
            "message": message,
            "hint": "检查设置里的引擎路径是否存在，或改用内置引擎。"
        })));
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
                self.last_fitness = number(&value, "fitness");
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
    /// rather than an error: a gap is tolerable, a 500 is not. What is *not* tolerable is a gap
    /// the caller cannot see, hence `truncated`.
    pub fn since(&self, since: u64) -> Delta {
        let reset = since > self.seq;
        let effective = if reset { 0 } else { since };
        let truncated = self
            .events
            .front()
            .map(|(oldest, _)| *oldest > effective.saturating_add(1))
            .unwrap_or(false);
        Delta {
            cursor: self.seq,
            reset,
            truncated,
            events: self
                .events
                .iter()
                .filter(|(seq, _)| *seq > effective)
                .map(|(_, value)| value.clone())
                .collect(),
        }
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
        let delta = state.since(1);
        assert_eq!(delta.cursor, 2);
        assert_eq!(delta.events.len(), 1);
        assert_eq!(delta.events[0]["type"], "log");
        assert!(!delta.reset && !delta.truncated);
    }

    /// Validation fitness is the only number that says whether the model is any good; dropping it
    /// left an AI client with `last_loss`, which happily decreases on a model that has memorised
    /// four images.
    #[test]
    fn validation_fitness_is_recorded() {
        let mut state = RunState::default();
        state.apply(&event(json!({
            "type": "epoch", "epoch": 1, "epochs": 2, "loss": 3.5, "fitness": 0.0421
        })));
        assert_eq!(state.last_fitness, Some(0.0421));

        // `--no-val` emits `fitness: null`; that must read as "not measured", not as 0.0.
        state.apply(&event(json!({
            "type": "epoch", "epoch": 2, "epochs": 2, "loss": 3.1, "fitness": null
        })));
        assert_eq!(state.last_fitness, None);
    }

    /// A client that saved its cursor before the app restarted holds a number the new process has
    /// never reached. Returning "nothing new" forever is the failure this guards against.
    #[test]
    fn cursor_from_a_previous_process_replays_instead_of_stalling() {
        let mut state = RunState::default();
        state.apply(&event(json!({ "type": "start", "epochs": 1 })));
        // seq is now 1, but the client is still asking from 500.
        let delta = state.since(500);
        assert!(delta.reset, "must be flagged so the client can tell it restarted");
        assert_eq!(delta.cursor, 1);
        assert_eq!(delta.events.len(), 1, "the retained events are replayed, not dropped");
    }

    /// The ring buffer is finite and log lines count, so old events can disappear. A client has to
    /// be able to tell "nothing new" from "you missed some".
    #[test]
    fn truncation_is_visible() {
        let mut state = RunState::default();
        for index in 0..(MAX_EVENTS + 25) {
            state.apply(&Message::Log(format!("line {index}")));
        }
        let delta = state.since(0);
        assert!(delta.truncated, "25 events were evicted and the caller must know");
        assert_eq!(delta.events.len(), MAX_EVENTS);

        // Polling often enough keeps the flag off.
        let fresh = state.since(state.seq - 1);
        assert!(!fresh.truncated);
    }

    /// There is a real window between `POST /train` returning 202 and the engine's first event —
    /// initialising the tensors takes tens of seconds here. A client polling in that window used
    /// to see `idle`, conclude the request was ignored, and re-send it, which then failed 409.
    #[test]
    fn begin_announces_the_run_before_the_engine_does() {
        let mut state = RunState::default();
        state.apply(&event(json!({ "type": "end", "best": "old/best.bpk" })));
        let cursor = state.seq;

        state.begin(20);
        assert_eq!(state.status, "running");
        assert_eq!(state.total_epochs, 20);
        assert_eq!(state.best, None, "the previous run's artifact must not carry over");
        assert_eq!(
            state.seq, cursor,
            "nothing has happened yet, so the cursor must stand still"
        );
    }

    /// A spawn failure has to read as `failed`. Leaving `running` up makes every client poll until
    /// its own timeout with no explanation.
    #[test]
    fn failed_spawn_is_recorded_as_failure() {
        let mut state = RunState::default();
        state.begin(5);
        state.fail("引擎不存在");
        assert_eq!(state.status, "failed");
        assert_eq!(state.error.as_ref().unwrap()["code"], "spawn_failed");
        // Recorded through `apply`, so it also reaches `/events` as a real event.
        let delta = state.since(0);
        assert_eq!(delta.events.len(), 1);
        assert_eq!(delta.events[0]["type"], "error");
    }
}
