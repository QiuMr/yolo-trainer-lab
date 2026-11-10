"""Drive the browser preview over CDP to verify pointer gestures.

The desktop app can only be driven by OS-level input injection, which is blocked whenever the
app runs elevated (UIPI silently drops the events) — and a blocked click looks exactly like a
broken gesture. Chromium in the preview build dispatches the same pointer/mouse/wheel events
that the app's hooks listen for, and lets us read the result back out of the DOM, so a gesture
can be verified as *state* instead of as pixels.

Usage:
    python spikes/qa/cdp_browser.py            # runs the whole gesture check
    python spikes/qa/cdp_browser.py nav        # one group: nav | scrub | resize | swipe | palette
"""

import json
import subprocess
import sys
import time
import urllib.request

import websocket

PORT = 9334
URL = "http://localhost:5199/?page=train"
EDGE = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
PROFILE = r"E:\工作区\yolo-trainer-lab\artifacts\edge-cdp-profile"


class Session:
    def __init__(self) -> None:
        self.seq = 0
        self.ws = websocket.create_connection(self._target(), timeout=15)

    @staticmethod
    def _target() -> str:
        for _ in range(40):
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json/list", timeout=1) as r:
                    for target in json.load(r):
                        if target.get("type") == "page":
                            return target["webSocketDebuggerUrl"]
            except Exception:
                pass
            time.sleep(0.25)
        raise SystemExit("no CDP target")

    def call(self, method: str, **params):
        self.seq += 1
        self.ws.send(json.dumps({"id": self.seq, "method": method, "params": params}))
        while True:
            message = json.loads(self.ws.recv())
            if message.get("id") == self.seq:
                if "error" in message:
                    raise RuntimeError(message["error"])
                return message.get("result", {})

    def js(self, expression: str):
        result = self.call(
            "Runtime.evaluate",
            expression=expression,
            returnByValue=True,
            awaitPromise=True,
        )
        if result.get("exceptionDetails"):
            raise RuntimeError(result["exceptionDetails"])
        return result["result"].get("value")

    def mouse(self, kind: str, x: float, y: float, buttons: int = 0, clicks: int = 1):
        self.call(
            "Input.dispatchMouseEvent",
            type=kind,
            x=x,
            y=y,
            button="left" if buttons or kind != "mouseMoved" else "none",
            buttons=buttons,
            clickCount=clicks,
        )

    def drag(self, x1: float, y1: float, x2: float, y2: float, steps: int = 24):
        self.mouse("mouseMoved", x1, y1)
        time.sleep(0.05)
        self.mouse("mousePressed", x1, y1, buttons=1)
        time.sleep(0.08)
        for i in range(1, steps + 1):
            self.mouse("mouseMoved", x1 + (x2 - x1) * i / steps, y1 + (y2 - y1) * i / steps, buttons=1)
            time.sleep(0.008)
        time.sleep(0.05)
        self.mouse("mouseReleased", x2, y2, buttons=0)
        time.sleep(0.25)

    def click(self, x: float, y: float):
        self.mouse("mousePressed", x, y, buttons=1)
        time.sleep(0.05)
        self.mouse("mouseReleased", x, y, buttons=0)
        time.sleep(0.3)

    def wheel(self, x: float, y: float, dx: float, dy: float = 0, times: int = 4):
        for _ in range(times):
            self.call(
                "Input.dispatchMouseEvent",
                type="mouseWheel",
                x=x,
                y=y,
                deltaX=dx,
                deltaY=dy,
            )
            time.sleep(0.03)
        time.sleep(0.45)

    def shot(self, path: str):
        data = self.call("Page.captureScreenshot", format="png")["data"]
        import base64

        with open(path, "wb") as fh:
            fh.write(base64.b64decode(data))


def centre(session: Session, script: str):
    box = session.js(script)
    if not box:
        raise SystemExit(f"element not found: {script}")
    return box["x"] + box["width"] / 2, box["y"] + box["height"] / 2


def find(session: Session, needle: str) -> dict:
    """Centre of the first element whose text is exactly `needle`."""
    return centre(
        session,
        f"""(() => {{
            const el = [...document.querySelectorAll('button,span,label')]
                .find(e => e.textContent.trim() === {json.dumps(needle)});
            if (!el) return null;
            const r = el.getBoundingClientRect();
            return {{x: r.x, y: r.y, width: r.width, height: r.height}};
        }})()""",
    )


def main() -> None:
    groups = sys.argv[1:] or ["nav", "scrub", "resize", "swipe", "palette"]
    proc = subprocess.Popen(
        [
            EDGE,
            "--headless=new",
            "--disable-gpu",
            f"--remote-debugging-port={PORT}",
            f"--user-data-dir={PROFILE}",
            "--window-size=1280,900",
            URL,
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        session = Session()
        session.call("Page.enable")
        session.call("Runtime.enable")
        time.sleep(1.6)

        results: list[str] = []

        if "nav" in groups:
            before = session.js("document.querySelector('h1').textContent")
            x, y = find(session, "数据集")
            session.click(x, y)
            after = session.js("document.querySelector('h1').textContent")
            pill = session.js(
                "getComputedStyle(document.querySelector('aside span[aria-hidden]')).transform"
            )
            results.append(f"nav click: {before} -> {after} (pill transform {pill})")
            print(results[-1], flush=True)

        if "scrub" in groups:
            x, y = find(session, "训练")
            session.click(x, y)
            session.js("document.querySelector('main').scrollTop = 0")
            handle = centre(
                session,
                """(() => {
                    const el = document.querySelector('span[title^="左右拖动调节"]');
                    if (!el) return null;
                    const r = el.getBoundingClientRect();
                    return {x: r.x, y: r.y, width: r.width, height: r.height};
                })()""",
            )
            x, y = handle
            session.drag(x, y, x + 80, y)
            value = session.js("document.querySelectorAll('input')[0].value")
            active = session.js(
                "(() => { const s = getComputedStyle(document.querySelectorAll('input')[0]);"
                " return s.borderColor + ' | ' + s.boxShadow; })()"
            )
            results.append(f"scrub: value={value} (100 + 80px/2 = 140 expected), style-after-release={active}")
            print(results[-1], flush=True)

        if "resize" in groups:
            width_before = session.js("document.querySelector('aside').getBoundingClientRect().width")
            handle = centre(
                session,
                """(() => {
                    const el = document.querySelector('[role="separator"]');
                    if (!el) return null;
                    const r = el.getBoundingClientRect();
                    return {x: r.x, y: r.y, width: r.width, height: r.height};
                })()""",
            )
            x, y = handle
            session.drag(x, 400, x + 60, 400)
            width_after = session.js("document.querySelector('aside').getBoundingClientRect().width")
            results.append(f"resize: {width_before} -> {width_after}")
            print(results[-1], flush=True)

        if "swipe" in groups:
            session.call("Runtime.evaluate", expression="window.location.search='?page=train'")
            time.sleep(1.2)
            before = session.js("document.querySelector('h1').textContent")
            session.wheel(600, 500, dx=40, times=5)
            after = session.js("document.querySelector('h1').textContent")
            results.append(f"swipe right (deltaX +40 x5): {before} -> {after}")
            print(results[-1], flush=True)

        if "palette" in groups:
            session.js("document.activeElement.blur()")
            session.call(
                "Input.dispatchKeyEvent",
                type="keyDown",
                key="k",
                code="KeyK",
                windowsVirtualKeyCode=75,
                modifiers=2,
            )
            session.call(
                "Input.dispatchKeyEvent",
                type="keyUp",
                key="k",
                code="KeyK",
                windowsVirtualKeyCode=75,
                modifiers=2,
            )
            time.sleep(0.4)
            open_state = session.js("!!document.querySelector('[role=\"dialog\"]')")
            results.append(f"ctrl+k opens palette: {open_state}")
            print(results[-1], flush=True)

            # Arrow down twice then Enter should run the third command.
            for _ in range(2):
                session.call(
                    "Input.dispatchKeyEvent",
                    type="rawKeyDown",
                    key="ArrowDown",
                    code="ArrowDown",
                    windowsVirtualKeyCode=40,
                )
                time.sleep(0.08)
            session.call(
                "Input.dispatchKeyEvent",
                type="rawKeyDown",
                key="Enter",
                code="Enter",
                windowsVirtualKeyCode=13,
            )
            time.sleep(0.6)
            page_after = session.js("document.querySelector('h1').textContent")
            results.append(f"palette enter -> page: {page_after}")
            print(results[-1], flush=True)

        print("\n".join(results))
        session.shot(r"E:\工作区\yolo-trainer-lab\artifacts\shots\cdp-final.png")
    finally:
        proc.terminate()


if __name__ == "__main__":
    main()
