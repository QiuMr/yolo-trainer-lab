"""Generate the pretrained-weights manifest that ships inside the installer.

The installer carries this manifest only — a few kilobytes of name/size/sha256. The weights
themselves live in a GitHub Release and are fetched on demand, which keeps the download at 29 MB
while still offering every rung. `.bpk` compresses at 1.11x, so bundling the weights was never an
option: 15 detection models alone are ~790 MiB.

Usage:
    python gen_manifest.py --bpk <dir with .bpk files> --out <manifest.json>
                           [--base-url https://github.com/OWNER/REPO/releases/download/TAG/]
                           [--only yolo26n yolov8n ...]

`--base-url` must end with a slash. The URL is baked into the shipped manifest, so a wrong value is
only discovered by a user whose download 404s — hence the assertion instead of a warning.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

# Families and rungs the product offers (D028). Anything else in the bpk directory is ignored so a
# stray artifact from an experiment cannot ship.
FAMILIES = ("yolo26", "yolo11", "yolov8")
SCALES = ("n", "s", "m", "l", "x")
SUFFIXES = {"": "detect", "-seg": "segment", "-cls": "classify"}

# What each task's published pretraining was trained on. Shown in the UI so nobody wonders why a
# fresh model already recognises 80 things.
DATASETS = {"detect": "COCO", "segment": "COCO", "classify": "ImageNet"}


def parse(arch: str) -> tuple[str, str, str] | None:
    for suffix, task in SUFFIXES.items():
        if suffix and not arch.endswith(suffix):
            continue
        stem = arch[: len(arch) - len(suffix)] if suffix else arch
        for family in FAMILIES:
            if stem.startswith(family) and stem[len(family) :] in SCALES:
                return family, stem[len(family) :], task
    return None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bpk", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--only", nargs="*", default=None)
    args = parser.parse_args()

    if not args.base_url.endswith("/"):
        raise SystemExit("--base-url must end with a slash")
    if not args.base_url.startswith("https://"):
        raise SystemExit("--base-url must be https; the download is unverified otherwise")

    source = Path(args.bpk)
    entries = []
    for path in sorted(source.glob("*.bpk")):
        arch = path.stem
        if args.only is not None and arch not in args.only:
            continue
        parsed = parse(arch)
        if parsed is None:
            print(f"skip {arch}: not a catalog architecture")
            continue
        family, scale, task = parsed
        data = path.read_bytes()
        entries.append(
            {
                "arch": arch,
                "family": family,
                "scale": scale,
                "task": task,
                "file": path.name,
                "bytes": len(data),
                "sha256": hashlib.sha256(data).hexdigest(),
                "classes": 1000 if task == "classify" else 80,
                "dataset": DATASETS[task],
            }
        )

    if not entries:
        raise SystemExit("no matching .bpk files")

    manifest = {
        "version": 1,
        "base_url": args.base_url,
        "weights": entries,
    }
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")

    total = sum(entry["bytes"] for entry in entries)
    print(f"{len(entries)} weights, {total / 1048576:.1f} MiB total -> {out}")
    for entry in entries:
        print(f"  {entry['arch']:<14} {entry['bytes'] / 1048576:7.2f} MiB  {entry['task']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
