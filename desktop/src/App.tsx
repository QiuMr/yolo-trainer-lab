import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { FolderDown, PanelLeft, Play } from "lucide-react";
import DatasetPage from "./pages/DatasetPage";
import TrainPage from "./pages/TrainPage";
import MonitorPage from "./pages/MonitorPage";
import SettingsPage from "./pages/SettingsPage";
import TitleBar from "./components/TitleBar";
import CommandPalette, { type PaletteAction } from "./components/CommandPalette";
import Toaster from "./components/Toaster";
import Sidebar, { NAV, useSidebar, type PageId } from "./components/Sidebar";
import {
  getSettings,
  onEngineEvent,
  onEngineLog,
  resolveEngine,
  scanDataset,
} from "./lib/api";
import { applyEvent, pushLog, setState } from "./lib/store";
import { useSwipe } from "./lib/gestures";
import { useWindowDrop } from "./lib/dnd";
import { toast } from "./lib/toast";

export default function App() {
  // `?page=train` opens that tab directly. Only used for screenshots and manual QA: the app has no
  // router, and without this you cannot inspect a page without clicking through the UI.
  const [page, setPage] = useState<PageId>(() => {
    const requested = new URLSearchParams(window.location.search).get("page");
    const known = NAV.some((item) => item.id === requested);
    return known ? (requested as PageId) : "dataset";
  });
  // Which way the page is entering from. Forward = moving down the rail, so the new page
  // comes in from the right. Without this the transition is a fade and direction is lost.
  const [direction, setDirection] = useState<"forward" | "back">("forward");
  const current = useRef<PageId>(page);
  const [engine, setEngine] = useState<string | null>(null);
  const [defaultDevice, setDefaultDevice] = useState("auto");
  const [paletteOpen, setPaletteOpen] = useState(
    () => new URLSearchParams(window.location.search).get("palette") === "1",
  );
  const sidebar = useSidebar();

  /** The only way pages change. Keeping one entry point is what makes direction reliable:
   *  every caller (rail, shortcut, swipe, palette) goes through the same comparison. */
  const go = useCallback((next: PageId) => {
    const from = NAV.findIndex((item) => item.id === current.current);
    const to = NAV.findIndex((item) => item.id === next);
    if (to === from) return;
    setDirection(to > from ? "forward" : "back");
    current.current = next;
    setPage(next);
  }, []);

  const step = useCallback(
    (delta: 1 | -1) => {
      const index = NAV.findIndex((item) => item.id === current.current);
      const next = NAV[index + delta];
      if (next) go(next.id);
    },
    [go],
  );

  // One place that re-reads everything the shell depends on. The settings page calls it after a
  // save, so changing the engine path updates the sidebar dot without a restart.
  const refresh = useCallback(() => {
    void resolveEngine().then(setEngine);
    void getSettings()
      .then((view) => setDefaultDevice(view.default_device))
      .catch(() => {
        /* browser preview has no shell; keep the defaults */
      });
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // Subscribe once, at the shell: a page switch must not drop a running stream.
  //
  // The unsubscribe handle only arrives after a promise, which means the cleanup can run first —
  // and it does, every time, under StrictMode's mount/unmount/mount. Pushing into a `disposers`
  // array that the cleanup already walked past leaks the listener and, worse, leaves a second one
  // behind on the next mount: every epoch event would then be folded twice, doubling the loss
  // series and the log. So the handle has to be registered against the liveness flag, not a list.
  useEffect(() => {
    let alive = true;
    const disposers: Array<() => void> = [];
    const keep = (off: () => void) => {
      if (alive) disposers.push(off);
      else off();
    };
    void onEngineEvent(applyEvent).then(keep);
    void onEngineLog((line) => pushLog(line)).then(keep);
    return () => {
      alive = false;
      for (const dispose of disposers) dispose();
    };
  }, []);

  // Two-finger horizontal swipe on the content area moves one page. Vertical scrolling is
  // untouched — the hook refuses anything that is not decisively horizontal.
  const swipeRef = useSwipe({
    onSwipeLeft: () => step(1),
    onSwipeRight: () => step(-1),
  });

  // Drop a folder anywhere on the window to scan it. Registered here rather than on the
  // dataset page because the gesture is unambiguous from any screen, and having to
  // navigate first would make people drop it, see nothing, and try the dialog instead.
  //
  // Scanning is slow on a large tree, so a second drop can easily land before the first
  // resolves. Sequencing keeps the answer on screen matching the folder that was last dropped —
  // without it a slow scan of A finishes after a fast scan of B and silently overwrites it.
  const scanSeq = useRef(0);
  const onDrop = useCallback(
    async (paths: string[]) => {
      const target = paths[0];
      if (!target) return;
      const seq = ++scanSeq.current;
      try {
        const summary = await scanDataset(target);
        if (seq !== scanSeq.current) return;
        setState({ dataset: summary });
        go("dataset");
        toast(
          `已扫描：${summary.train_images} 张训练图 · ${summary.classes.length} 类`,
          "ok",
        );
      } catch (cause) {
        if (seq !== scanSeq.current) return;
        toast(typeof cause === "string" ? cause : String(cause), "danger", 5000);
      }
    },
    [go],
  );
  const dropping = useWindowDrop(onDrop);

  // Keyboard gestures. Ctrl+1..4 mirrors the sidebar order; Ctrl+B is the convention every
  // editor uses for "collapse the panel"; Ctrl+K opens the command palette.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      // Alt+←/→ is the browser-history gesture on this platform, and in a frameless app
      // there is no history to go back to — so it is free to mean "previous/next page".
      if (event.altKey && !event.ctrlKey) {
        if (event.key === "ArrowLeft") {
          event.preventDefault();
          step(-1);
          return;
        }
        if (event.key === "ArrowRight") {
          event.preventDefault();
          step(1);
          return;
        }
      }

      if (!event.ctrlKey || event.altKey) return;
      const key = event.key.toLowerCase();

      if (key === "k") {
        event.preventDefault();
        setPaletteOpen((open) => !open);
        return;
      }
      if (key === "b") {
        event.preventDefault();
        sidebar.toggle();
        return;
      }
      const index = Number(event.key) - 1;
      if (Number.isInteger(index) && index >= 0 && index < NAV.length) {
        event.preventDefault();
        go(NAV[index].id);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [sidebar, go]);

  // Escape closes the palette even when focus has drifted out of the input.
  useEffect(() => {
    if (!paletteOpen) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setPaletteOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [paletteOpen]);

  const actions = useMemo<PaletteAction[]>(
    () => [
      ...NAV.map((item, index) => ({
        id: item.id,
        label: item.label,
        hint: item.pinned ? "页面" : item.hint,
        icon: item.icon,
        keys: `Ctrl+${index + 1}`,
        run: () => go(item.id),
      })),
      {
        id: "start",
        label: "开始训练",
        hint: "跳到训练页",
        icon: Play,
        run: () => go("train"),
      },
      {
        id: "sidebar",
        label: sidebar.collapsed ? "展开侧边栏" : "收起侧边栏",
        hint: "Ctrl+B",
        icon: PanelLeft,
        run: sidebar.toggle,
      },
    ],
    [go, sidebar],
  );

  return (
    <div className="flex h-full flex-col bg-canvas">
      <TitleBar
        collapsed={sidebar.collapsed}
        width={sidebar.width}
        dragging={sidebar.dragging}
        onToggle={sidebar.toggle}
        onOpenPalette={() => setPaletteOpen(true)}
      />

      <div className="flex min-h-0 flex-1">
        <Sidebar
          page={page}
          onSelect={go}
          engine={engine}
          collapsed={sidebar.collapsed}
          width={sidebar.width}
          dragging={sidebar.dragging}
          onStartResize={sidebar.startResize}
          onResetWidth={sidebar.resetWidth}
        />

        <main ref={swipeRef} className="min-w-0 flex-1 overflow-y-auto">
          {/* key=page replays the enter animation on every switch, without a router */}
          <div
            key={page}
            className={
              "mx-auto max-w-[1080px] px-10 py-8 " +
              (direction === "forward"
                ? "animate-page-in-right"
                : "animate-page-in-left")
            }
          >
            {page === "dataset" && <DatasetPage />}
            {page === "train" && (
              <TrainPage
                defaultDevice={defaultDevice}
                onStarted={() => go("monitor")}
                onGoDataset={() => go("dataset")}
              />
            )}
            {page === "monitor" && <MonitorPage />}
            {page === "settings" && <SettingsPage onEngineChanged={refresh} />}
          </div>
        </main>
      </div>

      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        actions={actions}
      />

      <Toaster />

      {/* Drop target. Full-window and pointer-transparent, so it never interferes with the
          drop itself — it only has to say "let go here". */}
      {dropping ? (
        <div className="pointer-events-none fixed inset-0 z-50 flex animate-fade-in items-center justify-center p-6">
          <div className="absolute inset-6 rounded-[18px] border-2 border-dashed border-ink/35 bg-canvas/80 backdrop-blur-[3px]" />
          <div className="relative flex animate-pop-in flex-col items-center gap-3 rounded-[14px] bg-ink px-7 py-6 text-canvas shadow-[0_24px_60px_-24px_rgba(10,10,10,0.6)]">
            <FolderDown size={26} strokeWidth={2.2} />
            <div className="text-[15px] font-bold">松开以扫描这个目录</div>
            <div className="text-[12px] font-medium text-canvas/60">
              需要 dataset.yaml 与 images / labels
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
