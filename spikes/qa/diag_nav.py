"""Read the nav rail's geometry over CDP and drive a click on the pinned settings row.

Two defects reported by hand: the selection pill sits low inside its row, and clicking
"设置" appears to land back on 数据集. Both are state that can be measured from the DOM,
which is far more reliable than looking at a screenshot.
"""

import json
import subprocess
import sys
import time
import urllib.request

import websocket

PORT = 9350
EDGE = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
PROFILE = r"E:\工作区\yolo-trainer-lab\artifacts\edge-diag"


def pick(page="train", extra=""):
    url = f"http://localhost:5199/?page={page}{extra}"
    proc = subprocess.Popen(
        [
            EDGE,
            "--headless=new",
            "--disable-gpu",
            f"--remote-debugging-port={PORT}",
            f"--user-data-dir={PROFILE}",
            "--window-size=1280,900",
            url,
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    for _ in range(60):
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json/list", timeout=1) as r:
                for target in json.load(r):
                    if target.get("type") == "page":
                        return proc, target["webSocketDebuggerUrl"]
        except Exception:
            pass
        time.sleep(0.25)
    proc.terminate()
    raise SystemExit("no CDP target")


class S:
    def __init__(self, ws_url):
        self.ws = websocket.create_connection(ws_url, timeout=15)
        self.seq = 0

    def call(self, method, **params):
        self.seq += 1
        self.ws.send(json.dumps({"id": self.seq, "method": method, "params": params}))
        while True:
            m = json.loads(self.ws.recv())
            if m.get("id") == self.seq:
                if "error" in m:
                    raise RuntimeError(m["error"])
                return m.get("result", {})

    def js(self, expr):
        r = self.call("Runtime.evaluate", expression=expr, returnByValue=True)
        return r["result"].get("value")

    def click(self, x, y):
        for kind in ("mousePressed", "mouseReleased"):
            self.call(
                "Input.dispatchMouseEvent",
                type=kind,
                x=x,
                y=y,
                button="left",
                clickCount=1,
            )
            time.sleep(0.05)


GEO = """
(() => {
  const pill = document.querySelector('aside span[aria-hidden]');
  const rows = [...document.querySelectorAll('aside button[aria-current], aside button')];
  const out = { pill: null, rows: [] };
  if (pill) {
    const r = pill.getBoundingClientRect();
    out.pill = { top: r.top, bottom: r.bottom, height: r.height, left: r.left, right: r.right };
    out.pillStyle = { top: getComputedStyle(pill).top, transform: getComputedStyle(pill).transform };
  }
  for (const b of rows) {
    const r = b.getBoundingClientRect();
    out.rows.push({
      label: b.textContent.trim().slice(0, 6),
      active: b.getAttribute('aria-current'),
      top: r.top, bottom: r.bottom, height: r.height,
      cx: r.left + r.width / 2, cy: r.top + r.height / 2,
    });
  }
  const col = document.querySelector('aside > div');
  const cs = getComputedStyle(col);
  out.col = {
    pt: cs.paddingTop, px: cs.paddingLeft,
    rectTop: col.getBoundingClientRect().top,
  };
  const wrap = document.querySelector('aside nav > div');
  out.wrap = { offsetTop: wrap.offsetTop, offsetParent: wrap.offsetParent && wrap.offsetParent.className.slice(0, 40) };
  out.h1 = document.querySelector('h1')?.textContent;
  return JSON.stringify(out);
})()
"""


def main():
    page = sys.argv[1] if len(sys.argv) > 1 else "train"
    extra = sys.argv[2] if len(sys.argv) > 2 else ""
    proc, url = pick(page, extra)
    s = S(url)
    time.sleep(1.8)

    geo = json.loads(s.js(GEO))
    print("page:", geo["h1"])
    print("col:", geo["col"], "wrap:", geo["wrap"])
    print("pill:", geo["pill"], geo["pillStyle"])
    for row in geo["rows"]:
        print("  row:", row)
    if geo["pill"]:
        for row in geo["rows"]:
            if row["active"]:
                print(
                    "delta pill.top - row.top =",
                    round(geo["pill"]["top"] - row["top"], 2),
                    "| pill.bottom - row.bottom =",
                    round(geo["pill"]["bottom"] - row["bottom"], 2),
                )

    # Click the pinned settings row, then read back what the shell thinks is open.
    settings_row = next((r for r in geo["rows"] if r["label"].startswith("设置")), None)
    if settings_row is None:
        print("no settings row found")
        proc.terminate()
        return
    print("\nclicking settings at", settings_row["cx"], settings_row["cy"])
    s.click(settings_row["cx"], settings_row["cy"])
    time.sleep(1.0)
    after = json.loads(s.js(GEO))
    print("page after click:", after["h1"])
    print("active row:", [r["label"] for r in after["rows"] if r["active"]])
    print("pill after:", after["pill"])
    print("row count after:", len(after["rows"]))
    proc.terminate()


if __name__ == "__main__":
    main()
