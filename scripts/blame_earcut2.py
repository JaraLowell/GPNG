"""Find any ring whose earcut triangles cover full-height strip columns."""
from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import mapbox_earcut as earcut
from PIL import Image

DATA = Path(__file__).resolve().parents[1] / "data" / "LOCALES" / "MAP_COUNTRIES.json"
BAD = Path(r"C:\CursorTemp\map_bad.png")


def tri_contains(tri, p):
    (x1, y1), (x2, y2), (x3, y3) = tri
    px, py = p
    denom = (y2 - y3) * (x1 - x3) + (x3 - x2) * (y1 - y3)
    if abs(denom) < 1e-12:
        return False
    a = ((y2 - y3) * (px - x3) + (x3 - x2) * (py - y3)) / denom
    b = ((y3 - y1) * (px - x3) + (x1 - x3) * (py - y3)) / denom
    c = 1 - a - b
    return a >= -1e-6 and b >= -1e-6 and c >= -1e-6


def main():
    d = json.loads(DATA.read_text(encoding="utf-8"))
    im = Image.open(BAD).convert("RGB")
    w, h = im.size

    # Brute: try many views centered on Africa
    samples = [(630.5, y + 0.5) for y in range(0, h, 20)]  # full-height column
    # also ocean-only samples
    ocean_samples = [(630.5, y + 0.5) for y in [450, 500, 550, 580]]

    views = []
    for span_lon in [60, 80, 100, 120, 150, 200, 360]:
        for clat in [-10, -20, -30, 0]:
            for clon in [20, 25, 30, 35]:
                views.append((clon, clat, span_lon))

    # Also check Sudan specifically across views
    suspects = []
    for c in d["countries"]:
        for ri, ring in enumerate(c["rings"]):
            body = ring[:-1] if ring[0] == ring[-1] else list(ring)
            if len(body) < 3:
                continue
            # quick: geo bbox must overlap lon 15..40
            lons = [p[0] for p in body]
            lats = [p[1] for p in body]
            if max(lons) < 10 or min(lons) > 45:
                continue
            suspects.append((c["name"], ri, body, min(lons), max(lons), min(lats), max(lats)))

    print("candidate rings", len(suspects))

    hits_total = {}
    for clon, clat, span_lon in views:
        aspect = w / h
        span_lat = span_lon / aspect

        def project(lon, lat):
            x = (lon - (clon - span_lon * 0.5)) / span_lon * w
            y = (1.0 - (lat - (clat - span_lat * 0.5)) / span_lat) * h
            return [x, y]

        for name, ri, body, *_ in suspects:
            pts = np.array([project(p[0], p[1]) for p in body], dtype=np.float64)
            # skip if all points off-screen far
            if pts[:, 0].max() < -100 or pts[:, 0].min() > w + 100:
                continue
            if pts[:, 1].max() < -100 or pts[:, 1].min() > h + 100:
                continue
            idx = earcut.triangulate_float64(pts, np.array([len(pts)], dtype=np.uint32))
            ocean_hit = 0
            for t in range(0, len(idx), 3):
                tri = (pts[idx[t]], pts[idx[t + 1]], pts[idx[t + 2]])
                # triangle vertical extent
                ys = [tri[0][1], tri[1][1], tri[2][1]]
                xs = [tri[0][0], tri[1][0], tri[2][0]]
                if max(ys) - min(ys) < h * 0.4:
                    continue  # not a tall spike triangle
                for s in ocean_samples:
                    if tri_contains(tri, s):
                        ocean_hit += 1
                        break
            if ocean_hit:
                key = f"{name}#{ri}"
                hits_total[key] = hits_total.get(key, 0) + ocean_hit
                print(
                    f"HIT {key} view clon={clon} clat={clat} span={span_lon} ocean_tris={ocean_hit}"
                )

    print("summary", sorted(hits_total.items(), key=lambda kv: -kv[1])[:20])

    # Inspect Sudan self-intersection geometry
    sudan = next(c for c in d["countries"] if c["name"] == "Sudan")
    r = sudan["rings"][0]
    print("Sudan n", len(r), "bbox", sudan["bbox"])


if __name__ == "__main__":
    main()
