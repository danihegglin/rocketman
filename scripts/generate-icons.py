#!/usr/bin/env python3
"""Generate every app icon file from the master SVG, assets/icon/rocketman.svg.

Writes, next to it in assets/icon/:
  png/<N>x<N>.png    Linux hicolor icons (16 to 512 px)
  rocketman.png      256 px, the window icon (X11) and the AppImage icon
  rocketman.ico      Windows: embedded in the .exe files and used by the .msi
  rocketman.icns     macOS: the .app bundle icon

The generated files are committed, so packaging doesn't need this script's
dependencies. Rerun it after changing the SVG:
  pip install cairosvg pillow
  scripts/generate-icons.py
"""
import io
from pathlib import Path

import cairosvg
from PIL import Image, ImageFilter

ICON_DIR = Path(__file__).resolve().parent.parent / "assets" / "icon"
SVG = (ICON_DIR / "rocketman.svg").read_bytes()


def render(size: int) -> Image.Image:
    png = cairosvg.svg2png(bytestring=SVG, output_width=size, output_height=size)
    return Image.open(io.BytesIO(png)).convert("RGBA")


def macos(size: int) -> Image.Image:
    """The icon on Apple's grid: the shape takes 824 of 1024 px, with a soft shadow."""
    shape = round(size * 824 / 1024)
    off = (size - shape) // 2
    icon = render(shape)
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    alpha = Image.new("L", (size, size), 0)
    alpha.paste(icon.getchannel("A"), (off, off + max(1, size // 100)))
    shadow = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    shadow.putalpha(alpha.filter(ImageFilter.GaussianBlur(size / 80)).point(lambda a: a * 0.35))
    canvas.alpha_composite(shadow)
    canvas.alpha_composite(icon, (off, off))
    return canvas


def main() -> None:
    (ICON_DIR / "png").mkdir(exist_ok=True)
    for size in (16, 24, 32, 48, 64, 128, 256, 512):
        render(size).save(ICON_DIR / "png" / f"{size}x{size}.png", optimize=True)
    render(256).save(ICON_DIR / "rocketman.png", optimize=True)

    # Each size is rendered from the SVG rather than scaled down from a big one.
    ico_sizes = (16, 20, 24, 32, 40, 48, 64, 256)
    ico = [render(s) for s in ico_sizes]
    # BMP entries (not PNG) are the most widely accepted, by rc.exe and old shells alike.
    ico[-1].save(
        ICON_DIR / "rocketman.ico",
        sizes=[(s, s) for s in ico_sizes],
        append_images=ico[:-1],
        bitmap_format="bmp",
    )

    icns = [macos(s) for s in (16, 32, 64, 128, 256, 512, 1024)]
    icns[-1].save(ICON_DIR / "rocketman.icns", append_images=icns[:-1])


if __name__ == "__main__":
    main()
