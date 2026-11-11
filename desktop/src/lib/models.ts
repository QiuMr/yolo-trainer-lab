import type { LucideIcon } from "lucide-react";
import { ScanSearch, Shapes, Tags } from "lucide-react";
import type { DatasetFormat, DatasetSummary } from "./api";

/**
 * The model catalog, mirroring montgomery's `ModelId::from_str` exactly.
 *
 * The engine can build 63 trainable architectures: 73 ids minus `yolo26*-sem` / `yolo26*-depth`,
 * which are inference-only and rejected before a loss is built. Only 45 are offered here. The
 * other 18 — YOLOv10, YOLO12, YOLOX, YOLOv3-tinyu — remain trainable through the CLI but are
 * deliberately absent from both the picker and the ONNX skeleton bundle. Reasons per family are
 * in `docs/DECISIONS.md` D028; the short version is that none of them is what a user of this
 * tool reaches for, and each one would ship a skeleton nobody downloads.
 *
 * The id is assembled from two parts, which is what makes one control able to cover 45 models:
 *   detect   -> prefix + scale          e.g. yolo26 + n      = yolo26n
 *   segment  -> prefix + scale + "-seg" e.g. yolo26 + n      = yolo26n-seg
 *   classify -> prefix + scale + "-cls" e.g. yolo26 + n      = yolo26n-cls
 * `prefix` carries its own trailing separator, so a family whose id needs a hyphen can still be
 * expressed even though the three bundled families happen to be separator-free.
 */

export type TaskId = "detect" | "segment" | "classify";

export type Task = {
  id: TaskId;
  /** Full name, used where there is room. */
  label: string;
  /** Two characters, for the compact control. */
  short: string;
  icon: LucideIcon;
  note: string;
  /** What the trained model actually emits. This is the honest answer to "what am I making?". */
  output: string;
  /**
   * Dataset layouts this task can consume. Detection and instance segmentation both read YOLO
   * label files (a polygon label is still a YOLO label); classification reads class folders.
   * The engine can also read COCO, but the scanner cannot, so it is not offered here.
   */
  formats: DatasetFormat[];
  /** Default input resolution for this task. Detection wants detail; classification wants the
   *  whole image squashed, and 640 there is both slower and no more accurate. */
  imageSize: number;
  imageSizeHint: string;
};

export const TASKS: Task[] = [
  {
    id: "detect",
    label: "目标检测",
    short: "检测",
    icon: ScanSearch,
    note: "框出每个目标",
    output: "边界框 · 类别 · 置信度",
    formats: ["yolo"],
    imageSize: 640,
    imageSizeHint: "32 的倍数，常用 640",
  },
  {
    id: "segment",
    label: "实例分割",
    short: "分割",
    icon: Shapes,
    note: "勾出每个目标的轮廓",
    output: "实例掩码 · 类别",
    formats: ["yolo"],
    imageSize: 640,
    imageSizeHint: "32 的倍数，常用 640",
  },
  {
    id: "classify",
    label: "图像分类",
    short: "分类",
    icon: Tags,
    note: "判断整图属于哪一类",
    output: "整图类别 · top-5",
    formats: ["imagenet-folders"],
    imageSize: 224,
    imageSizeHint: "32 的倍数，分类常用 224",
  },
];

export type Scale = {
  id: string;
  /** Shown large. One letter, always — every bundled family uses the same ladder. */
  label: string;
  /** Four characters at most: six columns still have to fit on one row. */
  note: string;
};

const N_S_M_L_X: Scale[] = [
  { id: "n", label: "n", note: "最快" },
  { id: "s", label: "s", note: "轻快" },
  { id: "m", label: "m", note: "均衡" },
  { id: "l", label: "l", note: "更准" },
  { id: "x", label: "x", note: "最准" },
];

export type Family = {
  key: string;
  label: string;
  /** Six characters at most on the second line of the chip. */
  note: string;
  /** Includes the trailing hyphen for families whose id needs one. */
  prefix: string;
  tasks: TaskId[];
  scales: Scale[];
};

/** Ordered by how likely a first-time user should pick them, not by release date. */
export const FAMILIES: Family[] = [
  {
    key: "yolo26",
    label: "YOLO26",
    note: "最新一代",
    prefix: "yolo26",
    tasks: ["detect", "segment", "classify"],
    scales: N_S_M_L_X,
  },
  {
    key: "yolo11",
    label: "YOLO11",
    note: "成熟稳定",
    prefix: "yolo11",
    tasks: ["detect", "segment", "classify"],
    scales: N_S_M_L_X,
  },
  {
    key: "yolov8",
    label: "YOLOv8",
    note: "生态最广",
    prefix: "yolov8",
    tasks: ["detect", "segment", "classify"],
    scales: N_S_M_L_X,
  },
];

export function architectureId(family: Family, scale: Scale, task: TaskId): string {
  const suffix = task === "detect" ? "" : task === "segment" ? "-seg" : "-cls";
  return `${family.prefix}${scale.id}${suffix}`;
}

export type Selection = {
  task: Task;
  family: Family;
  scale: Scale;
  id: string;
  /** All families that support the current task. */
  families: Family[];
};

/**
 * Resolve the three stored choices into a valid combination.
 *
 * The component keeps the *last* family and scale the user touched rather than a validated pair,
 * so switching task away and back restores the earlier chip instead of silently resetting to the
 * default. With all three families on the same ladder and present under every task, neither
 * fallback is reachable today; they stay because this function takes raw strings and must return
 * something renderable whatever it is handed, and because a future family with its own rungs
 * (the way YOLOX spelled its smallest one `nano`) would need them again.
 */
export function resolveSelection(
  taskId: TaskId,
  familyKey: string,
  scaleId: string,
): Selection {
  const task = TASKS.find((item) => item.id === taskId) ?? TASKS[0];
  const families = FAMILIES.filter((item) => item.tasks.includes(task.id));
  const family = families.find((item) => item.key === familyKey) ?? families[0];
  const scale = family.scales.find((item) => item.id === scaleId) ?? family.scales[0];
  return { task, family, scale, id: architectureId(family, scale, task.id), families };
}

export const DEFAULT_TASK: TaskId = "detect";
export const DEFAULT_FAMILY = "yolo26";
export const DEFAULT_SCALE = "n";

/**
 * Why the current task cannot train on the current dataset, or `null` when it can.
 *
 * Returning a sentence rather than a boolean keeps the explanation next to the rule: the caller
 * has no way to write a message that stays true if the rule changes.
 */
export function taskProblem(task: Task, dataset: DatasetSummary | null): string | null {
  if (!dataset) {
    return task.id === "classify"
      ? "分类需要按类别分文件夹的数据集：train/猫/、train/狗/…，每个文件夹是一类。"
      : null;
  }
  if (task.formats.includes(dataset.format)) return null;
  if (dataset.format === "imagenet-folders") {
    return "当前数据集按类别分文件夹，没有边界框标注。训练检测或分割模型需要 images/ 与 labels/ 结构。";
  }
  return "当前数据集是 YOLO 标签格式，没有「整图属于哪一类」的信息。分类需要把图片按类别分文件夹，或改选检测 / 分割。";
}
