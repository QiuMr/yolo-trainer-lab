import { useEffect, useState, type ReactNode } from "react";
import {
  Boxes,
  CircleCheck,
  CopyPlus,
  Cpu,
  Download,
  FolderOpen,
  Gauge,
  History,
  Layers,
  Play,
  Plus,
  RefreshCw,
  Sparkles,
  TriangleAlert,
  Wand2,
  Zap,
} from "lucide-react";
import { Button, Card, SectionTitle, cx } from "../components/ui";
import NumberField from "../components/NumberField";
import {
  downloadPretrained,
  onPretrainedProgress,
  pickFolder,
  pickModelFile,
  pretrainedCatalog,
  probeDevice,
  startTraining,
  type DeviceReport,
  type PretrainedProgress,
  type PretrainedWeight,
  type TrainInit,
} from "../lib/api";
import {
  DEFAULT_FAMILY,
  DEFAULT_SCALE,
  DEFAULT_TASK,
  TASKS,
  resolveSelection,
  taskProblem,
  type TaskId,
} from "../lib/models";
import { resetRun, setState, useAppState } from "../lib/store";

/** Human label for a backend id. `auto` says so rather than guessing: the runtime picks a backend
 *  and does not report which one, and naming one would be a false statement about the user's own
 *  machine. */
const BACKEND_LABEL: Record<string, string> = {
  vulkan: "Vulkan",
  dx12: "DX12",
  gl: "OpenGL",
  cpu: "CPU",
  auto: "后端由运行时选择",
};

/** `AMD Radeon(TM) Graphics (spirv_4098_5688)` → `AMD Radeon(TM) Graphics`. The parenthesised part
 *  is a compilation fingerprint — noise in a status row, useful in a tooltip. */
function adapterName(adapter: string): string {
  return adapter.replace(/\s*\([^()]*\)\s*$/, "");
}

const DEVICES = [
  { id: "auto", label: "自动", icon: Sparkles, note: "探测显卡，失败退回 CPU" },
  { id: "gpu", label: "GPU", icon: Zap, note: "强制使用显卡" },
  { id: "cpu", label: "CPU", icon: Cpu, note: "兼容性最好，速度最慢" },
] as const;

/**
 * Where the weights come from.
 *
 * `pretrained`, `finetune` and `resume` all read a `.bpk` and are still three different things:
 * `pretrained` is the official COCO/ImageNet checkpoint for the architecture you picked (kept by
 * the app, fetched once), `finetune` is a `.bpk` of your own, and `resume` continues a run with its
 * optimizer, EMA and epoch counter. The notes below say what is *kept*, because the file names are
 * identical and the difference is invisible otherwise.
 */
const INIT_MODES = [
  {
    id: "scratch" as const,
    label: "从零开始",
    icon: Plus,
    note: "随机初始化，不用预训练权重",
  },
  {
    id: "pretrained" as const,
    label: "用预训练权重",
    icon: Download,
    note: "官方通用权重，首次使用时下载",
  },
  {
    id: "finetune" as const,
    label: "用已有模型",
    icon: CopyPlus,
    note: "借它的权重，优化器重新开始",
  },
  {
    id: "resume" as const,
    label: "继续上次训练",
    icon: History,
    note: "连轮数、优化器状态一起接着跑",
  },
];

type InitMode = (typeof INIT_MODES)[number]["id"];

/** Last few path segments, for showing a checkpoint without a full 90-character path. */
function tailPath(path: string, segments = 2): string {
  const parts = path.split(/[\\/]+/).filter(Boolean);
  return parts.slice(-segments).join("/");
}

/** Weights run from 5 to 130 MB; a raw byte count is unreadable and a round number would hide the
 *  difference between "one second" and "a minute" on a slow link. */
function megabytes(bytes: number): string {
  return `${(bytes / 1048576).toFixed(1)} MB`;
}

/** One labelled row inside the model card. The label is a caption, not a heading — it should
 *  read as a dimension of the same choice, which is why it is small and uppercase rather than
 *  competing with the section title. */
function ChoiceRow({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <div className="mt-5 first:mt-0">
      <div className="mb-2 flex items-baseline gap-2">
        <span className="text-[11px] font-bold tracking-wide text-ink-faint uppercase">
          {label}
        </span>
        {hint ? (
          <span className="truncate text-[11px] font-medium text-ink-faint/80">{hint}</span>
        ) : null}
      </div>
      {children}
    </div>
  );
}

export default function TrainPage({
  defaultDevice = "auto",
  onStarted,
  onGoDataset,
}: {
  /** The user's default from Settings; the page still lets them override it for this run. */
  defaultDevice?: string;
  onStarted: () => void;
  onGoDataset: () => void;
}) {
  const { dataset, status } = useAppState();
  // Three stored choices, not one validated model id. Storing the raw picks lets a user switch
  // task away and back and find their earlier family still selected; `resolveSelection` maps
  // them onto something that actually exists (YOLOX has no "n" rung, it has "nano").
  const [taskId, setTaskId] = useState<TaskId>(DEFAULT_TASK);
  const [familyKey, setFamilyKey] = useState(DEFAULT_FAMILY);
  const [scaleId, setScaleId] = useState(DEFAULT_SCALE);
  const [initMode, setInitMode] = useState<InitMode>("scratch");
  /** Finetune source: a finished `.bpk`. */
  const [modelPath, setModelPath] = useState<string | null>(null);
  /** Resume source: a `checkpoints/last` directory, not a `.bpk`. */
  const [checkpointPath, setCheckpointPath] = useState<string | null>(null);
  const [device, setDevice] = useState<string>(defaultDevice);
  /** What the engine said about the selected device, and whether the question is in flight. */
  const [probe, setProbe] = useState<DeviceReport | null>(null);
  const [probing, setProbing] = useState(false);
  const [epochs, setEpochs] = useState("100");
  const [batch, setBatch] = useState("8");
  const [imgsz, setImgsz] = useState("640");
  /** Set once the user edits the resolution, after which task switches stop overwriting it. */
  const [imgszEdited, setImgszEdited] = useState(false);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** What the build can fetch, annotated with what is already on disk. */
  const [weights, setWeights] = useState<PretrainedWeight[]>([]);
  /** Live byte counter while a weight downloads; null when nothing is in flight. */
  const [progress, setProgress] = useState<PretrainedProgress | null>(null);
  /** True from "start pressed" until the weight is on disk and training actually starts. */
  const [preparing, setPreparing] = useState(false);

  const selection = resolveSelection(taskId, familyKey, scaleId);
  const model = selection.id;
  const problem = taskProblem(selection.task, dataset);
  const running = status === "running";
  /** The pretrained entry for the architecture currently selected, if the build offers one. */
  const weight = weights.find((item) => item.arch === model) ?? null;

  // The catalog is a few `stat`s, so it is read on mount and again after a download rather than
  // cached across sessions: a file the user deleted by hand must not stay "ready" in the UI.
  useEffect(() => {
    let alive = true;
    void pretrainedCatalog().then((list) => {
      if (alive) setWeights(list);
    });
    const stop = onPretrainedProgress((event) => {
      setProgress(event.phase === "done" ? null : event);
    });
    return () => {
      alive = false;
      void stop.then((off) => off());
    };
  }, []);

  // Classification at 640 is slower and no more accurate, so the default follows the task — but
  // only while the value is still ours. Overwriting a number someone typed is worse than a
  // suboptimal default.
  const taskImageSize = selection.task.imageSize;
  useEffect(() => {
    if (!imgszEdited) setImgsz(String(taskImageSize));
  }, [taskImageSize, imgszEdited]);

  // Ask the engine whether the selected device can actually compute, before a run is committed to
  // it. Re-asked per device, cached in the shell afterwards: on a broken driver the probe costs the
  // full timeout to say no, and a page that re-asked on every render would stack those up.
  useEffect(() => {
    let alive = true;
    setProbing(true);
    void probeDevice(device)
      .then((report) => {
        if (alive) setProbe(report);
      })
      .catch(() => {
        // An unreachable engine is already reported when training starts; a failed pre-flight must
        // not turn into a page-level error the user cannot dismiss.
        if (alive) setProbe(null);
      })
      .finally(() => {
        if (alive) setProbing(false);
      });
    return () => {
      alive = false;
    };
  }, [device]);

  // Validation lives here, not in each field: the start button is the only thing that
  // actually needs a yes/no, and a field cannot know whether a half-typed value matters.
  //
  // `Number.isInteger` is not redundant. The shell takes `u32` for epochs/batch/imgsz, so a "1.5"
  // passes every range check here and then fails in serde with a type error the user cannot read
  // ("invalid type: floating point 1.5, expected u32"). Range alone is not the contract.
  const epochsN = Number(epochs);
  const batchN = Number(batch);
  const imgszN = imgsz.trim() === "" ? 640 : Number(imgsz);
  const paramsValid =
    Number.isInteger(epochsN) &&
    epochsN >= 1 &&
    epochsN <= 1000 &&
    Number.isInteger(batchN) &&
    batchN >= 1 &&
    batchN <= 128 &&
    Number.isInteger(imgszN) &&
    imgszN >= 64 &&
    imgszN <= 2048 &&
    imgszN % 32 === 0;

  const ready =
    !running &&
    !preparing &&
    (initMode === "scratch"
      ? Boolean(dataset) && paramsValid && problem === null
      : initMode === "pretrained"
        ? Boolean(dataset) && paramsValid && problem === null && weight !== null
        : initMode === "finetune"
          ? Boolean(dataset) && paramsValid && Boolean(modelPath)
          : // Resume needs neither a dataset nor parameters: the checkpoint carries all three of
            // dataset, batch size and epoch budget.
            Boolean(checkpointPath));

  /** What the user still has to do, or null when nothing is missing. Drives the action bar; keeps
   *  the modes from each needing their own copy of "have I got everything". */
  const missing =
    initMode === "resume"
      ? checkpointPath
        ? null
        : "选一个训练检查点"
      : !dataset
        ? "选一个数据集"
        : initMode === "finetune" && !modelPath
          ? "选一个已有模型"
          : initMode === "pretrained" && !weight
            ? "这个架构没有预训练权重"
            : null;

  async function start() {
    // `ready` already gates the button; repeating the check keeps any other caller of start() from
    // launching a combination the scanner described as incompatible.
    if (!ready) return;
    setStarting(true);
    setError(null);
    try {
      let init: TrainInit;
      if (initMode === "pretrained") {
        if (!weight) return;
        // Fetch *before* resetting the run: a download that fails must leave the previous run's
        // monitor intact rather than blanking it and then reporting an error, and the engine
        // cannot start against a path that does not exist yet. When the file is already on disk
        // this returns immediately — the shell verifies the digest either way.
        setPreparing(true);
        const resolved = await downloadPretrained(model);
        setWeights(await pretrainedCatalog());
        init = { mode: "finetune", model: resolved };
      } else if (initMode === "scratch") {
        init = { mode: "scratch", architecture: model };
      } else if (initMode === "finetune" && modelPath) {
        init = { mode: "finetune", model: modelPath };
      } else {
        init = { mode: "resume", checkpoint: checkpointPath ?? "" };
      }
      resetRun();
      // The task rides along so the monitor can name the validation number: mAP for detection and
      // segmentation, accuracy for classification.
      setState({ status: "running", task: taskId });
      await startTraining({
        init,
        // Resume reads its dataset from the checkpoint; sending one would be ignored at best and
        // misleading at worst (the engine rejects `--data` alongside `--resume`).
        dataset: initMode === "resume" ? "" : (dataset?.manifest ?? ""),
        epochs: epochsN,
        batch: batchN,
        imgsz: initMode === "resume" ? null : imgszN,
        device,
      });
      onStarted();
    } catch (cause) {
      const message = typeof cause === "string" ? cause : String(cause);
      setError(message);
      setState({ status: "idle" });
    } finally {
      setPreparing(false);
      setStarting(false);
    }
  }

  return (
    <div>
      <h1 className="text-[26px] font-extrabold tracking-tight">训练</h1>
      <p className="mt-2 text-[14px] font-medium text-ink-soft">
        从零训练、接着预训练权重微调，或者继续之前的结果。
      </p>

      <Card className="mt-8" enter={0}>
        <SectionTitle
          icon={Layers}
          title="模型"
          hint="先决定权重从哪来，再决定架构长什么样。"
        />

        <ChoiceRow label="从哪开始">
          <div className="grid grid-cols-4 gap-2">
            {INIT_MODES.map((item) => {
              const Icon = item.icon;
              const active = initMode === item.id;
              return (
                <button
                  key={item.id}
                  onClick={() => setInitMode(item.id)}
                  aria-pressed={active}
                  data-init-mode={item.id}
                  className={cx(
                    "press flex items-start gap-2.5 rounded-[10px] border px-3 py-2.5 text-left",
                    "transition-[background-color,border-color,color] duration-120 ease-out",
                    active
                      ? "border-ink bg-ink text-canvas"
                      : "border-line-strong bg-canvas hover:border-ink/40 hover:bg-surface",
                  )}
                >
                  <Icon size={15} strokeWidth={2.4} className="mt-0.5 shrink-0" />
                  <span className="min-w-0">
                    <span className="block text-[13px] leading-tight font-bold">
                      {item.label}
                    </span>
                    <span
                      className={cx(
                        "mt-1 block text-[10.5px] leading-snug font-semibold",
                        active ? "text-canvas/65" : "text-ink-faint",
                      )}
                    >
                      {item.note}
                    </span>
                  </span>
                </button>
              );
            })}
          </div>
        </ChoiceRow>

        {initMode === "scratch" || initMode === "pretrained" ? (
          <>
            <ChoiceRow label="任务">
          <div className="grid grid-cols-3 gap-2">
            {TASKS.map((item) => {
              const Icon = item.icon;
              const active = selection.task.id === item.id;
              return (
                <button
                  key={item.id}
                  onClick={() => setTaskId(item.id)}
                  aria-pressed={active}
                  data-model-task={item.id}
                  className={cx(
                    "press flex items-center gap-2.5 rounded-[10px] border px-3 py-2.5 text-left",
                    "transition-[background-color,border-color,color] duration-120 ease-out",
                    active
                      ? "border-ink bg-ink text-canvas"
                      : "border-line-strong bg-canvas hover:border-ink/40 hover:bg-surface",
                  )}
                >
                  <Icon size={15} strokeWidth={2.4} className="shrink-0" />
                  <span className="min-w-0">
                    <span className="block text-[13px] leading-tight font-bold">
                      {item.short}
                    </span>
                    <span
                      className={cx(
                        "mt-1 block truncate text-[10.5px] font-semibold",
                        active ? "text-canvas/65" : "text-ink-faint",
                      )}
                    >
                      {item.note}
                    </span>
                  </span>
                </button>
              );
            })}
          </div>
        </ChoiceRow>

        {/* Re-keyed on the task so the row visibly re-forms when the family set changes —
            otherwise chips silently vanish and the layout just jumps. */}
        <ChoiceRow label="家族" hint={`${selection.families.length} 个可选`}>
          <div
            key={selection.task.id}
            className="grid animate-fade-in gap-2"
            style={{
              gridTemplateColumns: `repeat(${selection.families.length}, minmax(0, 1fr))`,
            }}
          >
            {selection.families.map((item) => {
              const active = selection.family.key === item.key;
              return (
                <button
                  key={item.key}
                  onClick={() => setFamilyKey(item.key)}
                  aria-pressed={active}
                  data-model-family={item.key}
                  className={cx(
                    "press rounded-[10px] border px-1.5 py-2 text-center",
                    "transition-[background-color,border-color,color] duration-120 ease-out",
                    active
                      ? "border-ink bg-ink text-canvas"
                      : "border-line-strong bg-canvas hover:border-ink/40 hover:bg-surface",
                  )}
                >
                  <span className="block truncate text-[12px] leading-tight font-bold">
                    {item.label}
                  </span>
                  <span
                    className={cx(
                      "mt-1 block truncate text-[10px] font-semibold",
                      active ? "text-canvas/60" : "text-ink-faint",
                    )}
                  >
                    {item.note}
                  </span>
                </button>
              );
            })}
          </div>
        </ChoiceRow>

        <ChoiceRow label="规模" hint="左边更快，右边更准">
          <div className="flex gap-1 rounded-[10px] bg-surface-sunken p-1">
            {selection.family.scales.map((item) => {
              const active = selection.scale.id === item.id;
              return (
                <button
                  key={item.id}
                  onClick={() => setScaleId(item.id)}
                  aria-pressed={active}
                  data-model-scale={item.id}
                  className={cx(
                    "press flex-1 rounded-[8px] py-1.5 text-center",
                    "transition-[background-color,color] duration-120 ease-out",
                    active
                      ? "bg-ink text-canvas"
                      : "text-ink-soft hover:bg-canvas hover:text-ink",
                  )}
                >
                  <span className="block text-[14px] leading-tight font-bold">
                    {item.label}
                  </span>
                  <span
                    className={cx(
                      "mt-0.5 block text-[10px] font-semibold",
                      active ? "text-canvas/60" : "text-ink-faint",
                    )}
                  >
                    {item.note}
                  </span>
                </button>
              );
            })}
          </div>
        </ChoiceRow>

        {/* The id stays visible even when the task is blocked: the user should still see the
            artifact name they are composing, and the warning sits under it rather than replacing
            it — a warning that hides the thing it is about is just an error. */}
        <div className="mt-4 flex items-center justify-between gap-3 rounded-[10px] bg-surface px-3.5 py-2.5">
          <div className="flex min-w-0 items-center gap-2">
            <Boxes size={14} strokeWidth={2.4} className="shrink-0 text-ink-faint" />
            <code
              key={model}
              className="animate-fade-in truncate font-mono text-[13px] font-bold tracking-tight"
            >
              {model}
            </code>
          </div>
          <span className="shrink-0 text-[11px] font-semibold text-ink-faint">
            {selection.task.output}
          </span>
        </div>

        {problem === null ? null : (
          <div className="mt-2 flex items-start gap-2.5 rounded-[10px] border border-warn/25 bg-warn/5 px-3.5 py-2.5">
            <TriangleAlert
              size={14}
              strokeWidth={2.4}
              className="mt-0.5 shrink-0 text-warn"
            />
            <span className="text-[11.5px] leading-relaxed font-semibold text-ink-soft">
              {problem}
            </span>
          </div>
        )}

        {/* Says what the run will actually start from. "Pretrained" alone is not enough: the
            user needs to know it already recognises 80 COCO classes (so their 3-class set is a
            narrowing, not a fresh start) and how much the download costs. */}
        {initMode === "pretrained" ? (
          weight === null ? (
            <div className="mt-2 flex items-start gap-2.5 rounded-[10px] border border-warn/25 bg-warn/5 px-3.5 py-2.5">
              <TriangleAlert
                size={14}
                strokeWidth={2.4}
                className="mt-0.5 shrink-0 text-warn"
              />
              <span className="text-[11.5px] leading-relaxed font-semibold text-ink-soft">
                这个架构还没有预训练权重。目前提供的是各家族的 n 档——换成 n，或者用「用已有模型」导入自己的 .bpk。
              </span>
            </div>
          ) : (
            <div className="mt-2 rounded-[10px] bg-surface px-3.5 py-2.5">
              <div className="flex items-center justify-between gap-3">
                <div className="flex min-w-0 items-center gap-2">
                  <Download size={14} strokeWidth={2.4} className="shrink-0 text-ink-faint" />
                  <span className="truncate text-[11.5px] font-semibold text-ink-soft">
                    {weight.dataset} 预训练 · 已有 {weight.classes} 类
                  </span>
                </div>
                <span className="shrink-0 text-[11px] font-bold text-ink-faint">
                  {weight.cached ? "已就绪" : `首次训练前下载 ${megabytes(weight.bytes)}`}
                </span>
              </div>
              {progress && progress.arch === weight.arch ? (
                <div className="mt-2">
                  <div className="h-1.5 overflow-hidden rounded-full bg-surface-sunken">
                    <div
                      className="h-full rounded-full bg-ink transition-[width] duration-200 ease-out"
                      style={{
                        width: `${
                          progress.total > 0
                            ? Math.min(100, Math.round((progress.received / progress.total) * 100))
                            : 0
                        }%`,
                      }}
                    />
                  </div>
                  <span className="mt-1 block text-[10.5px] font-semibold text-ink-faint">
                    {progress.phase === "verify"
                      ? "校验文件完整性…"
                      : `下载中 ${megabytes(progress.received)} / ${megabytes(progress.total)}`}
                  </span>
                </div>
              ) : null}
            </div>
          )
        ) : null}
          </>
        ) : initMode === "finetune" ? (
          <ChoiceRow
            label="已有模型"
            hint=".bpk 文件，训练产物里的 exports/best.bpk"
          >
            <div className="flex items-center gap-3 rounded-[10px] bg-surface px-3.5 py-3">
              <CopyPlus size={15} strokeWidth={2.4} className="shrink-0 text-ink-faint" />
              <code className="min-w-0 flex-1 truncate font-mono text-[12.5px] font-bold">
                {modelPath ?? "还没有选择模型"}
              </code>
              <Button
                variant="secondary"
                icon={FolderOpen}
                size="sm"
                className="shrink-0"
                onClick={() => {
                  void pickModelFile().then((picked) => {
                    if (picked) setModelPath(picked);
                  });
                }}
              >
                选择
              </Button>
            </div>
            <p className="mt-2 text-[11px] leading-relaxed font-medium text-ink-faint">
              它的架构和类别表会沿用，但训练从头计轮——优化器状态不继承。
              想接着上次的进度，请改用「继续上次训练」。
            </p>
          </ChoiceRow>
        ) : (
          <ChoiceRow
            label="训练检查点"
            hint="runs/<名称>/checkpoints/last 目录"
          >
            <div className="flex items-center gap-3 rounded-[10px] bg-surface px-3.5 py-3">
              <History size={15} strokeWidth={2.4} className="shrink-0 text-ink-faint" />
              <code className="min-w-0 flex-1 truncate font-mono text-[12.5px] font-bold">
                {checkpointPath ?? "还没有选择检查点"}
              </code>
              <Button
                variant="secondary"
                icon={FolderOpen}
                size="sm"
                className="shrink-0"
                onClick={() => {
                  void pickFolder("选择 checkpoints/last 目录").then((picked) => {
                    if (picked) setCheckpointPath(picked);
                  });
                }}
              >
                选择
              </Button>
            </div>
            <p className="mt-2 text-[11px] leading-relaxed font-medium text-ink-faint">
              数据集、批大小和总轮数都从检查点里读，下面的参数不会生效。
              目录里必须有 manifest.json、model.bin 和 optimizer.bin。
            </p>
          </ChoiceRow>
        )}
      </Card>

      <Card className="mt-4" enter={1}>
        <SectionTitle
          icon={Gauge}
          title="训练参数"
          hint={
            initMode === "resume"
              ? "续训时轮数、批大小和分辨率都由检查点决定，这里的值不会生效。"
              : "不确定就保持默认。拖动字段右侧的箭头可以左右调节。"
          }
        />
        {/* Dimmed rather than hidden: the checkpoint's values are still worth seeing, and a card
            that silently loses its inputs looks broken. Not a `disabled` prop on each field —
            NumberField's drag handles are not native controls, so pointer-events is what actually
            stops them. */}
        <div
          className={cx(
            "grid grid-cols-3 gap-4 transition-opacity duration-150",
            initMode === "resume" && "pointer-events-none opacity-40",
          )}
        >
          <NumberField
            label="训练轮数"
            hint="轮数越多越充分，也越费时间"
            value={epochs}
            onChange={setEpochs}
            min={1}
            max={1000}
            fallback={100}
            suffix="轮"
          />
          <NumberField
            label="批大小"
            hint="显存不够就调小"
            value={batch}
            onChange={setBatch}
            min={1}
            max={128}
            fallback={8}
          />
          <NumberField
            label="输入分辨率"
            hint={selection.task.imageSizeHint}
            value={imgsz}
            onChange={(value) => {
              setImgszEdited(true);
              setImgsz(value);
            }}
            min={64}
            max={2048}
            step={32}
            fallback={selection.task.imageSize}
            suffix="px"
          />
        </div>
      </Card>

      <Card className="mt-4" enter={2}>
        <SectionTitle
          icon={Zap}
          title="计算设备"
          hint="自动模式会先探测显卡，跑不动就退回 CPU，不会让训练失败。"
        />
        <div className="grid grid-cols-3 gap-3">
          {DEVICES.map((item) => {
            const Icon = item.icon;
            const active = device === item.id;
            return (
              <button
                key={item.id}
                onClick={() => setDevice(item.id)}
                aria-pressed={active}
                className={cx(
                  "press flex items-start gap-3 rounded-[10px] border p-3.5 text-left",
                  "transition-[background-color,border-color,transform] duration-120 ease-out",
                  active
                    ? "border-ink bg-surface"
                    : "border-line-strong bg-canvas hover:border-ink/40 hover:bg-surface",
                )}
              >
                <span
                  className={cx(
                    "flex h-8 w-8 shrink-0 items-center justify-center rounded-[8px]",
                    "transition-colors duration-120",
                    active ? "bg-ink text-canvas" : "bg-surface-sunken text-ink",
                  )}
                >
                  <Icon size={15} strokeWidth={2.5} />
                </span>
                <div className="min-w-0">
                  <div className="text-[13px] font-bold">{item.label}</div>
                  <div className="mt-0.5 text-[11px] leading-snug font-medium text-ink-faint">
                    {item.note}
                  </div>
                </div>
              </button>
            );
          })}
        </div>

        {/* The pre-flight answer. "Will it train" is not "which card is installed": a driver can
            accept a device and then fail the first readback, which is exactly what happens on the
            machine this project was developed on. Saying "无需安装 CUDA" here is worth the pixels —
            it is the question every PyTorch user arrives with. */}
        <div className="mt-3 flex flex-wrap items-center gap-x-2 gap-y-1.5 border-t border-line pt-3">
          {probing ? (
            <span className="text-[12px] font-medium text-ink-faint">
              正在检测这台机器的计算设备…
            </span>
          ) : probe?.ok ? (
            <>
              <CircleCheck size={13} strokeWidth={2.8} className="shrink-0 text-ok" />
              <span
                className="text-[12px] font-semibold text-ink"
                title={probe.adapter ?? undefined}
              >
                {probe.adapter ? adapterName(probe.adapter) : "图形设备"}
              </span>
              <span className="text-[12px] font-medium text-ink-faint">
                · {BACKEND_LABEL[probe.backend ?? "auto"] ?? probe.backend}
                {probe.backend === "cpu" ? "" : " · 无需安装 CUDA"}
              </span>
            </>
          ) : probe ? (
            <>
              <TriangleAlert size={13} strokeWidth={2.6} className="shrink-0 text-danger" />
              <span className="text-[12px] font-semibold text-ink">
                {device === "cpu" ? "CPU 不可用" : "显卡跑不起来"}
              </span>
              <span
                className="min-w-0 flex-1 truncate text-[12px] font-medium text-ink-faint"
                title={probe.reason ?? undefined}
              >
                {probe.reason ?? "引擎没有给出原因"}
                {device === "auto" ? "（自动模式会退回 CPU）" : ""}
              </span>
            </>
          ) : null}
          <button
            type="button"
            disabled={probing}
            onClick={() => {
              setProbing(true);
              void probeDevice(device, true)
                .then(setProbe)
                .catch(() => setProbe(null))
                .finally(() => setProbing(false));
            }}
            className="press ml-auto flex shrink-0 items-center gap-1 rounded-[7px] px-2 py-1 text-[11.5px] font-bold text-ink-faint transition-colors duration-120 hover:bg-surface-sunken hover:text-ink disabled:opacity-50"
          >
            <RefreshCw
              size={11}
              strokeWidth={2.6}
              className={probing ? "animate-spin" : undefined}
            />
            重新检测
          </button>
        </div>
      </Card>

      {error ? (
        <div className="mt-6 flex items-start gap-3 rounded-[14px] border border-danger/25 bg-danger/5 px-5 py-4">
          <TriangleAlert
            size={17}
            strokeWidth={2.4}
            className="mt-0.5 shrink-0 text-danger"
          />
          <div className="text-[13px] leading-relaxed font-medium text-ink-soft">
            {error}
          </div>
        </div>
      ) : null}

      {/* Action bar. It stays a black-button bar in *both* states on purpose: while blocked the
          primary action is "go pick a dataset", not a greyed-out button plus an orange sentence.
          A disabled control is not a call to action.

          Deliberately not sticky. It was tried: on an 808px viewport the model card alone is
          ~450px, so a pinned bar sits on top of the training-parameter inputs while the user is
          editing them. Scrolling to the end is also the natural order of the page — pick a model,
          tune, pick a device, start — so the button belongs last. */}
      <div
        className="mt-6 flex animate-card-in items-center justify-between gap-4 rounded-[14px] border border-line bg-canvas px-5 py-4"
        style={{ animationDelay: "110ms" }}
      >
        <div
          key={missing ?? initMode}
          className="flex min-w-0 animate-fade-in items-center gap-2.5"
        >
          {missing === null ? (
            <>
              {initMode === "resume" ? (
                <History size={15} strokeWidth={2.4} className="shrink-0 text-ink-soft" />
              ) : (
                <Wand2 size={15} strokeWidth={2.4} className="shrink-0 text-ink-soft" />
              )}
              <span className="text-[12px] font-semibold text-ink-faint">
                {initMode === "resume"
                  ? "将续训"
                  : initMode === "pretrained"
                    ? "将微调"
                    : "将创建"}
              </span>
              <span className="tnum truncate text-[13px] font-bold text-ink">
                {initMode === "resume"
                  ? tailPath(checkpointPath ?? "")
                  : initMode === "pretrained" && weight
                    ? `${model} ← ${weight.dataset} 预训练 · ${epochs} 轮 · batch ${batch} · ${imgsz}px · ${device}`
                    : `${model} · ${epochs} 轮 · batch ${batch} · ${imgsz}px · ${device}`}
              </span>
            </>
          ) : (
            <>
              <TriangleAlert
                size={15}
                strokeWidth={2.4}
                className="shrink-0 text-warn"
              />
              <span className="text-[13px] font-semibold text-ink">
                还差一步：{missing}
              </span>
              <span className="hidden text-[12px] font-medium text-ink-faint sm:inline">
                {initMode === "resume"
                  ? "指向 checkpoints/last 目录"
                  : "训练需要 images + labels"}
              </span>
            </>
          )}
        </div>

        {missing === null ? (
          <Button
            icon={Play}
            size="lg"
            onClick={start}
            loading={starting}
            disabled={!ready}
            title={
              initMode === "resume"
                ? undefined
                : !paramsValid
                  ? "参数超出范围，先修正再开始"
                  : (problem ?? undefined)
            }
            className="shrink-0"
          >
            {running
              ? "训练中"
              : initMode === "resume"
                ? "继续训练"
                : "开始训练"}
          </Button>
        ) : (
          <Button
            icon={FolderOpen}
            size="lg"
            onClick={
              initMode === "resume"
                ? () => {
                    void pickFolder("选择 checkpoints/last 目录").then((picked) => {
                      if (picked) setCheckpointPath(picked);
                    });
                  }
                : initMode === "finetune" && dataset
                  ? () => {
                      void pickModelFile().then((picked) => {
                        if (picked) setModelPath(picked);
                      });
                    }
                  : onGoDataset
            }
            className="shrink-0"
          >
            {initMode === "resume"
              ? "选择检查点"
              : initMode === "finetune" && dataset
                ? "选择模型"
                : "去选择数据集"}
          </Button>
        )}
      </div>
    </div>
  );
}
