"""Drive the running app through the WebView2 debugging port.

Why this exists: from outside the process the UI can only be driven by OS-level clicks, and any
flow that opens a native dialog (dataset picker) is a dead end. With `additionalBrowserArgs =
--remote-debugging-port=9333` in tauri.conf.json, the page is scriptable over CDP, so a command can
be invoked directly and the resulting UI can still be screenshotted normally.

Dev-only by design: the flag must NOT ship, because any local process could then drive the app.

Usage:
    python spikes/qa/cdp.py targets
    python spikes/qa/cdp.py eval "<js expression>"
    python spikes/qa/cdp.py train            # start a 1-epoch CPU run on the tiny dataset
    python spikes/qa/cdp.py open monitor     # click a sidebar nav item by its label
"""

import json
import sys

import websocket

DEBUG_PORT = 9333
NAV_LABELS = ("数据集", "训练", "监控", "设置")


def page_target() -> str:
    import urllib.request

    with urllib.request.urlopen(f"http://127.0.0.1:{DEBUG_PORT}/json/list", timeout=5) as response:
        targets = json.load(response)
    for target in targets:
        if target.get("type") == "page" and "localhost:1420" in target.get("url", ""):
            return target["webSocketDebuggerUrl"]
    raise SystemExit(f"no page target: {[t.get('url') for t in targets]}")


def evaluate(expression: str, await_promise: bool = True) -> str:
    socket = websocket.create_connection(page_target(), timeout=30)
    try:
        socket.send(
            json.dumps(
                {
                    "id": 1,
                    "method": "Runtime.evaluate",
                    "params": {
                        "expression": expression,
                        "awaitPromise": await_promise,
                        "returnByValue": True,
                        "userGesture": True,
                    },
                }
            )
        )
        while True:
            message = json.loads(socket.recv())
            if message.get("id") == 1:
                result = message.get("result", {})
                if "exceptionDetails" in result:
                    return "EXCEPTION: " + json.dumps(
                        result["exceptionDetails"], ensure_ascii=False
                    )
                return json.dumps(result.get("result", {}).get("value"), ensure_ascii=False)
    finally:
        socket.close()


def main() -> None:
    command = sys.argv[1]

    if command == "targets":
        import urllib.request

        with urllib.request.urlopen(f"http://127.0.0.1:{DEBUG_PORT}/json/list", timeout=5) as r:
            for target in json.load(r):
                print(target.get("type"), target.get("url"))
        return

    if command == "eval":
        print(evaluate(sys.argv[2]))
        return

    if command == "train":
        expression = """
        (async () => {
          const internals = window.__TAURI_INTERNALS__;
          try {
            await internals.invoke('start_training', {
              request: {
                architecture: 'yolo26n',
                dataset: 'E:\\\\工作区\\\\yolo-trainer-lab\\\\spikes\\\\tiny-dataset',
                epochs: 1,
                batch: 2,
                imgsz: 320,
                device: 'cpu',
              },
            });
            return 'started';
          } catch (error) {
            return 'failed: ' + JSON.stringify(error);
          }
        })()
        """
        print(evaluate(expression))
        return

    if command == "open":
        label = sys.argv[2]
        if label not in NAV_LABELS:
            raise SystemExit(f"unknown page: {label}")
        expression = f"""
        (() => {{
          const button = [...document.querySelectorAll('aside button')]
            .find((node) => node.textContent.includes('{label}'));
          if (!button) return 'not found';
          button.click();
          return 'clicked ' + button.textContent.trim().slice(0, 12);
        }})()
        """
        print(evaluate(expression))
        return

    raise SystemExit(f"unknown command: {command}")


if __name__ == "__main__":
    main()
