"""Build a small 3-class dataset from coco128 to compare scratch vs pretrained training.

Class choice: person (0), car (2), dog (16). All three exist in COCO, so the pretrained run has
already seen them; that makes any measured advantage an *upper bound* rather than a conservative
number. The point of the experiment is the opposite direction: if even a same-domain head start
does not show up here, the feature has no value at all.

Split is fixed (first 60 sorted images train, rest val) so both runs see identical data.
"""

from __future__ import annotations

import shutil
from pathlib import Path

ROOT = Path(__file__).parent
SRC = ROOT / "coco128" / "coco128"
DST = ROOT / "exp-dataset"
KEEP = {0: "person", 2: "car", 16: "dog"}
REMAP = {0: 0, 2: 1, 16: 2}
TRAIN_COUNT = 60


def main() -> None:
    if DST.exists():
        shutil.rmtree(DST)
    for split in ("train", "val"):
        (DST / "images" / split).mkdir(parents=True)
        (DST / "labels" / split).mkdir(parents=True)

    images = sorted((SRC / "images" / "train2017").glob("*.jpg"))
    kept = 0
    for index, image in enumerate(images):
        label = SRC / "labels" / "train2017" / f"{image.stem}.txt"
        if not label.exists():
            continue
        rows = []
        for line in label.read_text().splitlines():
            parts = line.split()
            if not parts:
                continue
            cls = int(parts[0])
            if cls in KEEP:
                rows.append(" ".join([str(REMAP[cls]), *parts[1:]]))
        if not rows:
            continue

        split = "train" if kept < TRAIN_COUNT else "val"
        shutil.copy2(image, DST / "images" / split / image.name)
        (DST / "labels" / split / f"{image.stem}.txt").write_text("\n".join(rows) + "\n")
        kept += 1

    yaml = DST / "dataset.yaml"
    yaml.write_text(
        "path: .\ntrain: images/train\nval: images/val\nnames:\n"
        + "".join(f"  {i}: {name}\n" for i, name in enumerate(["person", "car", "dog"])),
        encoding="utf-8",
    )
    for split in ("train", "val"):
        print(split, len(list((DST / "images" / split).glob("*.jpg"))))
    print("total", kept)


if __name__ == "__main__":
    main()
