#!/usr/bin/env python3
"""Probe/decode Graphic Packet ICONS.GP (v1.50+ library format)."""

from __future__ import annotations

import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ICON_PATH = ROOT / "ICONS.GP"
OUT = Path(__file__).resolve().parents[1] / "assets" / "icons"
VGA16 = [
    (0, 0, 0),
    (0, 0, 170),
    (0, 170, 0),
    (0, 170, 170),
    (170, 0, 0),
    (170, 0, 170),
    (170, 85, 0),
    (170, 170, 170),
    (85, 85, 85),
    (85, 85, 255),
    (85, 255, 85),
    (85, 255, 255),
    (255, 85, 85),
    (255, 85, 255),
    (255, 255, 85),
    (255, 255, 255),
]


def write_ppm(path: Path, w: int, h: int, pixels: list[list[int]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    lines = [f"P3\n{w} {h}\n255\n"]
    for y in range(h):
        row = []
        for x in range(w):
            r, g, b = VGA16[pixels[y][x] & 15]
            row.append(f"{r} {g} {b}")
        lines.append(" ".join(row) + "\n")
    path.write_text("".join(lines), encoding="ascii")


def decode_planar(data: bytes, w: int, h: int, bpr: int, planes: int = 4) -> list[list[int]]:
    need = planes * h * bpr
    assert len(data) >= need, (len(data), need)
    pix = [[0] * w for _ in range(h)]
    for p in range(planes):
        for y in range(h):
            row = data[(p * h + y) * bpr : (p * h + y + 1) * bpr]
            for x in range(w):
                byte = row[x // 8]
                bit = 7 - (x % 8)
                if byte & (1 << bit):
                    pix[y][x] |= 1 << p
    return pix


def decode_nibbles(data: bytes, w: int, h: int) -> list[list[int]]:
    pix = [[0] * w for _ in range(h)]
    i = 0
    for y in range(h):
        x = 0
        while x < w:
            byte = data[i]
            i += 1
            pix[y][x] = byte >> 4
            if x + 1 < w:
                pix[y][x + 1] = byte & 0x0F
            x += 2
    return pix


def main() -> None:
    raw = ICON_PATH.read_bytes()
    assert raw.startswith(b"Graphic Packet  Icon Libary")
    assert raw[63] == 0x1A
    file_crc = struct.unpack_from("<H", raw, 128)[0]
    base, stride, count = 130, 1482, 17
    print(f"file={ICON_PATH} size={len(raw)} crc16?=0x{file_crc:04X}")
    print(f"icons={count} stride={stride} base={base}")

    OUT.mkdir(parents=True, exist_ok=True)
    for n in range(count):
        off = base + n * stride
        chunk = raw[off : off + stride]
        w, h = struct.unpack_from("<HH", chunk, 0)
        print(f"icon[{n:02}] @{off} {w}x{h}")

        # Candidate A: 4-byte header + planar 9*40*4
        bpr = (w + 7) // 8
        planar_size = 4 * h * bpr
        for hdr in (4, 12, 13, 16):
            if hdr + planar_size <= stride:
                pix = decode_planar(chunk[hdr : hdr + planar_size], w, h, bpr)
                write_ppm(OUT / f"icon{n:02}_planar_hdr{hdr}.ppm", w, h, pix)

        # Candidate B: nibble pixels with large header
        nib_size = (w * h + 1) // 2
        for hdr in (12, 16, 82):
            if hdr + nib_size <= stride:
                pix = decode_nibbles(chunk[hdr : hdr + nib_size], w, h)
                write_ppm(OUT / f"icon{n:02}_nib_hdr{hdr}.ppm", w, h, pix)

    print(f"wrote trials under {OUT}")


if __name__ == "__main__":
    main()
