import { useEffect, useState, type ReactNode } from "react";
import {
  Boxes,
  Cpu,
  FolderOpen,
  Gauge,
  Layers,
  Play,
  Sparkles,
  TriangleAlert,
  Wand2,
  Zap,
} from "lucide-react";
import { Button, Card, SectionTitle, cx } from "../components/ui";
import NumberField from "../components/NumberField";
import { startTraining } from "../lib/api";
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

const DEVICES = [
  { id: "auto", label: "自动", icon: Sparkles, note: "探测显卡，失败退回 CPU" },
  { id: "gpu", label: "GPU", icon: Zap, note: "强制使用显卡" },
  { id: "cpu", label: "CPU", icon: Cpu, note: "兼容性最好，速度最慢" },
] as const;

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
  const [device, setDevice] = useState<string>(defaultDevice);
  const [epochs, setEpochs] = useState("100");
  const [batch, setBatch] = useState("8");
  const [imgsz, setImgsz] = useState("640");
  /** Set once the user edits the resolution, after which task switches stop overwriting it. */
  const [imgszEdited, setImgszEdited] = useState(false);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const selection = resolveSelection(taskId, familyKey, scaleId);
  const model = selection.id;
  const problem = taskProblem(selection.task, dataset);
  const running = status === "running";

  // Classification at 640 is slower and no more accurate, so the default follows the task — but
  // only while the value is still ours. Overwriting a number someone typed is worse than a
  // suboptimal default.
  const taskImageSize = selection.task.imageSize;
  useEffect(() => {
    if (!imgszEdited) setImgsz(String(taskImageSize));
  }, [taskImageSize, imgszEdited]);

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

  const ready = Boolean(dataset) && paramsValid && !running && problem === null;

  async function start() {
    // `ready` already gates the button; repeating the check keeps any other caller of start() from
    // launching a combination the scanner described as incompatible.
    if (!dataset || !paramsValid || problem !== null) return;
    setStarting(true);
    setError(null);
    try {
      resetRun();
      setState({ status: "running" });
      await startTraining({
        architecture: model,
        // The manifest, not the data root: `path:` may point elsewhere and training needs the yaml.
        dataset: dataset.manifest,
        epochs: epochsN,
        batch: batchN,
        imgsz: imgszN,
        device,
      });
      onStarted();
    } catch (cause) {
      const message = typeof cause === "string" ? cause : String(cause);
      setError(message);
      setState({ status: "idle" });
    } finally {
      setStarting(false);
    }
  }

  return (
    <div>
      <h1 className="text-[26px] font-extrabold tracking-tight">训练</h1>
      <p className="mt-2 text-[14px] font-medium text-ink-soft">
        从零训练一个视觉模型，不需要预训练权重。
      </p>

      <Card className="mt-8" enter={0}>
        <SectionTitle
          icon={Layers}
          title="模型"
          hint="任务决定输出形态，家族决定架构，规模决定速度与精度的取舍。"
        />

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
      </Card>

      <Card className="mt-4" enter={1}>
        <SectionTitle
          icon={Gauge}
          title="训练参数"
          hint="不确定就保持默认。拖动字段右侧的箭头可以左右调节。"
        />
        <div className="grid grid-cols-3 gap-4">
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
          key={dataset ? "ready" : "empty"}
          className="flex min-w-0 animate-fade-in items-center gap-2.5"
        >
          {dataset ? (
            <>
              <Wand2 size={15} strokeWidth={2.4} className="shrink-0 text-ink-soft" />
              <span className="text-[12px] font-semibold text-ink-faint">
                将创建
              </span>
              <span className="tnum truncate text-[13px] font-bold text-ink">
                {model} · {epochs} 轮 · batch {batch} · {imgsz}px · {device}
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
                还差一步：选一个数据集
              </span>
              <span className="hidden text-[12px] font-medium text-ink-faint sm:inline">
                训练需要 images + labels
              </span>
            </>
          )}
        </div>

        {dataset ? (
          <Button
            icon={Play}
            size="lg"
            onClick={start}
            loading={starting}
            disabled={!ready}
            title={
              !paramsValid
                ? "参数超出范围，先修正再开始"
                : (problem ?? undefined)
            }
            className="shrink-0"
          >
            {running ? "训练中" : "开始训练"}
          </Button>
        ) : (
          <Button
            icon={FolderOpen}
            size="lg"
            onClick={onGoDataset}
            className="shrink-0"
          >
            去选择数据集
          </Button>
        )}
      </div>
    </div>
  );
}
