"""Earcut-triangulate projected rings; find triangles covering the green strip."""
from __future__ import annotations

import json
from pathlib import Path

import numpy as np
from PIL import Image

try:
    import mapbox_earcut as earcut
except ImportError:
    import subprocess, sys

    subprocess.check_call([sys.executable, "-m", "pip", "install", "mapbox_earcut", "-q"])
    import mapbox_earcut as earcut

ROOT = Path(__file__).resolve().parents[1]
BAD = Path(r"C:\CursorTemp\map_bad.png")
DATA = ROOT / "data" / "LOCALES" / "MAP_COUNTRIES.json"


def main() -> None:
    d = json.loads(DATA.read_text(encoding="utf-8"))
    im = Image.open(BAD).convert("RGB")
    w, h = im.size
    px = im.load()

    # Recalibrate using field grid: search for vertical lines of non-blue/non-land
    # Count how many distinct green land components at mid-Africa vs known geography.
    # Use South Africa west coast ~16.3E and east ~32.8E if we can find tip row.
    tip_y = 317
    xs = [x for x in range(w) if px[x, tip_y] == (0, 85, 0)]
    print("tip_y land", xs[0], xs[-1], "n", len(xs))

    # Try BOTH calibrations:
    # A) madagascar guess
    # B) assume tip land x-range is roughly SA+neighbors lon 10..40
    cals = {
        "mada": ((753, 787), (43.2542, 50.4765)),
        "sa_tip": ((xs[0], xs[-1]), (12.0, 36.0)),  # rough southern africa width
    }

    samples = []
    for y in range(420, h - 10, 15):
        for x in range(590, 670, 15):
            if px[x, y] == (0, 85, 0):
                samples.append((x + 0.5, y + 0.5))
    print("strip samples", len(samples))

    for cal_name, ((x0, x1), (lon0, lon1)) in cals.items():
        lon_per_px = (lon1 - lon0) / (x1 - x0)
        lon_at0 = lon0 - x0 * lon_per_px
        lat_per_px = lon_per_px
        lon_at = lambda x, lo=lon_at0, lp=lon_per_px: lo + x * lp
        lat_at = lambda y, lp=lat_per_px: -34.83 + (tip_y - y) * lp
        center_lon = (lon_at(0) + lon_at(w)) / 2
        center_lat = (lat_at(0) + lat_at(h)) / 2
        span_lon = lon_at(w) - lon_at(0)
        span_lat = lat_at(0) - lat_at(h)
        print(f"\n=== {cal_name} view center={center_lon:.2f},{center_lat:.2f} span={span_lon:.1f}x{span_lat:.1f}")
        print(f"    strip geo sample", lon_at(630), lat_at(500))

        def project(lon, lat, cl=center_lon, ca=center_lat, sl=span_lon, sa=span_lat):
            x = (lon - (cl - sl * 0.5)) / sl * w
            y = (1.0 - (lat - (ca - sa * 0.5)) / sa) * h
            return [x, y]

        def tri_contains(tri, p):
            (x1, y1), (x2, y2), (x3, y3) = tri
            px_, py = p
            denom = (y2 - y3) * (x1 - x3) + (x3 - x2) * (y1 - y3)
            if abs(denom) < 1e-12:
                return False
            a = ((y2 - y3) * (px_ - x3) + (x3 - x2) * (py - y3)) / denom
            b = ((y3 - y1) * (px_ - x3) + (x1 - x3) * (py - y3)) / denom
            c = 1 - a - b
            return a >= -1e-6 and b >= -1e-6 and c >= -1e-6

        blame = {}
        for c in d["countries"]:
            bb = c["bbox"]
            if bb[2] < center_lon - span_lon * 0.6 or bb[0] > center_lon + span_lon * 0.6:
                continue
            if bb[3] < center_lat - span_lat * 0.6 or bb[1] > center_lat + span_lat * 0.6:
                continue
            for ri, ring in enumerate(c["rings"]):
                body = ring[:-1] if ring[0] == ring[-1] else ring
                if len(body) < 3:
                    continue
                pts = np.array([project(p[0], p[1]) for p in body], dtype=np.float64)
                rings_idx = np.array([len(pts)], dtype=np.uint32)
                try:
                    idx = earcut.triangulate_float64(pts, rings_idx)
                except Exception as e:
                    print("earcut fail", c["name"], ri, e)
                    continue
                hit = 0
                for t in range(0, len(idx), 3):
                    tri = (pts[idx[t]], pts[idx[t + 1]], pts[idx[t + 2]])
                    # skip tiny / degenerate
                    if any(tri_contains(tri, s) for s in samples):
                        hit += 1
                if hit:
                    key = f"{c['name']}#{ri}"
                    blame[key] = hit
        print("blame", sorted(blame.items(), key=lambda kv: -kv[1])[:15])


if __name__ == "__main__":
    main()
