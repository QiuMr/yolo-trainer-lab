import { useEffect, useState } from "react";
import {
  Activity,
  Bot,
  Boxes,
  Check,
  CircleAlert,
  Copy,
  Cpu,
  Eye,
  EyeOff,
  FileSearch,
  FolderOutput,
  FolderSearch,
  Loader2,
  Power,
  RotateCcw,
  Sparkles,
  Zap,
} from "lucide-react";
import { Badge, Button, Card, SectionTitle, cx } from "../components/ui";
import {
  getSettings,
  pickEngineFile,
  pickFolder,
  saveSettings,
  type Settings,
  type SettingsView,
} from "../lib/api";
import { copyText } from "../lib/clipboard";
import { toast } from "../lib/toast";

/* ─────────────────────────────────────────────────────────────────────────────
   Settings.

   Saves on every change instead of behind a Save button: with four fields there is
   nothing to compose, and a Save button that can be forgotten is a worse failure mode
   than an accidental write. The shell echoes back the stored view, so what is shown is
   always what is on disk — not what the form hopes is on disk.
   ───────────────────────────────────────────────────────────────────────────── */

const DEVICES = [
  { id: "auto", label: "自动", icon: Sparkles, note: "先试显卡，失败退回 CPU" },
  { id: "gpu", label: "GPU", icon: Zap, note: "强制显卡，跑不动就报错" },
  { id: "cpu", label: "CPU", icon: Cpu, note: "最稳，也最慢" },
] as const;

/** Show enough to recognise the token, not enough to leak it into a screenshot. */
function maskToken(token: string): string {
  if (token.length <= 12) return "•".repeat(Math.max(token.length, 8));
  return `${token.slice(0, 6)}${"•".repeat(10)}${token.slice(-4)}`;
}

/** The prompt the user pastes into an assistant. Deliberately short: the API document at `/` is
 *  the contract, and duplicating it here would go stale the first time a field changes. */
function promptFor(base: string, token: string): string {
  return [
    "我在本机用 YOLO Trainer 训练目标检测模型。它已经开好了本地控制接口，请直接通过接口帮我操作，不要让我手工点界面。",
    "",
    `接口地址：${base}`,
    `令牌：${token}`,
    "",
    `第一步先 GET ${base}/ （不需要令牌），那里有完整的端点、事件和错误码说明，按它来。`,
    "其余请求都要带请求头：Authorization: Bearer <上面的令牌>",
    "",
    "先扫描数据集确认可用，再起训练，然后轮询进度直到结束，最后把产物路径和 loss 变化告诉我。",
  ].join("\n");
}

export default function SettingsPage({
  onEngineChanged,
}: {
  onEngineChanged?: () => void;
}) {
  const [view, setView] = useState<SettingsView | null>(null);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The token is masked by default: this page gets screenshotted into bug reports.
  const [tokenShown, setTokenShown] = useState(false);

  useEffect(() => {
    let alive = true;
    void getSettings()
      .then((next) => {
        if (alive) setView(next);
      })
      .catch((cause) => {
        if (alive) setError(String(cause));
      });
    return () => {
      alive = false;
    };
  }, []);

  async function apply(patch: Partial<Settings>) {
    if (!view) return;
    setBusy(true);
    setError(null);
    try {
      const next = await saveSettings({
        engine_path: view.engine_path,
        default_device: view.default_device,
        output_dir: view.output_dir_is_default ? null : view.output_dir,
        control_enabled: view.control.enabled,
        control_port: view.control.port,
        ...patch,
      });
      setView(next);
      setSaved(true);
      window.setTimeout(() => setSaved(false), 1600);
      onEngineChanged?.();
    } catch (cause) {
      // The shell's message is already user-facing; do not wrap it again.
      setError(typeof cause === "string" ? cause : String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function copy(text: string, message: string) {
    const ok = await copyText(text);
    toast(ok ? message : "复制失败，请手动选中文本复制", ok ? "ok" : "danger");
  }

  if (!view) {
    return (
      <div>
        <h1 className="text-[26px] font-extrabold tracking-tight">设置</h1>
        <div className="mt-8 flex items-center gap-2 text-[13px] font-semibold text-ink-faint">
          <Loader2 size={15} strokeWidth={2.4} className="animate-spin" />
          正在读取配置…
        </div>
      </div>
    );
  }

  const engineOk = !view.engine_error;
  const control = view.control;

  return (
    <div>
      <div className="flex items-start justify-between gap-4">
        <div>
          <h1 className="text-[26px] font-extrabold tracking-tight">设置</h1>
          <p className="mt-2 text-[14px] font-medium text-ink-soft">
            引擎、计算设备与产物位置。改完立即生效，不需要重启。
          </p>
        </div>
        <span
          className={cx(
            "mt-1 flex items-center gap-1.5 text-[12px] font-bold text-ok",
            "transition-[opacity,transform] duration-180 ease-spring",
            saved
              ? "translate-y-0 scale-100 opacity-100"
              : "translate-y-1 scale-95 opacity-0",
          )}
        >
          <Check size={14} strokeWidth={3} />
          已保存
        </span>
      </div>

      <Card className="mt-6" enter={0}>
        <SectionTitle
          icon={Boxes}
          title="训练引擎"
          hint="独立的 montgomery.exe。留空则自动在当前目录与安装目录里查找。"
          action={
            <div className="flex shrink-0 items-center gap-2">
              <Button
                icon={FileSearch}
                variant="secondary"
                size="sm"
                onClick={async () => {
                  const picked = await pickEngineFile();
                  if (picked) void apply({ engine_path: picked });
                }}
              >
                选择文件
              </Button>
              <Button
                icon={RotateCcw}
                variant="ghost"
                size="sm"
                disabled={view.engine_path === null || busy}
                onClick={() => void apply({ engine_path: null })}
              >
                自动检测
              </Button>
            </div>
          }
        />

        <div
          className={cx(
            "rounded-[10px] border px-4 py-3.5",
            engineOk ? "border-line-strong" : "border-danger/30 bg-danger/5",
          )}
        >
          <div className="flex items-center gap-2 text-[12px] font-bold">
            <span
              className={cx(
                "h-2 w-2 shrink-0 rounded-full",
                engineOk ? "bg-ok" : "bg-danger",
              )}
            />
            {engineOk ? "已找到引擎" : "引擎不可用"}
            {view.engine_running ? (
              <span className="ml-1 rounded-[6px] bg-surface-sunken px-1.5 py-0.5 text-[10px] font-bold text-ink-soft">
                训练中
              </span>
            ) : null}
          </div>
          <div className="mt-2 font-mono text-[12px] leading-relaxed font-medium break-all text-ink-soft">
            {engineOk ? view.engine_resolved : view.engine_error}
          </div>
          {view.engine_path !== null ? (
            <div className="mt-2 text-[11px] font-semibold text-ink-faint">
              手动指定：{view.engine_path}
            </div>
          ) : null}
        </div>
      </Card>

      <Card className="mt-4" enter={1}>
        <SectionTitle
          icon={Zap}
          title="默认计算设备"
          hint="训练页会以这里的选择作为初始值，每次训练仍然可以单独改。"
        />
        <div className="grid grid-cols-3 gap-3">
          {DEVICES.map((item) => {
            const Icon = item.icon;
            const active = view.default_device === item.id;
            return (
              <button
                key={item.id}
                type="button"
                disabled={busy}
                onClick={() => void apply({ default_device: item.id })}
                className={cx(
                  "press flex items-start gap-3 rounded-[10px] border p-3.5 text-left",
                  "transition-[background-color,border-color] duration-120 ease-out",
                  active
                    ? "border-ink bg-surface"
                    : "border-line-strong bg-canvas hover:border-ink/40",
                )}
              >
                <span
                  className={cx(
                    "flex h-8 w-8 shrink-0 items-center justify-center rounded-[8px]",
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

      <Card className="mt-4" enter={2}>
        <SectionTitle
          icon={FolderOutput}
          title="产物目录"
          hint="每次训练在这里新建一个子目录，存放权重、曲线与导出的 best.bpk。"
          action={
            <div className="flex shrink-0 items-center gap-2">
              <Button
                icon={FolderSearch}
                variant="secondary"
                size="sm"
                onClick={async () => {
                  const picked = await pickFolder("选择产物目录");
                  if (picked) {
                    void apply({ output_dir: picked });
                  }
                }}
              >
                选择目录
              </Button>
              <Button
                icon={RotateCcw}
                variant="ghost"
                size="sm"
                disabled={view.output_dir_is_default || busy}
                onClick={() => void apply({ output_dir: null })}
              >
                恢复默认
              </Button>
            </div>
          }
        />
        <div className="rounded-[10px] border border-line-strong px-4 py-3.5">
          <div className="flex items-center gap-2 text-[12px] font-bold">
            <span className="h-2 w-2 shrink-0 rounded-full bg-ok" />
            {view.output_dir_is_default ? "应用数据目录" : "自定义目录"}
          </div>
          <div className="mt-2 font-mono text-[12px] leading-relaxed font-medium break-all text-ink-soft">
            {view.output_dir}
          </div>
        </div>
      </Card>

      <Card className="mt-4" enter={3}>
        <SectionTitle
          icon={Bot}
          title="AI 操控"
          hint="开一个只在本机监听的接口，AI 助手或脚本就能直接扫描数据集、起训练、读进度。"
          action={
            control.running ? (
              <Badge tone="ok" icon={Activity}>
                运行中
              </Badge>
            ) : (
              <Badge tone="neutral">已关闭</Badge>
            )
          }
        />

        {control.running ? (
          <div className="rounded-[10px] border border-line-strong px-4 py-3.5">
            <div className="flex items-center gap-2 text-[12px] font-bold">
              <span className="h-2 w-2 shrink-0 rounded-full bg-ok" />
              仅本机可访问
              <span className="ml-auto font-mono text-[12px] font-semibold text-ink-soft">
                {control.base_url}
              </span>
            </div>

            <div className="mt-3 flex items-center gap-2">
              <span className="shrink-0 text-[11px] font-bold tracking-wide text-ink-faint uppercase">
                令牌
              </span>
              <code className="min-w-0 flex-1 truncate rounded-[6px] bg-surface-sunken px-2 py-1 font-mono text-[12px] font-medium text-ink-soft">
                {tokenShown ? control.token : maskToken(control.token)}
              </code>
              <button
                type="button"
                onClick={() => setTokenShown((shown) => !shown)}
                title={tokenShown ? "隐藏令牌" : "显示令牌"}
                aria-label={tokenShown ? "隐藏令牌" : "显示令牌"}
                className="flex h-7 w-7 shrink-0 items-center justify-center rounded-[6px] text-ink-faint transition-colors duration-120 hover:bg-surface-sunken hover:text-ink"
              >
                {tokenShown ? (
                  <EyeOff size={14} strokeWidth={2.4} />
                ) : (
                  <Eye size={14} strokeWidth={2.4} />
                )}
              </button>
              <button
                type="button"
                onClick={() => void copy(control.token, "令牌已复制")}
                title="复制令牌"
                aria-label="复制令牌"
                className="flex h-7 w-7 shrink-0 items-center justify-center rounded-[6px] text-ink-faint transition-colors duration-120 hover:bg-surface-sunken hover:text-ink"
              >
                <Copy size={14} strokeWidth={2.4} />
              </button>
            </div>
          </div>
        ) : (
          <div
            className={cx(
              "rounded-[10px] border px-4 py-3.5",
              control.error ? "border-danger/30 bg-danger/5" : "border-line-strong",
            )}
          >
            <div className="flex items-center gap-2 text-[12px] font-bold">
              <span
                className={cx(
                  "h-2 w-2 shrink-0 rounded-full",
                  control.error ? "bg-danger" : "bg-ink-faint",
                )}
              />
              {control.error ? "启动失败" : "服务未开启"}
            </div>
            <div className="mt-2 text-[12px] leading-relaxed font-medium text-ink-soft">
              {control.error ??
                "开启后，任何本机程序都能用这段地址驱动训练。端口冲突时会自动换一个可用端口。"}
            </div>
          </div>
        )}

        <div className="mt-4 flex flex-wrap items-center gap-2">
          <Button
            icon={Copy}
            onClick={() =>
              void copy(promptFor(control.base_url, control.token), "提示词已复制")
            }
            disabled={!control.running}
          >
            复制给 AI 的提示词
          </Button>
          <Button
            icon={Power}
            variant={control.enabled ? "ghost" : "secondary"}
            disabled={busy}
            onClick={() =>
              void apply({ control_enabled: !control.enabled })
            }
          >
            {control.enabled ? "关闭服务" : "开启服务"}
          </Button>
        </div>

        <p className="mt-3 text-[11px] leading-relaxed font-medium text-ink-faint">
          把提示词粘贴给任意 AI 助手，它会先读接口文档再动手。
          服务只绑定 127.0.0.1，外部网络连不上。
        </p>
      </Card>

      {error ? (
        <div className="mt-4 flex items-start gap-3 rounded-[14px] border border-danger/25 bg-danger/5 px-5 py-4">
          <CircleAlert
            size={16}
            strokeWidth={2.4}
            className="mt-0.5 shrink-0 text-danger"
          />
          <div className="text-[13px] leading-relaxed font-semibold text-danger">
            {error}
          </div>
        </div>
      ) : null}

      <p className="mt-6 text-[11px] leading-relaxed font-medium text-ink-faint">
        配置文件：{view.config_dir}\settings.json
      </p>
    </div>
  );
}
