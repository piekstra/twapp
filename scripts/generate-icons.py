#!/usr/bin/env python3
"""Generate colored icon variants for twapp session instances.

Takes the twapp logo (transparent PNG) and composites it onto colored
rounded-rect backgrounds. Generates both light and dark variants for
each palette color.
"""

import os
import subprocess
import sys
import tempfile
from PIL import Image, ImageDraw, ImageChops, ImageEnhance

# Must match THEME_PALETTE in src-tauri/src/cli/theme.rs (skip index 0 = default)
PALETTE = [
    ("rose",        "#ffe0e0", "#4a2020"),
    ("cornflower",  "#e0e8ff", "#1a2040"),
    ("mint",        "#e0ffe0", "#1a3a1a"),
    ("peach",       "#fff0e0", "#3a2a1a"),
    ("lavender",    "#f0e0ff", "#2a1a3a"),
    ("seafoam",     "#e0ffff", "#1a3a3a"),
    ("lemon",       "#fef3c7", "#3a3520"),
    ("cappuccino",  "#e8d8cc", "#2e2420"),
    ("sage",        "#e8f0e0", "#2a3020"),
]
# Columns: (name, light_bg_hex, dark_bg_hex)

ICONSET_SIZES = [
    ("icon_16x16.png",      16),
    ("icon_16x16@2x.png",   32),
    ("icon_32x32.png",      32),
    ("icon_32x32@2x.png",   64),
    ("icon_128x128.png",    128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png",    256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png",    512),
    ("icon_512x512@2x.png", 1024),
]

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
PROJECT_ROOT = os.path.dirname(SCRIPT_DIR)
LOGO_PATH = os.path.join(PROJECT_ROOT, "docs", "images", "twapp-logo.png")
OUTPUT_DIR = os.path.join(PROJECT_ROOT, "src-tauri", "icons", "variants")


def hex_to_rgb(h: str) -> tuple:
    h = h.lstrip("#")
    return tuple(int(h[i:i+2], 16) for i in (0, 2, 4))


def create_rounded_rect(size: int, color: tuple) -> Image.Image:
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    radius = int(size * 0.22)
    draw.rounded_rectangle(
        [(0, 0), (size - 1, size - 1)],
        radius=radius,
        fill=(*color, 255),
    )
    return img


def invert_logo_for_light_bg(logo: Image.Image) -> Image.Image:
    """Create a dark version of the logo for light backgrounds.

    Inverts the RGB channels while preserving alpha.
    """
    r, g, b, a = logo.split()
    rgb = Image.merge("RGB", (r, g, b))
    inverted = ImageChops.invert(rgb)
    # Darken a bit more for better contrast on pastels
    inverted = ImageEnhance.Brightness(inverted).enhance(0.6)
    result = inverted.convert("RGBA")
    result.putalpha(a)
    return result


def composite_logo(bg: Image.Image, logo: Image.Image) -> Image.Image:
    """Center the logo on the background, scaled to ~65% width."""
    size = bg.size[0]
    logo_w = int(size * 0.85)
    aspect = logo.size[1] / logo.size[0]
    logo_h = int(logo_w * aspect)
    logo_resized = logo.resize((logo_w, logo_h), Image.LANCZOS)
    x = (size - logo_w) // 2
    y = (size - logo_h) // 2
    result = bg.copy()
    result.paste(logo_resized, (x, y), logo_resized)
    return result


def generate_icns(name: str, composited_1024: Image.Image) -> bool:
    icns_path = os.path.join(OUTPUT_DIR, f"icon-{name}.icns")
    with tempfile.TemporaryDirectory() as tmpdir:
        iconset_dir = os.path.join(tmpdir, f"icon-{name}.iconset")
        os.makedirs(iconset_dir)
        for filename, size in ICONSET_SIZES:
            resized = composited_1024.resize((size, size), Image.LANCZOS)
            resized.save(os.path.join(iconset_dir, filename))
        result = subprocess.run(
            ["iconutil", "-c", "icns", iconset_dir, "-o", icns_path],
            capture_output=True, text=True,
        )
        if result.returncode != 0:
            print(f"  ERROR: {name}: {result.stderr}", file=sys.stderr)
            return False
    print(f"  {name}: {icns_path}")
    return True


def main():
    if not os.path.exists(LOGO_PATH):
        print(f"Logo not found: {LOGO_PATH}", file=sys.stderr)
        sys.exit(1)

    os.makedirs(OUTPUT_DIR, exist_ok=True)

    logo = Image.open(LOGO_PATH).convert("RGBA")
    logo_dark = invert_logo_for_light_bg(logo)

    print(f"Generating {len(PALETTE) * 2} icon variants (light + dark)...")

    ok = True
    for name, light_hex, dark_hex in PALETTE:
        # Dark variant: dark bg + original (light) logo
        dark_bg = create_rounded_rect(1024, hex_to_rgb(dark_hex))
        dark_icon = composite_logo(dark_bg, logo)
        if not generate_icns(f"{name}-dark", dark_icon):
            ok = False

        # Light variant: pastel bg + inverted (dark) logo
        light_bg = create_rounded_rect(1024, hex_to_rgb(light_hex))
        light_icon = composite_logo(light_bg, logo_dark)
        if not generate_icns(f"{name}-light", light_icon):
            ok = False

    # Also save 512px preview PNGs for easy inspection
    preview_dir = os.path.join(OUTPUT_DIR, "previews")
    os.makedirs(preview_dir, exist_ok=True)
    for name, light_hex, dark_hex in PALETTE:
        for mode in ("dark", "light"):
            icns = os.path.join(OUTPUT_DIR, f"icon-{name}-{mode}.icns")
            preview = os.path.join(preview_dir, f"{name}-{mode}.png")
            subprocess.run(
                ["sips", "-s", "format", "png", icns, "--out", preview],
                capture_output=True,
            )

    if ok:
        print(f"\nDone. {len(PALETTE) * 2} icons in {OUTPUT_DIR}/")
    else:
        print("Some icons failed.", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
