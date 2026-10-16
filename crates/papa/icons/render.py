#!/usr/bin/env python3
"""Renders Papa's icon files from papa.svg.

Needs resvg (https://github.com/linebender/resvg) on the PATH, or its path in
$RESVG; Python's standard library does the rest. Run it from anywhere:

    python3 crates/papa/icons/render.py

Writes, next to this script:
- 32x32.png ... 512x512.png: the Linux hicolor icons, one per size;
- icon.ico: Windows (16 to 256 px);
- papa.icns: macOS (16 to 1024 px, Retina sizes included).
"""

import os
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
SVG = HERE / "papa.svg"
RESVG = os.environ.get("RESVG", "resvg")

LINUX_SIZES = [32, 64, 128, 256, 512]
ICO_SIZES = [16, 24, 32, 48, 64, 256]
# ICNS entry types holding PNG data, by pixel size. The `ic1x` types are the
# Retina versions of half the size (ic12 is 32 pt at 2x, and so on).
ICNS_TYPES = [
    (b"icp4", 16),
    (b"ic11", 32),
    (b"icp5", 32),
    (b"ic12", 64),
    (b"ic07", 128),
    (b"ic13", 256),
    (b"ic08", 256),
    (b"ic14", 512),
    (b"ic09", 512),
    (b"ic10", 1024),
]


def render(size: int, out: Path) -> bytes:
    subprocess.run([RESVG, "-w", str(size), "-h", str(size), str(SVG), str(out)], check=True)
    return out.read_bytes()


def ico(pngs: dict[int, bytes]) -> bytes:
    """An ICO file whose images are PNGs (Windows Vista and later)."""
    header = struct.pack("<HHH", 0, 1, len(ICO_SIZES))
    offset = len(header) + 16 * len(ICO_SIZES)
    entries, data = b"", b""
    for size in ICO_SIZES:
        png = pngs[size]
        # 0 means 256 in the one-byte width and height fields.
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(png), offset)
        offset += len(png)
        data += png
    return header + entries + data


def icns(pngs: dict[int, bytes]) -> bytes:
    """An ICNS file: a header, then (type, length, PNG) entries, big-endian."""
    body = b"".join(kind + struct.pack(">I", 8 + len(pngs[size])) + pngs[size] for kind, size in ICNS_TYPES)
    return b"icns" + struct.pack(">I", 8 + len(body)) + body


def main() -> int:
    sizes = sorted(set(LINUX_SIZES) | set(ICO_SIZES) | {size for _, size in ICNS_TYPES})
    with tempfile.TemporaryDirectory() as tmp:
        pngs = {size: render(size, Path(tmp) / f"{size}.png") for size in sizes}
    for size in LINUX_SIZES:
        (HERE / f"{size}x{size}.png").write_bytes(pngs[size])
    (HERE / "icon.ico").write_bytes(ico(pngs))
    (HERE / "papa.icns").write_bytes(icns(pngs))
    print(f"rendered {SVG.name} into {HERE}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
