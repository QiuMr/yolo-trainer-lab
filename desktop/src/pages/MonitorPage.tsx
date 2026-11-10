import { useEffect, useRef } from "react";
import * as echarts from "echarts";
import {
  ArrowDownToLine,
  CircleStop,
  Clock,
  CircleCheck,
  LineChart,
  ScrollText,
  Target,
  TrendingDown,
  TriangleAlert,
} from "lucide-react";
import { Badge, Button, Card, SectionTitle, Stat, cx } from "../components/ui";
import PredictCard from "../components/PredictCard";
import { AnimatedNumber, Progress } from "../components/Motion";
import { ERROR_COPY, stopTraining } from "../lib/api";
import { useAppState } from "../lib/store";
import { useAutoFollow } from "../lib/gestures";

const AXIS_LABEL = { color: "#a1a1aa", fontWeight: 600, fontSize: 11 };

export default function MonitorPage() {
  const state = useAppState();
  const chartRef = useRef<HTMLDivElement>(null);
  const chartInstance = useRef<echarts.ECharts | null>(null);
  // Follows new output only while the user is already at the bottom; scrolling up pauses it.
  const log = useAutoFollow<HTMLDivElement>([state.logs.length]);

  useEffect(() => {
    if (!chartRef.current) return;
    const chart = echarts.init(chartRef.current);
    chartInstance.current = chart;
    chart.setOption({
      animationDuration: 300,
      grid: { left: 8, right: 12, top: 16, bottom: 8, containLabel: true },
      xAxis: {
        type: "value",
        min: 1,
        // No `name`: ECharts renders it flush against the axis end, which produced a clipped
        // "EP" fragment outside the plot area. The card title already says what the axis is.
        axisLine: { lineStyle: { color: "#e8e8ea" } },
        axisTick: { show: false },
        axisLabel: AXIS_LABEL,
        splitLine: { show: false },
      },
      yAxis: {
        type: "value",
        scale: true,
        axisLine: { show: false },
        axisTick: { show: false },
        axisLabel: AXIS_LABEL,
        splitLine: { lineStyle: { color: "#f4f4f5" } },
      },
      tooltip: {
        trigger: "axis",
        backgroundColor: "#0a0a0a",
        borderWidth: 0,
        textStyle: { color: "#ffffff", fontSize: 12, fontWeight: 600 },
        padding: [8, 10],
      },
      series: [
        {
          type: "line",
          data: [],
          showSymbol: false,
          symbol: "circle",
          symbolSize: 6,
          itemStyle: { color: "#0a0a0a" },
          smooth: 0.25,
          lineStyle: { color: "#0a0a0a", width: 2.2 },
          areaStyle: {
            color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
              { offset: 0, color: "rgba(10,10,10,0.10)" },
              { offset: 1, color: "rgba(10,10,10,0)" },
            ]),
          },
        },
      ],
    });

    const observer = new ResizeObserver(() => chart.resize());
    observer.observe(chartRef.current);
    return () => {
      observer.disconnect();
      chart.dispose();
      chartInstance.current = null;
    };
  }, []);

  // Push new points without rebuilding the chart, so the animation stays continuous.
  useEffect(() => {
    chartInstance.current?.setOption({
      // ECharts' default update animation is a full re-layout of the series; 220ms with a
      // cubic-out easing makes a single new point look like the line growing into place
      // rather than the whole chart blinking.
      animationDurationUpdate: 220,
      animationEasingUpdate: "cubicOut",
      series: [
        {
          data: state.losses,
          // A line needs two points to be a line. Short runs (1–2 epochs, common when testing
          // whether a GPU works at all) would otherwise render an empty plot area and look like a
          // broken chart rather than a finished one-point run.
          showSymbol: state.losses.length <= 8,
          // Repeated here, not just in the initial option: ECharts keeps the palette colour for
          // symbols unless the item style is present on the update that creates them, and an
          // off-palette blue dot on a black-and-white chart looks like a bug.
          itemStyle: { color: "#0a0a0a", borderColor: "#0a0a0a" },
        },
      ],
    });
  }, [state.losses]);

  const remaining =
    state.lastEpochSeconds && state.totalEpochs > state.epoch
      ? (state.totalEpochs - state.epoch) * state.lastEpochSeconds
      : null;

  const copy = state.error ? ERROR_COPY[state.error.code] ?? ERROR_COPY.INTERNAL : null;

  return (
    <div>
      <div className="flex items-start justify-between gap-6">
        <div>
          <h1 className="text-[26px] font-extrabold tracking-tight">监控</h1>
          <p className="mt-2 text-[14px] font-medium text-ink-soft">
            训练过程中的损失、进度与日志。
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {state.status === "running" ? (
            <Badge tone="ok" pulse>
              训练中
            </Badge>
          ) : state.status === "done" ? (
            <Badge tone="ok" icon={CircleCheck}>
              已完成
            </Badge>
          ) : state.status === "failed" ? (
            <Badge tone="danger" icon={TriangleAlert}>
              失败
            </Badge>
          ) : (
            <Badge tone="neutral">未开始</Badge>
          )}
          <Button
            variant="secondary"
            icon={CircleStop}
            disabled={state.status !== "running"}
            onClick={() => void stopTraining()}
          >
            停止
          </Button>
        </div>
      </div>

      {state.error && copy ? (
        <div className="mt-6 flex animate-card-in items-start gap-3 rounded-[14px] border border-danger/25 bg-danger/5 px-5 py-4">
          <TriangleAlert
            size={17}
            strokeWidth={2.4}
            className="mt-0.5 shrink-0 text-danger"
          />
          <div>
            <div className="text-[13px] font-bold text-danger">
              {copy.title}
              <span className="ml-2 font-mono text-[11px] font-semibold opacity-70">
                {state.error.code}
              </span>
            </div>
            <div className="mt-1 text-[13px] leading-relaxed font-medium text-ink-soft">
              {state.error.hint ?? copy.action}
            </div>
          </div>
        </div>
      ) : null}

      <div className="mt-8 grid grid-cols-4 gap-4">
        <Stat
          label="进度"
          value={
            <>
              <AnimatedNumber value={state.epoch} />
              <span className="text-[17px] font-bold text-ink-faint">
                {" "}
                / {state.totalEpochs || 0}
              </span>
            </>
          }
          unit="轮"
          icon={Target}
          enter={0}
        />
        <Stat
          label="当前损失"
          value={state.lastLoss}
          digits={2}
          icon={TrendingDown}
          enter={1}
        />
        <Stat
          label="单轮耗时"
          value={state.lastEpochSeconds}
          digits={2}
          // A unit next to a dash reads as "— 秒", which parses as a value that does not exist.
          unit={state.lastEpochSeconds === null ? undefined : "秒"}
          icon={Clock}
          enter={2}
        />
        <Stat
          label="预计剩余"
          value={remaining}
          unit={remaining === null ? undefined : "秒"}
          icon={Clock}
          enter={3}
        />
      </div>

      <Card className="mt-4" enter={0}>
        <SectionTitle
          icon={LineChart}
          title="训练损失"
          hint={
            state.backend
              ? `后端 ${state.backend}${state.adapter ? ` · ${state.adapter}` : ""}`
              : "每轮平均损失，越低越好。"
          }
          action={
            state.totalEpochs > 0 ? (
              <div className="flex shrink-0 items-center gap-2.5">
                <Progress
                  value={(state.epoch / state.totalEpochs) * 100}
                  className="w-[110px]"
                />
                <span className="tnum text-[11.5px] font-bold text-ink-faint">
                  {Math.round((state.epoch / state.totalEpochs) * 100)}%
                </span>
              </div>
            ) : undefined
          }
        />
        <div className="relative">
          <div ref={chartRef} data-no-swipe className="h-[260px] w-full" />
          {state.losses.length === 0 ? (
            // Opaque overlay rather than unmounting the chart: the instance must stay alive so the
            // first data point animates in instead of the chart being rebuilt mid-run.
            <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 rounded-[10px] bg-canvas">
              <LineChart size={20} strokeWidth={2.2} className="text-ink-faint" />
              <span className="text-[13px] font-bold text-ink-soft">
                等待第一轮训练
              </span>
              <span className="text-[12px] font-medium text-ink-faint">
                每完成一轮，这里会画出该轮的平均损失
              </span>
            </div>
          ) : null}
        </div>
      </Card>

      {/* Only after a successful run: the card infers with the best artifact of that run, so
          showing it mid-run or after a failure would promise a model that does not exist yet. */}
      {state.status === "done" && state.best ? (
        <PredictCard className="mt-4" />
      ) : null}

      <Card className="mt-4" padded={false} enter={1}>
        <div className="border-b border-line px-6 py-4">
          <SectionTitle
            icon={ScrollText}
            title="运行日志"
            hint="引擎的原始输出，出问题先看这里。"
            className="mb-0"
          />
        </div>
        <div className="relative">
          <div
            ref={log.ref}
            data-no-swipe
            onScroll={log.onScroll}
            className="max-h-[220px] overflow-y-auto px-6 py-4"
          >
            {state.logs.length === 0 ? (
              <p className="text-[13px] font-medium text-ink-faint">
                暂无输出。
              </p>
            ) : (
              <pre className="m-0 font-mono text-[12px] leading-relaxed font-medium whitespace-pre-wrap">
                {state.logs.map((line) => (
                  // Keyed by a monotonic id, not the index: the list is capped at 400 lines
                  // and dropping the oldest one would otherwise replay every fade-in.
                  <div
                    key={line.id}
                    className="flex animate-log-in gap-3"
                  >
                    <span className="shrink-0 text-ink-faint">{line.time}</span>
                    <span
                      className={
                        line.tone === "danger"
                          ? "text-danger"
                          : line.tone === "warn"
                            ? "text-warn"
                            : "text-ink-soft"
                      }
                    >
                      {line.text}
                    </span>
                  </div>
                ))}
              </pre>
            )}
          </div>

          {/* Auto-follow is paused the moment the user scrolls up, and this is the only way
              back. Silently resuming would yank the panel out from under someone reading. */}
          {!log.atBottom && state.logs.length > 0 ? (
            <button
              type="button"
              onClick={log.scrollToLatest}
              className={cx(
                "animate-pop-in absolute right-4 bottom-3 flex h-7 items-center gap-1.5",
                "rounded-[8px] bg-ink px-2.5 text-[11.5px] font-bold text-canvas",
                "shadow-[0_8px_20px_-12px_rgba(10,10,10,0.9)]",
                "press",
              )}
            >
              <ArrowDownToLine size={12} strokeWidth={2.8} />
              跳到最新
            </button>
          ) : null}
        </div>
      </Card>
    </div>
  );
}
