"""Generate the app icon master (1024x1024 PNG).

Black rounded square + white "Y". Drawn geometrically rather than with a font so the
shape is identical on every machine and stays readable at 16px in the system tray.

Run:  python spikes/brand/make_icon.py
Out:  artifacts/brand/icon.png   (feed to `pnpm tauri icon`)
"""

from pathlib import Path

from PIL import Image, ImageDraw

S = 4  # supersample factor; downscaled at the end for clean anti-aliased edges
SIZE = 1024 * S


def px(value: float) -> int:
    return int(round(value * S))


def main() -> None:
    canvas = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(canvas)

    # Squircle-ish plate. Radius 220/1024 reads as "rounded" at 32px without turning
    # into a circle at 16px.
    draw.rounded_rectangle(
        (px(48), px(48), px(976), px(976)),
        radius=px(220),
        fill=(10, 10, 10, 255),
    )

    white = (255, 255, 255, 255)
    stroke = px(94)
    junction = (px(512), px(524))

    # Left arm, then the stem, as one polyline; joint="curve" rounds the bend.
    draw.line(
        [(px(340), px(320)), junction, (px(512), px(700))],
        fill=white,
        width=stroke,
        joint="curve",
    )
    # Right arm.
    draw.line([(px(684), px(320)), junction], fill=white, width=stroke)

    # Round caps: Pillow's line() has butt ends, which look chopped at this weight.
    r = stroke / 2
    for cx, cy in [(340, 320), (684, 320), (512, 700)]:
        draw.ellipse(
            (px(cx) - r, px(cy) - r, px(cx) + r, px(cy) + r), fill=white
        )

    out = Path(__file__).resolve().parents[2] / "artifacts" / "brand"
    out.mkdir(parents=True, exist_ok=True)

    master = canvas.resize((1024, 1024), Image.LANCZOS)
    master.save(out / "icon.png")

    # A 16/32px preview strip so the shapes can be judged at tray size, not just zoomed.
    for size in (16, 24, 32, 48):
        master.resize((size, size), Image.LANCZOS).save(out / f"preview-{size}.png")

    print(f"wrote {out / 'icon.png'}")


if __name__ == "__main__":
    main()
