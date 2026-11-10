import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { ImageUp, Loader2, RefreshCw, ScanSearch, TriangleAlert } from "lucide-react";
import { Button, Card, SectionTitle, cx } from "./ui";
import { Progress } from "./Motion";
import { isTauri, pickImageFile, predictImage, type PredictResult } from "../lib/api";

/**
 * Post-training check: run the trained model on one image.
 *
 * A falling loss only says the optimizer is working. Whether the artifact is usable is a
 * different question, and the answer is a picture. This card is the shortest path from "training
 * finished" to "I can see it working" without leaving the app.
 */

type Detection = {
  class_name: string;
  confidence: number;
  xmin: number;
  ymin: number;
  xmax: number;
  ymax: number;
};

type ClassScore = { class_name: string; confidence: number };

/** The engine's document is untyped here on purpose — `predict.rs` forwards it verbatim, and the
 *  shape depends on the task. These two readers are the only places that assume anything. */
function detectionsOf(result: PredictResult): Detection[] {
  const value = (result.prediction as { detections?: unknown }).detections;
  return Array.isArray(value) ? (value as Detection[]) : [];
}

function classesOf(result: PredictResult): ClassScore[] {
  const value = (result.prediction as { classes?: unknown }).classes;
  return Array.isArray(value) ? (value as ClassScore[]) : [];
}

export default function PredictCard({ className }: { className?: string }) {
  const [image, setImage] = useState<string | null>(null);
  const [result, setResult] = useState<PredictResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function choose() {
    // The browser preview has no file dialog; api.ts hands back a stand-in prediction instead, so
    // the card can still be reviewed there.
    const picked = isTauri
      ? await pickImageFile()
      : "…\\tiny-dataset\\images\\val\\img1.jpg";
    if (!picked) return;
    setImage(picked);
    setResult(null);
    setError(null);
    setBusy(true);
    try {
      setResult(await predictImage({ source: picked }));
    } catch (cause) {
      setError(typeof cause === "string" ? cause : String(cause));
    } finally {
      setBusy(false);
    }
  }

  // The engine returns detections in model order, not by confidence. A weakly trained model can
  // emit three hundred near-identical boxes, and "the first eight" then looks like noise — sort so
  // the list answers "what did it recognise" rather than "what did it emit first".
  const detections = result
    ? detectionsOf(result)
        .slice()
        .sort((a, b) => b.confidence - a.confidence)
    : [];
  const classes = result ? classesOf(result) : [];
  const imageUrl =
    result?.output && isTauri ? convertFileSrc(result.output) : null;

  return (
    <Card className={className}>
      <SectionTitle
        icon={ScanSearch}
        title="验证模型"
        hint="拿一张图跑一次推理。损失下降只说明在学，看到框才算能用。"
        action={
          <Button
            variant="secondary"
            icon={image ? RefreshCw : ImageUp}
            loading={busy}
            onClick={() => void choose()}
            className="shrink-0"
          >
            {image ? "换一张" : "选一张图"}
          </Button>
        }
      />

      {error ? (
        <div className="flex items-start gap-2.5 rounded-[10px] border border-danger/25 bg-danger/5 px-3.5 py-2.5">
          <TriangleAlert size={14} strokeWidth={2.4} className="mt-0.5 shrink-0 text-danger" />
          <span className="text-[12px] leading-relaxed font-semibold text-ink-soft">
            {error}
          </span>
        </div>
      ) : null}

      {!result && !error ? (
        <div className="rounded-[10px] bg-surface px-4 py-6 text-center">
          {busy ? (
            <Loader2 size={20} strokeWidth={2.2} className="mx-auto animate-spin text-ink-faint" />
          ) : (
            <ScanSearch size={20} strokeWidth={2.2} className="mx-auto text-ink-faint" />
          )}
          <p className="mt-2 text-[12.5px] font-semibold text-ink-faint">
            {busy ? "正在推理…" : image ? "已选好图片" : "选一张图，看看模型认出了什么"}
          </p>
          {image ? (
            <p className="mt-1 truncate text-[11px] font-medium text-ink-faint/80">{image}</p>
          ) : null}
        </div>
      ) : null}

      {result ? (
        <div className="flex gap-4">
          {imageUrl ? (
            <div className="w-[46%] shrink-0">
              <img
                src={imageUrl}
                alt="推理结果"
                className="w-full rounded-[10px] border border-line object-contain"
              />
              <p className="mt-1.5 truncate text-[10.5px] font-medium text-ink-faint">
                {result.output}
              </p>
            </div>
          ) : null}

          <div className={cx("min-w-0 flex-1", imageUrl ? "" : "w-full")}>
            {classes.length > 0 ? (
              <div className="space-y-2.5">
                {classes.map((item, index) => (
                  <div key={item.class_name} className="flex items-center gap-2.5">
                    <span className="w-24 shrink-0 truncate text-[12px] font-bold">
                      {item.class_name}
                    </span>
                    <Progress value={item.confidence * 100} className="flex-1" />
                    <span className="tnum w-11 shrink-0 text-right text-[11.5px] font-bold text-ink-soft">
                      {(item.confidence * 100).toFixed(1)}%
                    </span>
                    {index === 0 ? (
                      <span className="shrink-0 rounded-[6px] bg-ink px-1.5 py-0.5 text-[10px] font-bold text-canvas">
                        最高
                      </span>
                    ) : null}
                  </div>
                ))}
              </div>
            ) : null}

            {detections.length > 0 ? (
              <div>
                <div className="mb-1.5 flex items-baseline justify-between">
                  <span className="text-[11px] font-bold tracking-wide text-ink-faint uppercase">
                    检出 {detections.length} 个
                  </span>
                  {detections.length > 8 ? (
                    <span className="text-[10.5px] font-medium text-ink-faint">
                      只列前 8 个
                    </span>
                  ) : null}
                </div>
                <div className="space-y-1">
                  {detections.slice(0, 8).map((item, index) => (
                    <div
                      key={`${item.class_name}-${index}`}
                      className="flex items-center justify-between gap-3 rounded-[8px] bg-surface px-2.5 py-1.5"
                    >
                      <span className="min-w-0 truncate text-[12px] font-bold">
                        {item.class_name}
                      </span>
                      <span className="tnum shrink-0 text-[11.5px] font-semibold text-ink-soft">
                        {(item.confidence * 100).toFixed(1)}%
                      </span>
                    </div>
                  ))}
                </div>
              </div>
            ) : null}

            {detections.length === 0 && classes.length === 0 ? (
              <p className="text-[12.5px] leading-relaxed font-semibold text-ink-soft">
                这张图没有检出任何目标。可能是模型还没训好，也可能是图里确实没有它认识的类别。
              </p>
            ) : null}
          </div>
        </div>
      ) : null}
    </Card>
  );
}
