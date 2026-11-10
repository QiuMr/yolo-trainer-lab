import { useEffect, useState } from "react";
import { CircleAlert, Download, RotateCcw, ShieldCheck } from "lucide-react";
import { Badge, Button, Card, SectionTitle, cx } from "./ui";
import { Progress } from "./Motion";
import {
  checkUpdate,
  getAppVersion,
  installUpdate,
  onUpdateProgress,
  type UpdateInfo,
} from "../lib/api";

/* ─────────────────────────────────────────────────────────────────────────────
   "版本与更新" card on the settings page.

   Two deliberate choices:

   1. A silent check on mount, with a 6-hour localStorage cache. Users should not have to
      click to learn there is an update, but they also should not wait on a network request
      every time they flip to this tab. A failed check is NOT cached — a transient proxy
      hiccup must not turn into six hours of "could not check".
   2. Network failures are shown as a quiet sentence, not a red error banner. Being offline
      is normal; a red box makes people think the app is broken. Only a failed *install*
      gets a stronger treatment, because that is an action the user explicitly started.
   ───────────────────────────────────────────────────────────────────────────── */

const CACHE_KEY = "yolo.update.check";
const CACHE_TTL = 6 * 60 * 60 * 1000;

type Cached = { info: UpdateInfo; ts: number };

function readCache(current: string): UpdateInfo | null {
  try {
    const raw = localStorage.getItem(CACHE_KEY);
    if (!raw) return null;
    const cached = JSON.parse(raw) as Cached;
    if (typeof cached?.ts !== "number" || Date.now() - cached.ts > CACHE_TTL) return null;
    // Written before an upgrade: its "has update" conclusion no longer holds.
    if (cached.info?.current !== current) return null;
    return cached.info;
  } catch {
    return null;
  }
}

function writeCache(info: UpdateInfo): void {
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify({ info, ts: Date.now() } satisfies Cached));
  } catch {
    // Private mode / quota: the cache only saves one request, losing it is harmless.
  }
}

export default function UpdateCard() {
  const [version, setVersion] = useState<string | null>(null);
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [checking, setChecking] = useState(false);
  const [phase, setPhase] = useState<"idle" | "downloading" | "installing">("idle");
  const [percent, setPercent] = useState(0);
  const [installError, setInstallError] = useState<string | null>(null);

  /** `force` bypasses the cache — that is what the refresh button is for. */
  async function runCheck(force = false) {
    setChecking(true);
    try {
      const current = await getAppVersion();
      setVersion(current);
      if (!force) {
        const cached = readCache(current);
        if (cached) {
          setInfo(cached);
          return;
        }
      }
      const next = await checkUpdate();
      setInfo(next);
      if (!next.error) writeCache(next);
    } catch (cause) {
      setInfo({
        current: version ?? "—",
        latest: version ?? "—",
        has_update: false,
        notes: "",
        error: String(cause),
      });
    } finally {
      setChecking(false);
    }
  }

  useEffect(() => {
    let alive = true;
    void runCheck();

    let unlisten: (() => void) | undefined;
    void onUpdateProgress((progress) => {
      if (!alive) return;
      setPhase(progress.phase);
      setPercent(progress.percent);
    }).then((fn) => {
      if (alive) unlisten = fn;
      else fn();
    });

    return () => {
      alive = false;
      unlisten?.();
    };
    // Mount-only: the silent check is a startup concern, not a render effect. The refresh
    // button calls runCheck directly.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function doInstall() {
    setInstallError(null);
    setPercent(0);
    setPhase("downloading");
    try {
      // On Windows a successful install exits the process; this promise normally never settles.
      await installUpdate();
      setPhase("installing");
    } catch (cause) {
      setPhase("idle");
      setInstallError(typeof cause === "string" ? cause : String(cause));
    }
  }

  const busy = phase !== "idle";
  const headline = busy
    ? phase === "installing"
      ? "正在安装，程序将自动重启"
      : percent >= 0
        ? `正在下载更新 · ${percent}%`
        : "正在下载更新…"
    : info?.error
      ? info.error
      : info?.has_update
        ? `发现新版本 v${info.latest}`
        : info
          ? "已是最新版本"
          : "正在检查更新…";

  return (
    <Card className="mt-4" enter={4}>
      <SectionTitle
        icon={ShieldCheck}
        title="版本与更新"
        hint="安装包会先校验签名，再由 Windows 安装器升级。"
      />

      <div className="flex items-center justify-between gap-4">
        <div className="flex min-w-0 items-center gap-3">
          <span className="shrink-0 rounded-[8px] bg-surface-sunken px-2.5 py-1 font-mono text-[12px] font-bold tracking-tight">
            {/* Browser preview has no version number; prefixing it with "v" would read as a lie. */}
            {version && /^\d/.test(version) ? `v${version}` : (version ?? "…")}
          </span>
          <span
            className={cx(
              "truncate text-[13px] font-semibold",
              info?.error && !busy ? "text-ink-faint" : "text-ink-soft",
            )}
            title={headline}
          >
            {headline}
          </span>
          {info?.has_update && !busy ? (
            <Badge tone="warn">可升级</Badge>
          ) : null}
        </div>

        <div className="flex shrink-0 items-center gap-2">
          <Button
            icon={RotateCcw}
            variant="ghost"
            size="sm"
            loading={checking}
            disabled={busy}
            onClick={() => void runCheck(true)}
          >
            检查更新
          </Button>
          {info?.has_update ? (
            <Button
              icon={Download}
              size="sm"
              disabled={busy}
              onClick={() => void doInstall()}
            >
              下载并安装
            </Button>
          ) : null}
        </div>
      </div>

      {info?.has_update && info.notes.trim() ? (
        <div className="mt-3 rounded-[10px] bg-surface px-3.5 py-2.5">
          <p className="whitespace-pre-wrap text-[12px] leading-relaxed font-medium text-ink-soft">
            {info.notes.trim()}
          </p>
        </div>
      ) : null}

      {busy && phase === "downloading" && percent >= 0 ? (
        <Progress value={percent} className="mt-3 w-full" />
      ) : null}

      {installError ? (
        <div className="mt-3 flex items-start gap-2 rounded-[10px] border border-danger/20 bg-danger/5 px-3 py-2.5">
          <CircleAlert size={14} strokeWidth={2.4} className="mt-0.5 shrink-0 text-danger" />
          <span className="text-[12px] leading-relaxed font-semibold text-danger">
            {installError}
          </span>
        </div>
      ) : null}

      <p className="mt-3 text-[11px] leading-relaxed font-medium text-ink-faint">
        训练产物、数据集与设置保存在用户目录，升级不会覆盖。
      </p>
    </Card>
  );
}
