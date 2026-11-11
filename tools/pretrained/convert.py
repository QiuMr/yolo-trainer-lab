"""Convert official Ultralytics .pt checkpoints into Montgomery .bpk artifacts.

Development-time only: this is the one step that needs PyTorch, and it happens on the build
machine. The shipped app never sees a .pt file — it reads the .bpk that lands here.

Usage:
    python convert.py --archs yolov8n yolov8s ... [--python <interpreter>]

Outputs `<arch>.bpk` next to each `<arch>.pt`. The intermediate tensor state is deleted because
it is roughly twice the size of the checkpoint and has no reuse.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).parent


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--archs", nargs="+", required=True)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--engine", default=str(HERE.parent.parent / "artifacts/target-dist/release/montgomery.exe"))
    parser.add_argument("--converter", default=str(HERE.parent.parent / "research/montgomery/tools/export_checkpoint_state.py"))
    parser.add_argument("--out", default=str(HERE / "bpk"))
    args = parser.parse_args()

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    failures = []
    for arch in args.archs:
        pt = HERE / f"{arch}.pt"
        if not pt.exists():
            failures.append(f"{arch}: missing {pt.name}")
            continue
        state = out / f"{arch}-state.pt"
        bpk = out / f"{arch}.bpk"
        if bpk.exists():
            print(f"{arch}: already packed, skipping", flush=True)
            continue

        step = subprocess.run(
            [args.python, args.converter, str(pt), str(state)],
            capture_output=True,
            text=True,
        )
        if step.returncode != 0:
            failures.append(f"{arch}: convert failed: {step.stderr.strip()[-300:]}")
            continue

        step = subprocess.run(
            [
                args.engine,
                "pack-weights",
                "--architecture",
                arch,
                "--state",
                str(state),
                "--output",
                str(bpk),
            ],
            capture_output=True,
            text=True,
        )
        state.unlink(missing_ok=True)
        if step.returncode != 0:
            failures.append(f"{arch}: pack failed: {step.stderr.strip()[-300:]}")
            continue
        print(f"{arch}: {bpk.stat().st_size / 1048576:.2f} MiB", flush=True)

    if failures:
        print("\nFAILURES:", file=sys.stderr)
        for line in failures:
            print("  " + line, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
