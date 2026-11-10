"""Screenshot the model picker and prove the three rows actually drive each other.

The picker is the only control whose state is not readable from a single element: the id at the
bottom is a product of three independent picks, and the family row changes shape with the task.
So this script asserts the *combinations*, not the pixels — the screenshot is only there to
check spacing and weight afterwards.

Usage:
    python spikes/qa/shot_model_picker.py                 # default port 5200
    python spikes/qa/shot_model_picker.py 5200 out.png
"""

import base64
import json
import subprocess
import sys
import time
import urllib.request

import websocket

EDGE = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
PROFILE = r"E:\工作区\yolo-trainer-lab\artifacts\edge-cdp-profile-picker"


def first_page_url(port: int) -> str:
    for _ in range(60):
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list", timeout=1) as r:
                for target in json.load(r):
                    if target.get("type") == "page":
                        return target["webSocketDebuggerUrl"]
        except Exception:
            pass
        time.sleep(0.25)
    raise SystemExit("no CDP target")


class Session:
    def __init__(self, port: int) -> None:
        self.seq = 0
        self.ws = websocket.create_connection(first_page_url(port), timeout=20)

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
            "Runtime.evaluate", expression=expression, returnByValue=True, awaitPromise=True
        )
        if result.get("exceptionDetails"):
            raise RuntimeError(json.dumps(result["exceptionDetails"], ensure_ascii=False))
        return result["result"].get("value")

    def click(self, x: float, y: float):
        for kind, buttons in (("mousePressed", 1), ("mouseReleased", 0)):
            self.call(
                "Input.dispatchMouseEvent",
                type=kind,
                x=x,
                y=y,
                button="left",
                buttons=buttons,
                clickCount=1,
            )
            time.sleep(0.05)
        time.sleep(0.35)

    def click_attr(self, attribute: str, value: str):
        box = self.js(
            f"""(() => {{
                const el = document.querySelector('[{attribute}="{value}"]');
                if (!el) return null;
                const r = el.getBoundingClientRect();
                return {{x: r.x + r.width / 2, y: r.y + r.height / 2}};
            }})()"""
        )
        if not box:
            raise SystemExit(f"not found: {attribute}={value}")
        self.click(box["x"], box["y"])

    def shot(self, path: str, clip: dict | None = None):
        params = {"format": "png"}
        if clip:
            params["clip"] = {**clip, "scale": 1}
        data = self.call("Page.captureScreenshot", **params)["data"]
        with open(path, "wb") as fh:
            fh.write(base64.b64decode(data))


def read_state(session: Session) -> dict:
    """The picker's whole observable state, read from the DOM by role rather than by index."""
    return session.js(
        """(() => {
            const card = [...document.querySelectorAll('h3')]
                .find(h => h.textContent.trim() === '模型').closest('div.rounded-\\\\[14px\\\\]');
            const rows = [...card.querySelectorAll('div.mt-5')];
            const picked = (row) => {
                const btn = [...row.querySelectorAll('button')]
                    .find(b => b.getAttribute('aria-pressed') === 'true');
                return btn ? btn.textContent.trim() : null;
            };
            const families = rows[1] ? [...rows[1].querySelectorAll('button')] : [];
            const scales = rows[2] ? [...rows[2].querySelectorAll('button')] : [];
            return {
                task: picked(rows[0]),
                family: picked(rows[1]),
                scale: picked(rows[2]),
                familyCount: families.length,
                familyLabels: families.map(b => b.textContent.trim()),
                scaleCount: scales.length,
                scaleLabels: scales.map(b => b.textContent.trim()),
                id: card.querySelector('code') ? card.querySelector('code').textContent.trim() : null,
                blocked: !!card.querySelector('.border-warn\\\\/25'),
                card: (() => { const r = card.getBoundingClientRect();
                    return {x: r.x, y: r.y, width: r.width, height: r.height}; })(),
            };
        })()"""
    )


def main() -> None:
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 5200
    out = sys.argv[2] if len(sys.argv) > 2 else r"E:\工作区\yolo-trainer-lab\artifacts\qa-model-picker.png"
    url = f"http://[::1]:{port}/?page=train&demo=1"

    proc = subprocess.Popen(
        [
            EDGE,
            "--headless=new",
            "--disable-gpu",
            f"--remote-debugging-port=9335",
            f"--user-data-dir={PROFILE}",
            "--window-size=1280,940",
            "--force-prefers-reduced-motion",
            url,
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        session = Session(9335)
        session.call("Page.enable")
        session.call("Runtime.enable")
        time.sleep(2.2)

        checks: list[str] = []
        baseline = read_state(session)
        checks.append(f"默认 {baseline['task']} / {baseline['family']} / {baseline['scale']} -> {baseline['id']}")
        assert baseline["id"] == "yolo26n", baseline

        session.click_attr("data-model-task", "segment")
        split = read_state(session)
        checks.append(
            f"切到分割: 家族 {split['familyCount']} 个 {split['familyLabels']} -> {split['id']}"
        )
        assert split["id"].endswith("-seg"), split
        assert split["familyCount"] == 3, split

        session.click_attr("data-model-family", "yolov8")
        v8 = read_state(session)
        checks.append(f"选 YOLOv8: -> {v8['id']} 规模 {v8['scaleCount']} 档")
        assert v8["id"] == "yolov8n-seg", v8

        session.click_attr("data-model-scale", "x")
        biggest = read_state(session)
        checks.append(f"选最准: -> {biggest['id']}")
        assert biggest["id"] == "yolov8x-seg", biggest

        session.click_attr("data-model-task", "detect")
        back = read_state(session)
        checks.append(f"回检测: 家族 {back['familyCount']} 个，沿用 YOLOv8 -> {back['id']}")
        assert back["id"] == "yolov8x", back

        session.click_attr("data-model-family", "yolox")
        yolox = read_state(session)
        checks.append(
            f"选 YOLOX（无 x 之前的 n 档）: 规模 {yolox['scaleLabels']} -> {yolox['id']}"
        )
        assert yolox["scaleCount"] == 6, yolox
        assert yolox["id"].startswith("yolox-"), yolox

        session.click_attr("data-model-task", "classify")
        classify = read_state(session)
        checks.append(f"切到分类: -> {classify['id']} 阻断提示={classify['blocked']}")
        assert classify["blocked"], classify

        session.click_attr("data-model-task", "detect")
        session.click_attr("data-model-family", "yolo26")
        session.click_attr("data-model-scale", "n")
        final = read_state(session)
        checks.append(f"还原: -> {final['id']}")

        card = final["card"]
        session.shot(
            out,
            {
                "x": max(0, card["x"] - 12),
                "y": max(0, card["y"] - 12),
                "width": min(1280, card["width"] + 24),
                "height": card["height"] + 24,
            },
        )
        session.shot(out.replace(".png", "-full.png"))
        for line in checks:
            print("OK", line)
        print("shot:", out)
    finally:
        proc.terminate()


if __name__ == "__main__":
    main()
