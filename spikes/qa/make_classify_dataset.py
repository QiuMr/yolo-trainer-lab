"""Build a tiny ImageNet-style classification dataset.

Classification is the one task whose dataset layout the app has never been able to produce, so
there was no fixture to test against. This generates one: three classes, each a distinct shape on
a distinct background, which is enough for a model to show a falling loss in one epoch without
needing a download.

Usage:
    python spikes/qa/make_classify_dataset.py [output_dir]
"""

import math
import os
import random
import sys

from PIL import Image, ImageDraw

SIZE = 96
TRAIN_PER_CLASS = 6
VAL_PER_CLASS = 2

# (name, background, shape) — shape differs per class so the task is not separable by colour alone.
CLASSES = [
    ("circle", (198, 40, 40), "ellipse"),
    ("square", (46, 125, 50), "rect"),
    ("triangle", (25, 118, 210), "polygon"),
]


def draw_sample(kind: str, background: tuple[int, int, int], rng: random.Random) -> Image.Image:
    image = Image.new("RGB", (SIZE, SIZE), background)
    draw = ImageDraw.Draw(image)
    margin = rng.randint(14, 26)
    box = (margin, margin, SIZE - margin, SIZE - margin)
    if kind == "ellipse":
        draw.ellipse(box, fill=(250, 250, 250))
    elif kind == "rect":
        draw.rectangle(box, fill=(250, 250, 250))
    else:
        cx = SIZE / 2
        draw.polygon(
            [
                (cx, box[1]),
                (box[2], box[3]),
                (box[0], box[3]),
            ],
            fill=(250, 250, 250),
        )
    # A little noise keeps the images from being perfectly separable at a glance.
    for _ in range(180):
        x = rng.randrange(SIZE)
        y = rng.randrange(SIZE)
        base = image.getpixel((x, y))
        jitter = rng.randint(-18, 18)
        image.putpixel(
            (x, y),
            tuple(max(0, min(255, channel + jitter)) for channel in base),
        )
    return image


def main() -> None:
    out = sys.argv[1] if len(sys.argv) > 1 else r"E:\工作区\yolo-trainer-lab\spikes\tiny-classify"
    rng = random.Random(20261111)

    for split, count in (("train", TRAIN_PER_CLASS), ("val", VAL_PER_CLASS)):
        for name, background, shape in CLASSES:
            directory = os.path.join(out, split, name)
            os.makedirs(directory, exist_ok=True)
            for index in range(count):
                image = draw_sample(shape, background, rng)
                image.save(os.path.join(directory, f"{name}-{index:02d}.png"))

    manifest = os.path.join(out, "dataset.yaml")
    names = "\n".join(f"  - {name}" for name, _, _ in CLASSES)
    with open(manifest, "w", encoding="utf-8") as handle:
        handle.write(
            "path: .\n"
            "train: train\n"
            "val: val\n"
            "names:\n"
            f"{names}\n"
        )

    total = sum(
        1
        for split, count in (("train", TRAIN_PER_CLASS), ("val", VAL_PER_CLASS))
        for name, _, _ in CLASSES
        for _ in range(count)
    )
    print(f"{out}: {len(CLASSES)} classes, {total} images")
    print(manifest)


if __name__ == "__main__":
    main()
