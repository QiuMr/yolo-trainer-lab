import { resetRun, setState } from "./store";
import { toast } from "./toast";

/* ─────────────────────────────────────────────────────────────────────────────
   Demo data for the browser preview.

   The Tauri sidecar is the only source of truth for a run, which means `pnpm dev` in a
   plain browser shows an empty monitor page — no curve, no counters, no log. That makes
   the most motion-heavy screen impossible to review without building the whole shell.

   `?demo=1` fills the store with a plausible mid-run state. Never loaded in the real app,
   and never fabricated from the engine's side: the numbers here are obviously fake on
   purpose (a smooth exponential decay, not real training output).
   ───────────────────────────────────────────────────────────────────────────── */

export function seedDemoRun(finished = false) {
  resetRun();
  const totalEpochs = finished ? 40 : 100;
  const lastEpoch = finished ? 40 : 37;
  const losses: Array<[number, number]> = [];
  for (let epoch = 1; epoch <= lastEpoch; epoch += 1) {
    const value = 2.4 * Math.exp(-epoch / 14) + 0.18 + Math.sin(epoch / 2.1) * 0.03;
    losses.push([epoch, Number(value.toFixed(4))]);
  }

  setState({
    status: finished ? "done" : "running",
    backend: "cpu",
    adapter: "Device<Flex(Cpu)>",
    runDir: "C:\\Users\\you\\AppData\\Roaming\\com.yolotrainer.desktop\\runs\\2026-11-10-19-20",
    totalEpochs,
    epoch: lastEpoch,
    losses,
    lastLoss: losses[losses.length - 1][1],
    lastEpochSeconds: 4.82,
    // `?demo=done` also exercises the post-training inference card, which only renders once a run
    // has produced an artifact.
    best: finished
      ? "C:\\Users\\you\\AppData\\Roaming\\com.yolotrainer.desktop\\runs\\2026-11-10-19-20\\exports\\best.bpk"
      : null,
    logs: [
      "引擎就绪 · montgomery 0.4.0",
      "数据集 tiny-dataset · 4 train / 2 val · 3 类",
      "显卡探测失败，回退到 CPU",
      "start · cpu · Device<Flex(Cpu)> · 100 轮 · batch 8",
      ...losses.slice(-3).map(([epoch, loss]) => `epoch ${epoch}/100 · loss ${loss}`),
    ].map((text, index) => ({
      id: index + 1,
      time: `19:2${index}:0${index % 9}`,
      text,
      tone: text.includes("回退") ? ("warn" as const) : undefined,
    })),
  });

  // One toast, long-lived, so the stack's placement and contrast can be reviewed in a
  // screenshot. It is the only piece of chrome that has no other way to be seen.
  toast("已扫描：4 张训练图 · 3 类", "ok", 120_000);
}
