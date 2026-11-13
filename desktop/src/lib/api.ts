import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * Engine bridge.
 *
 * Everything the UI knows about training comes through here, and every field is defined by
 * docs/API.md — the engine's contract, not a shape invented on this side. In a plain browser
 * (design preview without the Tauri runtime) the functions fall back to sample data so the layout
 * stays inspectable.
 */

export const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Layouts the scanner understands. COCO exists in the engine but not here — see dataset.rs. */
export type DatasetFormat = "yolo" | "imagenet-folders";

export type DatasetSummary = {
  /** Data root the splits resolve against (`path:` in the yaml, or the yaml's own folder). */
  root: string;
  /** Absolute path of the `dataset.yaml`. Training needs this, not `root`: when `path:` points
   *  somewhere else, there is no yaml at the data root. */
  manifest: string;
  classes: string[];
  train_images: number;
  val_images: number;
  missing_labels: number;
  annotated: number;
  /** Per-class training image count, only for classification (empty for YOLO). */
  class_counts: number[];
  format: DatasetFormat;
  warning: string | null;
};

/**
 * How a run starts. `montgomery train` accepts exactly one of these, so the shell models it as a
 * choice rather than three optional fields that can contradict each other.
 *
 * `finetune` and `resume` are not interchangeable, and the difference is invisible in the UI unless
 * it is stated: a `.bpk` starts a **new** run (optimizer, EMA and epoch counter all restart), while
 * a checkpoint continues them. Picking the wrong one looks like it worked.
 */
export type TrainInit =
  | { mode: "scratch"; architecture: string }
  | { mode: "finetune"; model: string }
  | { mode: "resume"; checkpoint: string };

export type TrainRequest = {
  init: TrainInit;
  /** Manifest for scratch/finetune; empty for resume, which takes the dataset from the checkpoint. */
  dataset: string;
  epochs: number;
  batch: number;
  imgsz: number | null;
  device: string;
};

/** Events forwarded from the engine's stdout, plus `exit` which the shell adds. */
export type EngineEvent =
  | {
      type: "start";
      run: string;
      backend: string;
      adapter: string;
      epochs: number;
      batch_size: number;
      image_size: number | null;
      class_names: string[];
    }
  | {
      type: "epoch";
      epoch: number;
      epochs: number;
      loss: number;
      fitness: number | null;
      elapsed_s: number;
    }
  | { type: "end"; run: string; best: string | null; last: string | null }
  | { type: "error"; code: string; message: string; hint: string | null }
  | { type: "exit"; code: number | null };

/** Settings as stored. Mirrors `settings::Settings` on the Rust side. */
export type Settings = {
  engine_path: string | null;
  default_device: string;
  output_dir: string | null;
  /** Local HTTP control service for AI clients. Mirrors `settings::Settings`. */
  control_enabled: boolean;
  /** `0` asks the OS for a free port; the actual one comes back in `SettingsView.control`. */
  control_port: number;
  /** `wecom` | `pushplus` | `generic`. Where a failure summary is pushed. */
  feedback_kind: string;
  /** Empty means reports are written locally and never leave the machine. */
  feedback_webhook: string;
};

/**
 * Live state of the control service. Mirrors `runtime::ControlInfo`.
 *
 * `port` is the port it actually got, which is not necessarily `control_port` in settings: if the
 * requested one is taken the service falls back to an OS-assigned port rather than failing.
 */
export type ControlInfo = {
  enabled: boolean;
  running: boolean;
  port: number;
  token: string;
  base_url: string;
  error: string | null;
};

/**
 * Settings as rendered. The shell resolves the paths; the UI never guesses where things live.
 * Mirrors `SettingsView` in src-tauri/src/lib.rs.
 *
 * Written out rather than intersected with [`Settings`] because the two disagree on purpose:
 * a stored `output_dir` of `null` means "use the default", while the view always carries a
 * usable absolute path.
 */
export type SettingsView = {
  engine_path: string | null;
  default_device: string;
  engine_resolved: string | null;
  engine_error: string | null;
  /** Always absolute, ready to display. */
  output_dir: string;
  output_dir_is_default: boolean;
  config_dir: string;
  engine_running: boolean;
  control: ControlInfo;
  /** Where a failure summary is pushed; empty means "local report only". */
  feedback_kind: string;
  feedback_webhook: string;
  /** Folder holding the local reports, so the UI can show where they are. */
  diagnostics_dir: string;
};

const SAMPLE_SETTINGS: SettingsView = {
  engine_path: null,
  engine_resolved: "E:\\工作区\\yolo-trainer-lab\\artifacts\\target-dist\\release\\montgomery.exe",
  engine_error: null,
  default_device: "auto",
  output_dir: "C:\\Users\\you\\AppData\\Roaming\\com.yolotrainer.desktop\\runs",
  output_dir_is_default: true,
  config_dir: "C:\\Users\\you\\AppData\\Roaming\\com.yolotrainer.desktop",
  engine_running: false,
  control: {
    enabled: true,
    running: true,
    port: 47821,
    token: "a1b2c3d4e5f60718293a4b5c6d7e8f90",
    base_url: "http://127.0.0.1:47821",
    error: null,
  },
  feedback_kind: "wecom",
  feedback_webhook: "",
  diagnostics_dir: "C:\\Users\\you\\AppData\\Roaming\\com.yolotrainer.desktop\\diagnostics",
};

const SAMPLE_DATASET: DatasetSummary = {
  root: "E:\\工作区\\yolo-trainer-lab\\spikes\\tiny-dataset",
  manifest: "E:\\工作区\\yolo-trainer-lab\\spikes\\tiny-dataset\\dataset.yaml",
  classes: ["dog", "person", "bike"],
  train_images: 4,
  val_images: 2,
  missing_labels: 0,
  annotated: 4,
  class_counts: [],
  format: "yolo",
  warning: null,
};

const SAMPLE_CLASSIFY_DATASET: DatasetSummary = {
  root: "E:\\工作区\\yolo-trainer-lab\\spikes\\tiny-classify",
  manifest: "E:\\工作区\\yolo-trainer-lab\\spikes\\tiny-classify\\dataset.yaml",
  classes: ["circle", "square", "triangle"],
  train_images: 18,
  val_images: 6,
  missing_labels: 0,
  annotated: 18,
  class_counts: [6, 6, 6],
  format: "imagenet-folders",
  warning: null,
};

export async function resolveEngine(): Promise<string | null> {
  if (!isTauri) return null;
  try {
    return await invoke<string>("resolve_engine");
  } catch {
    return null;
  }
}

export async function pickDatasetFolder(): Promise<string | null> {
  if (!isTauri) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const selected = await open({
    directory: true,
    multiple: false,
    title: "选择数据集目录",
  });
  return typeof selected === "string" ? selected : null;
}

/** One image, for the post-training check. */
export async function pickImageFile(): Promise<string | null> {
  if (!isTauri) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const selected = await open({
    directory: false,
    multiple: false,
    title: "选择一张图片",
    filters: [{ name: "图片", extensions: ["jpg", "jpeg", "png", "webp"] }],
  });
  return typeof selected === "string" ? selected : null;
}

/** Mirrors `predict.rs`. `output` is absent for classification, which renders no image. */
export type PredictResult = {
  model: string;
  source: string;
  output: string | null;
  prediction: Record<string, unknown>;
};

export type PredictRequest = {
  /** Omitted means "the best model from the last run". */
  model?: string | null;
  source: string;
  output?: string | null;
  confidence?: number | null;
  masks?: boolean;
};

/** Plausible output for the browser preview, so the inference card can be reviewed without a
 *  shell around it. Never reaches the real app: `predictImage` returns this only when `!isTauri`. */
const SAMPLE_PREDICTION: PredictResult = {
  model: "…\\runs\\2026-11-10-19-20\\exports\\best.bpk",
  source: "…\\tiny-dataset\\images\\val\\img1.jpg",
  output: null,
  prediction: {
    task: "detection",
    coordinate_format: "xyxy",
    detections: [
      { class_name: "person", confidence: 0.86, xmin: 120, ymin: 40, xmax: 260, ymax: 300 },
      { class_name: "dog", confidence: 0.71, xmin: 300, ymin: 90, xmax: 430, ymax: 280 },
      { class_name: "bike", confidence: 0.54, xmin: 40, ymin: 210, xmax: 190, ymax: 330 },
    ],
  },
};

export async function predictImage(
  request: PredictRequest,
): Promise<PredictResult> {
  if (!isTauri) return { ...SAMPLE_PREDICTION, source: request.source };
  return await invoke<PredictResult>("predict_image", { request });
}

/** Result of an offline ONNX export: the file the user can take elsewhere. */
export type OnnxExport = {
  output: string;
  classes: number;
  tensors: number;
  bytes: number;
};

/**
 * Write the current model as ONNX. No Python and no internet: the installer ships a weight-free
 * graph per architecture and the engine fills it from the checkpoint.
 *
 * `model` is omitted for "the best model of the most recent run", which is what the button means.
 */
export async function exportOnnx(model?: string): Promise<OnnxExport> {
  if (!isTauri) {
    // Preview: the shell is what writes the file, so there is nothing honest to return but a
    // stand-in shape. Keeping it explicit stops the button from looking broken in the browser.
    return { output: "…\\onnx\\2026-11-10-19-20-best.onnx", classes: 3, tensors: 127, bytes: 12_000_000 };
  }
  return await invoke<OnnxExport>("export_onnx", { request: { model: model ?? null } });
}

/**
 * A pretrained weight the build can fetch.
 *
 * The installer ships the *list* only — a few kilobytes of name, size and digest. The file itself
 * comes from a GitHub Release the first time someone trains with it. Bundling was measured and
 * rejected: a `.bpk` compresses at 1.11x, so even the nine n-rung weights would have taken the
 * download from 29 MB to 80 MB, and all fifteen detection models are 374 MB.
 */
export type PretrainedWeight = {
  arch: string;
  family: string;
  scale: string;
  task: string;
  bytes: number;
  classes: number;
  /** What it was pretrained on: `COCO` for detection/segmentation, `ImageNet` for classification. */
  dataset: string;
  /** Already on disk — nothing to download. */
  cached: boolean;
  path: string | null;
};

export type PretrainedProgress = {
  arch: string;
  phase: "download" | "verify" | "done";
  received: number;
  total: number;
};

export async function pretrainedCatalog(): Promise<PretrainedWeight[]> {
  if (!isTauri) return [];
  return await invoke<PretrainedWeight[]>("pretrained_catalog");
}

/**
 * Resolve one weight to a local path, downloading it if needed.
 *
 * Rejects with a message the user can act on: a blocked network names the file and the exact
 * directory to drop it in, so the feature still works offline.
 */
export async function downloadPretrained(arch: string): Promise<string> {
  if (!isTauri) throw new Error("当前是界面预览模式，无法下载预训练权重。");
  return await invoke<string>("download_pretrained", { arch });
}

export async function onPretrainedProgress(
  handler: (progress: PretrainedProgress) => void,
): Promise<UnlistenFn> {
  if (!isTauri) return () => {};
  return await listen<PretrainedProgress>("pretrained-progress", (event) =>
    handler(event.payload),
  );
}

export async function scanDataset(path: string): Promise<DatasetSummary> {
  if (!isTauri) {
    // Preview mode: return a sample so the page can be judged without an engine. The path picks
    // which layout to stand in for — the two render different cards.
    const sample = path.includes("classify")
      ? SAMPLE_CLASSIFY_DATASET
      : SAMPLE_DATASET;
    return { ...sample, root: path || sample.root, manifest: path ? `${path}\\dataset.yaml` : sample.manifest };
  }
  return await invoke<DatasetSummary>("scan_dataset", { path });
}

export async function getSettings(): Promise<SettingsView> {
  if (!isTauri) return SAMPLE_SETTINGS;
  return await invoke<SettingsView>("get_settings");
}

/**
 * Open the folder holding the local failure reports.
 *
 * The shell creates it first: a build that has never failed has no `diagnostics/` directory, and
 * asking Windows to open a path that does not exist shows an error instead of an empty folder.
 */
export async function openDiagnosticsFolder(): Promise<void> {
  if (!isTauri) return;
  await invoke("open_diagnostics_folder");
}

/** Returns the stored settings as the shell sees them, so the UI can show what actually landed. */
export async function saveSettings(
  settings: Settings,
): Promise<SettingsView> {
  if (!isTauri) {
    // Preview: echo the edit back without pretending to know the shell's resolved paths.
    return {
      ...SAMPLE_SETTINGS,
      engine_path: settings.engine_path,
      default_device: settings.default_device,
      output_dir: settings.output_dir ?? SAMPLE_SETTINGS.output_dir,
      output_dir_is_default: settings.output_dir === null,
      // Preview has no listener, so the toggle only has to change what is rendered.
      control: { ...SAMPLE_SETTINGS.control, enabled: settings.control_enabled },
      feedback_kind: settings.feedback_kind,
      feedback_webhook: settings.feedback_webhook,
    };
  }
  return await invoke<SettingsView>("save_settings", { settings });
}

/** Pick a folder for a given purpose; the dialog title is the only difference between callers. */
export async function pickFolder(title: string): Promise<string | null> {
  if (!isTauri) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const selected = await open({ directory: true, multiple: false, title });
  return typeof selected === "string" ? selected : null;
}

export async function pickEngineFile(): Promise<string | null> {  if (!isTauri) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const selected = await open({
    multiple: false,
    title: "选择 montgomery 引擎",
    filters: [{ name: "可执行文件", extensions: ["exe"] }],
  });
  return typeof selected === "string" ? selected : null;
}

/** A finished model, used as the starting point for a new run (finetune). */
export async function pickModelFile(): Promise<string | null> {
  if (!isTauri) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const selected = await open({
    multiple: false,
    title: "选择一个 .bpk 模型",
    filters: [{ name: "Montgomery 模型", extensions: ["bpk"] }],
  });
  return typeof selected === "string" ? selected : null;
}

export async function startTraining(request: TrainRequest): Promise<void> {
  if (!isTauri) {
    throw new Error("当前是界面预览模式，未连接训练引擎。");
  }
  await invoke("start_training", { request });
}

export async function stopTraining(): Promise<void> {
  if (!isTauri) return;
  await invoke("stop_training");
}

export async function onEngineEvent(
  handler: (event: EngineEvent) => void,
): Promise<UnlistenFn> {
  if (!isTauri) return () => {};
  return await listen<EngineEvent>("engine-event", (payload) =>
    handler(payload.payload),
  );
}

export async function onEngineLog(
  handler: (line: string) => void,
): Promise<UnlistenFn> {
  if (!isTauri) return () => {};
  return await listen<{ line: string }>("engine-log", (payload) =>
    handler(payload.payload.line),
  );
}

/* ── Automatic updates ────────────────────────────────────────────────────────
   The feed lives at GitHub Releases; the Rust side owns the URL, the signature check and the
   installer. The UI only ever sees a small `UpdateInfo` — no URLs, no signatures, no raw JSON.
   ───────────────────────────────────────────────────────────────────────────── */

export type UpdateInfo = {
  current: string;
  /** Equals `current` when there is nothing to install, so the chip is never blank. */
  latest: string;
  has_update: boolean;
  notes: string;
  /** A filled string means "could not check" — distinct from "no update". */
  error: string | null;
};

export type UpdateProgress = {
  phase: "downloading" | "installing";
  /** `-1` when the server did not send Content-Length. */
  percent: number;
};

/** Browser preview has no installed binary, so there is deliberately no fake version number. */
export async function getAppVersion(): Promise<string> {
  if (!isTauri) return "预览模式";
  return await invoke<string>("app_version");
}

export async function checkUpdate(): Promise<UpdateInfo> {
  if (!isTauri) {
    return {
      current: "预览模式",
      latest: "预览模式",
      has_update: false,
      notes: "",
      error: "浏览器预览没有 Tauri 运行时，更新检查不可用。",
    };
  }
  return await invoke<UpdateInfo>("check_update");
}

/**
 * On Windows a successful install **exits the process**, so this promise normally never
 * settles. Callers must treat the `installing` progress event as the terminal state.
 */
export async function installUpdate(): Promise<void> {
  if (!isTauri) throw new Error("预览模式无法安装更新。");
  await invoke<void>("install_update");
}

export async function onUpdateProgress(
  handler: (progress: UpdateProgress) => void,
): Promise<UnlistenFn> {
  if (!isTauri) return () => {};
  // Must match `updater::PROGRESS_EVENT` in src-tauri/src/updater.rs. It is a plain string
  // because there is no shared constant across the IPC boundary; docs/API.md is the contract.
  return await listen<UpdateProgress>("update-progress", (payload) =>
    handler(payload.payload),
  );
}

/** Human-readable summary of an error code, per docs/API.md. */
export const ERROR_COPY: Record<string, { title: string; action: string }> = {
  GPU_LOST: {
    title: "显卡设备丢失",
    action: "更新显卡驱动，或改用 CPU 重新训练。",
  },
  GPU_FALLBACK: {
    title: "已回退到 CPU",
    action: "显卡不可用，训练会慢一些，但仍然可以进行。",
  },
  NO_DATASET: {
    title: "数据集有问题",
    action: "检查 dataset.yaml 的路径与划分。",
  },
  BAD_ANNOTATION: {
    title: "标注格式错误",
    action: "每行应为：类别 中心x 中心y 宽 高（0~1）。",
  },
  EMPTY_DATASET: { title: "数据集是空的", action: "检查 train 目录里是否有图片。" },
  CLASS_MISMATCH: {
    title: "类别对不上",
    action: "续训要求类别与原数据集一致。",
  },
  OOM: { title: "显存不足", action: "调小 batch 或分辨率后重试。" },
  INTERNAL: { title: "引擎报错", action: "查看下方日志里的详细原因。" },
};
