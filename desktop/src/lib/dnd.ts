import { useEffect, useState } from "react";
import { isTauri } from "./api";

/* ─────────────────────────────────────────────────────────────────────────────
   Window drop target.

   Dragging a folder from Explorer onto the window is the shortest path to "scan this":
   no dialog, no nested tree to click through, and the folder is already the thing the
   user was looking at. The gesture needs feedback on the way in, which is why this hook
   exposes an `over` flag — a drop zone with no hover state makes people let go too early.

   Registered at the shell, not on the dataset page: dropping a folder is unambiguous from
   any screen, and being told "go to the dataset tab first" would defeat the gesture.
   ───────────────────────────────────────────────────────────────────────────── */

type DropPayload =
  | { type: "enter" | "over"; paths: string[] }
  | { type: "drop"; paths: string[] }
  | { type: "leave" };

export function useWindowDrop(onDrop: (paths: string[]) => void) {
  const [over, setOver] = useState(false);

  useEffect(() => {
    if (!isTauri) return;
    let dispose: (() => void) | undefined;
    let alive = true;

    void import("@tauri-apps/api/window").then(async ({ getCurrentWindow }) => {
      if (!alive) return;
      const off = await getCurrentWindow().onDragDropEvent((event) => {
        const payload = event.payload as DropPayload;
        if (payload.type === "enter" || payload.type === "over") {
          setOver(true);
        } else if (payload.type === "leave") {
          setOver(false);
        } else {
          setOver(false);
          const paths = payload.paths.filter(Boolean);
          if (paths.length > 0) onDrop(paths);
        }
      });
      if (alive) dispose = off;
      else off();
    });

    return () => {
      alive = false;
      dispose?.();
    };
  }, [onDrop]);

  return over;
}
