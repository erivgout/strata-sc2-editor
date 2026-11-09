"""Generate the Strata icon as SVG and PNGs from one geometry.

Concept: three stacked strata lines, each stepping up a cliff level, seen in
cross-section. Mineral cyan on slate. Usage: python make_icon.py <out_dir>
"""
import sys
from pathlib import Path

from PIL import Image, ImageDraw

SLATE = (0x1B, 0x23, 0x2D)
DEEP = (0x13, 0x19, 0x20)
CYAN = (0x62, 0xCD, 0xEB)

# Geometry in a 512 x 512 box. Each stratum: points, opacity.
STROKE = 34
STRATA = [
    ([(116, 360), (268, 360), (316, 312), (396, 312)], 0.38),
    ([(116, 280), (212, 280), (260, 232), (396, 232)], 0.68),
    ([(116, 200), (156, 200), (204, 152), (396, 152)], 1.0),
]
RADIUS = 112


def mix(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def svg(maskable=False):
    scale, off = (0.78, 512 * 0.11) if maskable else (1.0, 0.0)
    rx = 0 if maskable else RADIUS
    parts = [
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">',
        "  <title>Strata</title>",
        f'  <rect width="512" height="512" rx="{rx}" fill="#1B232D"/>',
        f'  <g fill="none" stroke="#62CDEB" stroke-width="{STROKE * scale:g}" '
        'stroke-linecap="round" stroke-linejoin="round">',
    ]
    for pts, op in STRATA:
        d = " ".join(
            f"{'M' if i == 0 else 'L'}{x * scale + off:g} {y * scale + off:g}" for i, (x, y) in enumerate(pts)
        )
        parts.append(f'    <path d="{d}" stroke-opacity="{op:g}"/>')
    parts += ["  </g>", "</svg>", ""]
    return "\n".join(parts)


def png(size, maskable=False):
    ss = 4
    n = size * ss
    k = n / 512
    scale, off = (0.78, 512 * 0.11) if maskable else (1.0, 0.0)
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    if maskable:
        d.rectangle([0, 0, n, n], fill=SLATE + (255,))
    else:
        d.rounded_rectangle([0, 0, n - 1, n - 1], radius=RADIUS * k, fill=SLATE + (255,))
    w = STROKE * scale * k
    for pts, op in STRATA:
        color = mix(SLATE, CYAN, op) + (255,)
        p = [((x * scale + off) * k, (y * scale + off) * k) for x, y in pts]
        d.line(p, fill=color, width=round(w), joint="curve")
        for x, y in (p[0], p[-1]):
            d.ellipse([x - w / 2, y - w / 2, x + w / 2, y + w / 2], fill=color)
    return img.resize((size, size), Image.LANCZOS)


def ico(path):
    """Windows icon (exe, shortcuts, file types): each size drawn on its own."""
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    imgs = [png(s) for s in sizes]
    imgs[-1].save(path, format="ICO", sizes=[(s, s) for s in sizes], append_images=imgs[:-1])


def main():
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    (out / "icon.svg").write_text(svg(), encoding="utf-8")
    (out / "icon-maskable.svg").write_text(svg(maskable=True), encoding="utf-8")
    for s in (32, 180, 192, 512):
        png(s).save(out / f"icon-{s}.png", optimize=True)
    png(512, maskable=True).save(out / "icon-maskable-512.png", optimize=True)
    ico(out / "icon.ico")


if __name__ == "__main__":
    main()
