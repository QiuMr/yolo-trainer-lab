"""Generate every ONNX skeleton that ships with the installer.

The skeletons are build-time artifacts: they are produced here, committed into the desktop app's
resources, and filled at run time with no Python. Adding an architecture is a new name in the
matrix below — nothing else changes, because the fill step is driven entirely by the manifest.

The matrix is deliberately smaller than the engine's capability. montgomery can train 63
architectures; the product offers 3 families x 3 tasks x 5 scales = 45, and ships exactly those.
YOLOv10 / YOLO12 / YOLOX / YOLOv3-tinyu stay trainable through the CLI but are absent from both
the picker and this bundle, so no skeleton is produced for them (docs/DECISIONS.md D028).
Skeletons for architectures that are no longer listed are pruned from the output directory, so
re-running this after changing the matrix is the whole update procedure.

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

# Ordered the way the picker shows them, cheapest first is not the point — these are just names.
FAMILIES = ["yolov8", "yolo11", "yolo26"]

# (task name passed to generate.py, id suffix). Detection has no suffix, which is why the suffix
# lives in this table instead of being derived from the task name.
TASKS = [("detect", ""), ("segment", "-seg"), ("classify", "-cls")]

# Every bundled family uses the same ladder, so a scale is a name and nothing more. A second
# ladder would have to be spelled out per family.
SCALES = ["n", "s", "m", "l", "x"]


def targets() -> list[tuple[str, str, str, str]]:
    """(family, task, cfg, model_id) for every skeleton the installer ships.

    `cfg` happens to always equal `<model_id>.yaml` across these three families and three tasks:
    yolov8n / yolov8n-seg / yolov8n-cls are all real filenames in ultralytics' cfg/models tree.
    """
    rows = []
    for family in FAMILIES:
        for task, suffix in TASKS:
            for scale in SCALES:
                model_id = f"{family}{scale}{suffix}"
                rows.append((family, task, f"{model_id}.yaml", model_id))
    return rows


def prune(out: Path, keep: set[str], dry_run: bool) -> int:
    """Delete skeletons and manifests whose architecture is no longer in the matrix.

    Restricted to `<model_id>.onnx` / `<model_id>.json` pairs so an unrelated file dropped into
    the resources directory survives. Returns the number of files removed.
    """
    removed = 0
    for path in sorted(out.iterdir()):
        if path.suffix not in {".onnx", ".json"} or path.stem in keep:
            continue
        print(f"    prune {path.name}")
        removed += 1
        if not dry_run:
            path.unlink()
    return removed


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ultralytics", required=True, type=Path)
    parser.add_argument("--contracts", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--only", nargs="*", help="limit to these model ids")
    parser.add_argument(
        "--keep-stale",
        action="store_true",
        help="leave skeletons from architectures that are no longer in the matrix in place",
    )
    args = parser.parse_args()

    here = Path(__file__).with_name("generate.py")
    rows = targets()
    ids = {row[3] for row in rows}
    if len(ids) != len(rows):
        # Two rows producing the same filename would silently overwrite each other.
        duplicates = sorted({row[3] for row in rows if [r[3] for r in rows].count(row[3]) > 1})
        print(f"duplicate model ids in matrix: {duplicates}", file=sys.stderr)
        return 2

    selected = [row for row in rows if not args.only or row[3] in args.only]
    print(f"matrix: {len(rows)} skeletons, {len(selected)} selected", flush=True)

    failures = []
    for family, task, cfg, model_id in selected:
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

    # Pruning only runs on a full build: with --only the other architectures are simply not
    # regenerated this time, and treating that as "removed" would eat the bundle.
    if not args.only and not args.keep_stale:
        stale = prune(args.out, ids, dry_run=bool(failures))
        if stale:
            verb = "would prune" if failures else "pruned"
            print(f"\n{verb} {stale} file(s) from architectures outside the matrix")

    if failures:
        print("\nfailed:", *[f"{name}: {error}" for name, error in failures], sep="\n  ")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
