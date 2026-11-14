import { useSyncExternalStore } from "react";
import type { DatasetSummary, EngineEvent } from "./api";

/**
 * A minimal external store.
 *
 * Training state has to survive page switches and outlive any single component, so it cannot live
 * in a page's `useState`. Twenty lines of pub/sub is enough here — a state library would add a
 * dependency for something this small.
 */

export type LogLine = {
  /** Monotonic, unlike the array index: the log is capped at 400 lines, and dropping the
   *  oldest one shifts every index by one — which makes React re-key the whole list and
   *  replay the fade-in on lines that have been on screen for minutes. */
  id: number;
  time: string;
  text: string;
  tone?: "warn" | "danger";
};

export type AppState = {
  dataset: DatasetSummary | null;
  /** `detect` | `segment` | `classify`, set by the page that started the run. The monitor needs it
   *  to name the validation number correctly: detection and segmentation report mAP, classification
   *  reports accuracy, and calling one the other would be a wrong number under a right-looking
   *  label. */
  task: string | null;
  /** Lifecycle of the current run, driven purely by engine events. */
  status: "idle" | "running" | "done" | "failed";
  backend: string | null;
  adapter: string | null;
  runDir: string | null;
  totalEpochs: number;
  epoch: number;
  losses: Array<[number, number]>;
  lastLoss: number | null;
  /** Validation fitness from the last epoch (detection: mAP). The only number that says whether the
   *  model is any good — loss falls on a model that memorised four images. `null` under `--no-val`,
   *  which must read as "not measured" rather than as zero. */
  lastFitness: number | null;
  lastEpochSeconds: number | null;
  best: string | null;
  error: { code: string; message: string; hint: string | null } | null;
  logs: LogLine[];
};

const EMPTY: AppState = {
  dataset: null,
  task: null,
  status: "idle",
  backend: null,
  adapter: null,
  runDir: null,
  totalEpochs: 0,
  epoch: 0,
  losses: [],
  lastLoss: null,
  lastFitness: null,
  lastEpochSeconds: null,
  best: null,
  error: null,
  logs: [],
};

let state: AppState = EMPTY;

const listeners = new Set<() => void>();

function emit() {
  for (const listener of listeners) listener();
}

export function getState() {
  return state;
}

export function setState(patch: Partial<AppState> | ((prev: AppState) => Partial<AppState>)) {
  const next = typeof patch === "function" ? patch(state) : patch;
  state = { ...state, ...next };
  emit();
}

export function resetRun() {
  state = {
    ...state,
    status: "idle",
    task: null,
    backend: null,
    adapter: null,
    runDir: null,
    totalEpochs: 0,
    epoch: 0,
    losses: [],
    lastLoss: null,
    lastFitness: null,
    lastEpochSeconds: null,
    best: null,
    error: null,
    logs: [],
  };
  emit();
}

function now() {
  const date = new Date();
  return [date.getHours(), date.getMinutes(), date.getSeconds()]
    .map((value) => String(value).padStart(2, "0"))
    .join(":");
}

let logSeq = 0;

export function pushLog(text: string, tone?: LogLine["tone"]) {
  setState((prev) => ({
    // Keep the tail bounded: a long run can emit thousands of lines.
    logs: [...prev.logs, { id: ++logSeq, time: now(), text, tone }].slice(-400),
  }));
}

/** Fold one engine event into the store. Unknown types are ignored by design. */
export function applyEvent(event: EngineEvent) {
  switch (event.type) {
    case "start":
      setState({
        status: "running",
        backend: event.backend,
        adapter: event.adapter,
        runDir: event.run,
        totalEpochs: event.epochs,
        epoch: 0,
        losses: [],
        lastLoss: null,
        lastFitness: null,
        best: null,
        error: null,
      });
      pushLog(
        `start · ${event.backend} · ${event.adapter} · ${event.epochs} 轮 · batch ${event.batch_size}`,
      );
      if (event.backend === "cpu") {
        pushLog("显卡不可用，已回退到 CPU，训练会慢一些。", "warn");
      }
      break;

    case "epoch":
      setState((prev) => ({
        epoch: event.epoch,
        totalEpochs: event.epochs,
        losses: [...prev.losses, [event.epoch, event.loss]],
        lastLoss: event.loss,
        lastFitness: event.fitness,
        lastEpochSeconds: event.elapsed_s,
      }));
      pushLog(
        `epoch ${event.epoch}/${event.epochs} · loss ${event.loss.toFixed(4)} · ${event.elapsed_s.toFixed(2)}s`,
      );
      break;

    case "end":
      setState({ status: "done", best: event.best, runDir: event.run });
      pushLog(`完成 · ${event.best ?? event.run}`);
      break;

    case "error":
      setState({
        status: "failed",
        error: { code: event.code, message: event.message, hint: event.hint },
      });
      pushLog(`[${event.code}] ${event.message}`, "danger");
      if (event.hint) pushLog(`提示：${event.hint}`, "danger");
      break;

    case "exit":
      setState((prev) => ({
        // An exit without a terminal event means the process died mid-run (usually the driver).
        status:
          prev.status === "running"
            ? event.code === 0
              ? "done"
              : "failed"
            : prev.status,
      }));
      if (event.code !== 0 && event.code !== null) {
        pushLog(`引擎进程退出，代码 ${event.code}`, "danger");
      }
      break;

    default:
      break;
  }
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useAppState() {
  return useSyncExternalStore(subscribe, getState, getState);
}
