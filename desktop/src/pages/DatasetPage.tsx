import { useEffect, useState } from "react";
import {
  ArrowRight,
  CheckCircle2,
  FolderOpen,
  Images,
  Loader2,
  ScanLine,
  Tags,
  TriangleAlert,
} from "lucide-react";
import { Badge, Button, Card, SectionTitle, Stat } from "../components/ui";
import { Progress } from "../components/Motion";
import {
  isTauri,
  pickDatasetFolder,
  scanDataset,
} from "../lib/api";
import { setState, useAppState } from "../lib/store";

export default function DatasetPage() {
  const { dataset } = useAppState();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Design preview has no engine to ask, so it loads a sample immediately. `?demo=classify` loads
  // the classification sample instead, because that layout renders a different card.
  useEffect(() => {
    if (isTauri) return;
    const demo = new URLSearchParams(window.location.search).get("demo");
    void load(
      demo === "classify"
        ? "E:\\工作区\\yolo-trainer-lab\\spikes\\tiny-classify"
        : "E:\\工作区\\yolo-trainer-lab\\spikes\\tiny-dataset",
    );
  }, []);

  async function load(path: string) {
    setBusy(true);
    setError(null);
    try {
      const summary = await scanDataset(path);
      setState({ dataset: summary });
    } catch (cause) {
      // The engine's message is already user-facing; do not wrap it again.
      setError(typeof cause === "string" ? cause : String(cause));
      setState({ dataset: null });
    } finally {
      setBusy(false);
    }
  }

  async function choose() {
    const picked = await pickDatasetFolder();
    if (picked) await load(picked);
  }

  return (
    <div>
      <div className="flex items-start justify-between gap-6">
        <div>
          <h1 className="text-[26px] font-extrabold tracking-tight">数据集</h1>
          <p className="mt-2 text-[14px] font-medium text-ink-soft">
            指向一个数据集目录：YOLO 的 images/ + labels/，或按类别分文件夹的
            train/&lt;类名&gt;/。
          </p>
        </div>
        <div className="flex shrink-0 gap-2">
          {dataset ? (
            <Button
              variant="secondary"
              icon={Loader2}
              loading={busy}
              // Re-scan from the manifest, not the data root: `path:` may point elsewhere, and then
              // there is no dataset.yaml at the root to find.
              onClick={() => load(dataset.manifest)}
            >
              重新扫描
            </Button>
          ) : null}
          <Button icon={FolderOpen} onClick={choose} disabled={busy}>
            选择目录
          </Button>
        </div>
      </div>

      {error ? (
        <div className="mt-6 flex items-start gap-3 rounded-[14px] border border-danger/25 bg-danger/5 px-5 py-4">
          <TriangleAlert
            size={17}
            strokeWidth={2.4}
            className="mt-0.5 shrink-0 text-danger"
          />
          <div>
            <div className="text-[13px] font-bold text-danger">无法读取数据集</div>
            <div className="mt-1 text-[13px] leading-relaxed font-medium text-ink-soft">
              {error}
            </div>
          </div>
        </div>
      ) : null}

      {!dataset ? (
        <div className="mt-8 rounded-[14px] border border-dashed border-line-strong px-6 py-16 text-center">
          {busy ? (
            <Loader2
              size={24}
              strokeWidth={2.2}
              className="mx-auto animate-spin text-ink-faint"
            />
          ) : (
            <ScanLine
              size={24}
              strokeWidth={2.2}
              className="mx-auto text-ink-faint"
            />
          )}
          <p className="mt-3 text-[14px] font-semibold text-ink-faint">
            {busy ? "正在扫描…" : "还没有选择数据集"}
          </p>
          <p className="mt-1 text-[12px] font-medium text-ink-faint">
            目录里需要有 dataset.yaml，格式由内容自动判断
          </p>
        </div>
      ) : (
        <>
          <div className="mt-8 grid grid-cols-3 gap-4">
            <Stat
              label="训练图片"
              value={dataset.train_images}
              unit="张"
              icon={Images}
              enter={0}
            />
            <Stat
              label="验证图片"
              value={dataset.val_images}
              unit="张"
              icon={Images}
              enter={1}
            />
            <Stat
              label="类别数"
              value={dataset.classes.length}
              icon={Tags}
              enter={2}
            />
          </div>

          {dataset.warning ? (
            <div className="mt-4 flex items-start gap-3 rounded-[14px] border border-warn/25 bg-warn/5 px-5 py-4">
              <TriangleAlert
                size={17}
                strokeWidth={2.4}
                className="mt-0.5 shrink-0 text-warn"
              />
              <div className="text-[13px] leading-relaxed font-medium text-ink-soft">
                {dataset.warning}
              </div>
            </div>
          ) : null}

          <Card className="mt-4" enter={0}>
            <SectionTitle
              icon={FolderOpen}
              title="数据源"
              hint={dataset.root}
              action={
                dataset.missing_labels === 0 && dataset.val_images > 0 ? (
                  <Badge tone="ok" icon={CheckCircle2}>
                    可以开始训练
                  </Badge>
                ) : (
                  <Badge tone="warn" icon={TriangleAlert}>
                    有需要处理的问题
                  </Badge>
                )
              }
            />
            <div className="flex flex-wrap gap-2">
              {dataset.classes.map((name, index) => (
                <span
                  key={name}
                  // Each chip arrives 26ms after the previous one: fast enough to read as one
                  // gesture, slow enough that the eye catches the count.
                  className="inline-flex animate-card-in items-center gap-1.5 rounded-[8px] border border-line bg-canvas px-2.5 py-1.5 text-[12px] font-bold"
                  style={{ animationDelay: `${index * 26}ms` }}
                >
                  <span className="tnum text-ink-faint">{index}</span>
                  {name}
                </span>
              ))}
            </div>
          </Card>

          <Card className="mt-4" enter={1}>
            {dataset.format === "imagenet-folders" ? (
              <>
                <SectionTitle
                  icon={Tags}
                  title="类别分布"
                  hint="每类的训练图片数。差距过大时，模型会偏向图片多的那一类。"
                />
                <div className="space-y-2.5">
                  {dataset.classes.map((name, index) => {
                    const count = dataset.class_counts[index] ?? 0;
                    const share =
                      dataset.train_images === 0
                        ? 0
                        : (count / dataset.train_images) * 100;
                    return (
                      <div key={name} className="flex items-center gap-3">
                        <span className="w-28 shrink-0 truncate text-[12px] font-bold">
                          {name}
                        </span>
                        <Progress value={share} className="flex-1" />
                        <span className="tnum w-10 shrink-0 text-right text-[12px] font-bold text-ink-soft">
                          {count}
                        </span>
                      </div>
                    );
                  })}
                </div>
              </>
            ) : (
              <>
                <SectionTitle
                  icon={Images}
                  title="标注概况"
                  hint={`${dataset.annotated} 张已标注，${dataset.missing_labels} 张缺标注。`}
                />
                <div className="flex items-center gap-3">
                  {/* Fixed 180px, not full width: a full-bleed bar at 100% is indistinguishable from
                      a horizontal rule, and this page is usually at 100%. */}
                  <Progress
                    value={
                      dataset.train_images === 0
                        ? 0
                        : (dataset.annotated / dataset.train_images) * 100
                    }
                    className="w-[180px]"
                  />
                  <span className="tnum min-w-[42px] text-right text-[12px] font-bold text-ink-soft">
                    {dataset.train_images === 0
                      ? "0%"
                      : `${Math.round(
                          (dataset.annotated / dataset.train_images) * 100,
                        )}%`}
                  </span>
                </div>
              </>
            )}
          </Card>
        </>
      )}

      {/* The empty half of this page used to be dead white. These are not decorative cards:
          they are the three things that actually go wrong for a first-time user, in order. */}
      {dataset ? (
        <Card className="mt-4" enter={2}>
          <SectionTitle
            icon={ArrowRight}
            title="下一步"
            hint="数据集已就绪，可以直接开始训练。"
          />
          <ol className="grid grid-cols-3 gap-3">
            {[
              {
                step: "1",
                title: "确认参数",
                note: "轮数、批大小、分辨率，默认值可用",
              },
              {
                step: "2",
                title: "开始训练",
                note: "自动探测显卡，失败回退 CPU",
              },
              {
                step: "3",
                title: "看曲线",
                note: "监控页实时显示每轮损失",
              },
            ].map((item) => (
              <li
                key={item.step}
                className="rounded-[10px] bg-surface-sunken px-4 py-3.5"
              >
                <div className="flex items-center gap-2">
                  <span className="tnum flex h-5 w-5 items-center justify-center rounded-full bg-ink text-[11px] font-bold text-canvas">
                    {item.step}
                  </span>
                  <span className="text-[13px] font-bold">{item.title}</span>
                </div>
                <p className="mt-1.5 text-[12px] leading-relaxed font-medium text-ink-soft">
                  {item.note}
                </p>
              </li>
            ))}
          </ol>
        </Card>
      ) : null}
    </div>
  );
}
