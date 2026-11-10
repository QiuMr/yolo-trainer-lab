"""End-to-end probe of the local control service, over plain HTTP.

Run against a live app (debug or packaged) — it starts a real CPU training run, so it is not part
of `cargo test`. Everything it asserts is observable from the HTTP responses alone, which is the
point: an AI client has exactly the same view as this script.

    python spikes/qa/probe_control.py
"""

import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

BASE = "http://127.0.0.1:47821"
ROOT = Path(__file__).resolve().parents[2]
DATASET = ROOT / "spikes" / "tiny-dataset"


def token() -> str:
    appdata = Path.home() / "AppData" / "Roaming" / "com.yolotrainer.desktop"
    return (appdata / "control-token").read_text(encoding="utf-8").strip()


# An empty ProxyHandler ignores http_proxy/HTTPS_PROXY from the environment. Without this, a
# machine running a local proxy forwards 127.0.0.1 requests to it and the probe fails with an
# upstream error that looks exactly like "the service is down". Real AI clients hit the same trap.
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def call(method: str, path: str, body=None, auth: str | None = None, timeout=30):
    data = None if body is None else json.dumps(body).encode("utf-8")
    request = urllib.request.Request(BASE + path, data=data, method=method)
    if auth:
        request.add_header("Authorization", f"Bearer {auth}")
    if data is not None:
        request.add_header("Content-Type", "application/json")
    try:
        with OPENER.open(request, timeout=timeout) as response:
            return response.status, json.loads(response.read().decode("utf-8") or "null")
    except urllib.error.HTTPError as error:
        raw = error.read().decode("utf-8", "replace")
        return error.code, json.loads(raw) if raw.startswith("{") else {"raw": raw}


def main() -> int:
    secret = token()
    failures = []

    def check(name, condition, detail=""):
        status = "PASS" if condition else "FAIL"
        print(f"[{status}] {name}{(' · ' + str(detail)) if detail else ''}")
        if not condition:
            failures.append(name)

    code, _ = call("GET", "/health")
    check("未带令牌返回 401", code == 401, code)

    code, health = call("GET", "/health", auth=secret)
    check("带令牌 /health 返回 200", code == 200, code)
    check("/health 报出引擎路径", bool(health.get("engine")), health.get("engine"))

    code, state = call("GET", "/state", auth=secret)
    check("/state 返回 idle", code == 200 and state.get("status") == "idle", state.get("status"))

    code, scan = call("GET", f"/datasets/scan?path={urllib.parse.quote(str(DATASET))}", auth=secret)
    ok = code == 200 and scan["dataset"]["train_images"] == 4
    check("扫描数据集（中文路径）", ok, scan.get("dataset", {}).get("train_images"))

    code, accepted = call(
        "POST",
        "/train",
        {"dataset": str(DATASET), "epochs": 2, "batch": 2, "imgsz": 320, "device": "cpu"},
        auth=secret,
    )
    check("POST /train 返回 202", code == 202, code)
    check("accepted 回显参数", accepted.get("accepted", {}).get("epochs") == 2, accepted)

    code, conflict = call(
        "POST", "/train", {"dataset": str(DATASET), "epochs": 1}, auth=secret
    )
    check("并发训练返回 409", code == 409, code)

    cursor = 0
    seen_epochs = []
    deadline = time.time() + 240
    final = {}
    while time.time() < deadline:
        code, payload = call("GET", f"/state?since={cursor}", auth=secret)
        if code != 200:
            check("轮询 /state", False, code)
            break
        cursor = payload.get("cursor", cursor)
        for event in payload.get("events", []):
            if event.get("type") == "epoch":
                seen_epochs.append(event["epoch"])
                print(
                    f"       epoch {event['epoch']}/{event.get('epochs')} "
                    f"loss={event.get('loss'):.2f} {event.get('elapsed_s'):.2f}s"
                )
        final = payload.get("state", final)
        if final.get("status") in ("done", "failed"):
            break
        time.sleep(0.6)

    check("训练跑完 status=done", final.get("status") == "done", final.get("status"))
    check("收到 2 个 epoch 事件", seen_epochs == [1, 2], seen_epochs)
    check("backend 是 cpu", final.get("backend") == "cpu", final.get("backend"))
    check("best 产物存在", bool(final.get("best")) and Path(final["best"]).is_file(), final.get("best"))
    check("loss 单调下降有记录", final.get("last_loss") is not None, final.get("last_loss"))
    check("exit_code=0", final.get("exit_code") == 0, final.get("exit_code"))

    # A stop must be immediate and must surface as `failed`, not as a silent `done`.
    call("POST", "/train", {"dataset": str(DATASET), "epochs": 200, "batch": 2, "imgsz": 320, "device": "cpu"}, auth=secret)
    time.sleep(1.5)
    code, stopped = call("POST", "/stop", auth=secret)
    check("POST /stop 返回 200", code == 200, code)
    deadline = time.time() + 30
    while time.time() < deadline:
        _, payload = call("GET", "/state", auth=secret)
        if payload.get("status") in ("failed", "idle", "done"):
            break
        time.sleep(0.4)
    check("中止后不是 running", payload.get("status") != "running", payload.get("status"))

    print()
    if failures:
        print(f"{len(failures)} 项失败: {failures}")
        return 1
    print("全部通过")
    return 0


if __name__ == "__main__":
    sys.exit(main())
