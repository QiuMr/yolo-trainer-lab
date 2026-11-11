"""Generate every ONNX skeleton that ships with the installer.

The skeletons are build-time artifacts: they are produced here, committed into the desktop app's
resources, and filled at run time with no Python. Adding an architecture is a new row in TARGETS —
nothing else changes, because the fill step is driven entirely by the manifest.

  python tools/onnx_skeleton/generate_all.py \
      --ultralytics research/ultralytics --contracts research/montgomery/tools/onnx \
      --out desktop/src-tauri/resources/onnx
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import time
from pathlib import Path

# (family, task, cfg, model_id). Only scales the product actually recommends are bundled: every
# entry costs installer size and a torch export at build time.
TARGETS = [
    ("yolov8", "detect", "yolov8n.yaml", "yolov8n"),
    ("yolov8", "detect", "yolov8s.yaml", "yolov8s"),
    ("yolo11", "detect", "yolo11n.yaml", "yolo11n"),
    ("yolo11", "detect", "yolo11s.yaml", "yolo11s"),
    ("yolo26", "detect", "yolo26n.yaml", "yolo26n"),
    ("yolo26", "detect", "yolo26s.yaml", "yolo26s"),
    ("yolov8", "classify", "yolov8n-cls.yaml", "yolov8n-cls"),
    ("yolo26", "classify", "yolo26n-cls.yaml", "yolo26n-cls"),
    ("yolov8", "segment", "yolov8n-seg.yaml", "yolov8n-seg"),
    ("yolo26", "segment", "yolo26n-seg.yaml", "yolo26n-seg"),
]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ultralytics", required=True, type=Path)
    parser.add_argument("--contracts", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--only", nargs="*", help="limit to these model ids")
    args = parser.parse_args()

    here = Path(__file__).with_name("generate.py")
    failures = []
    for family, task, cfg, model_id in TARGETS:
        if args.only and model_id not in args.only:
            continue
        started = time.time()
        print(f"--- {model_id} ({family}/{task})", flush=True)
        result = subprocess.run(
            [
                args.python,
                str(here),
                "--ultralytics", str(args.ultralytics),
                "--contracts", str(args.contracts),
                "--family", family,
                "--task", task,
                "--cfg", cfg,
                "--model-id", model_id,
                "--out", str(args.out),
            ],
            capture_output=True,
            text=True,
        )
        duration = time.time() - started
        if result.returncode != 0:
            # A single unsupported combination must not hide the rest of the matrix.
            detail = (result.stderr or result.stdout).strip().splitlines()
            failures.append((model_id, detail[-1] if detail else "unknown error"))
            print(f"    FAILED after {duration:.0f}s: {failures[-1][1]}", flush=True)
            continue
        tail = (result.stdout or "").strip().splitlines()
        print(f"    {tail[-1] if tail else 'ok'}  ({duration:.0f}s)", flush=True)

    if failures:
        print("\nfailed:", *[f"{name}: {error}" for name, error in failures], sep="\n  ")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
