"""Calibrate map view from bad screenshot and find which ring fills the strip."""
from __future__ import annotations

import json
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
BAD = Path(r"C:\CursorTemp\map_bad.png")
DATA = ROOT / "data" / "LOCALES" / "MAP_COUNTRIES.json"


def is_land(c: tuple[int, int, int]) -> bool:
    r, g, b = c
    return r == 0 and g == 85 and b == 0


def pip_xy(x: float, y: float, ring: list[tuple[float, float]]) -> bool:
    n = len(ring) - (1 if ring and abs(ring[0][0] - ring[-1][0]) < 1e-6 and abs(ring[0][1] - ring[-1][1]) < 1e-6 else 0)
    if n < 3:
        return False
    inside = False
    j = n - 1
    for i in range(n):
        xi, yi = ring[i]
        xj, yj = ring[j]
        if ((yi > y) != (yj > y)) and (x < (xj - xi) * (y - yi) / (yj - yi + 1e-15) + xi):
            inside = not inside
        j = i
    return inside


def main() -> None:
    d = json.loads(DATA.read_text(encoding="utf-8"))
    im = Image.open(BAD).convert("RGB")
    w, h = im.size
    px = im.load()

    # Pure DarkGreen mask
    # Madagascar: known bbox ~43.25..50.48, -25.6..-12.0 — find its blob
    # At mid latitudes madagascar is separate run ~753-787 at y250 in prior scan
    # Use that: x 753..787 = lon 43.25..50.48
    x_m0, x_m1 = 753, 787
    lon_m0, lon_m1 = 43.2542, 50.4765
    lon_per_px = (lon_m1 - lon_m0) / (x_m1 - x_m0)
    lon_at = lambda x: lon_m0 + (x - x_m0) * lon_per_px
    print("lon_per_px", lon_per_px)
    print("lon strip 591..668", lon_at(591), lon_at(668))
    print("lon 0..w", lon_at(0), lon_at(w))

    # Cape Agulhas ~20.0E, -34.83 — find southernmost pure land near lon 20
    cape_x = (20.0 - lon_at(0)) / lon_per_px
    print("expected cape x", cape_x)
    # Find max y with land near that x
    cape_ys = []
    for y in range(h):
        for dx in range(-15, 16):
            x = int(cape_x + dx)
            if 0 <= x < w and is_land(px[x, y]):
                cape_ys.append(y)
                break
    print("cape land y range", min(cape_ys), max(cape_ys))
    cape_y = max(cape_ys)
    # lat_per_px: isotropic in our map (same deg/pixel if aspect compensated)
    # Our project uses span_lat = span_lon/aspect, so deg/px lon = span_lon/w, deg/px lat = span_lat/h
    # = span_lon/(aspect*h) = span_lon/w — isotropic in degrees per pixel. Good.
    lat_per_px = lon_per_px
    # cape_y corresponds to lat -34.83
    lat_at = lambda y: -34.83 + (cape_y - y) * lat_per_px
    print("lat at y0", lat_at(0), "y500", lat_at(500), "y595", lat_at(595))
    print("strip sample lon/lat", lon_at(630), lat_at(500))

    # Who owns strip pixels in GEO space?
    print("--- geo PIP strip samples ---")
    for y in [450, 500, 550]:
        for x in [600, 630, 660]:
            lon, lat = lon_at(x), lat_at(y)
            hits = [
                c["name"]
                for c in d["countries"]
                for r in c["rings"]
                if pip_lonlat(lon, lat, r)
            ]
            print(f"  px({x},{y}) = ({lon:.2f},{lat:.2f}) -> {hits}")

    # Screen-space fill of every visible country at strip ocean point
    center_lon = (lon_at(0) + lon_at(w)) / 2
    center_lat = (lat_at(0) + lat_at(h)) / 2
    span_lon = lon_at(w) - lon_at(0)
    span_lat = lat_at(0) - lat_at(h)
    print("inferred view center", center_lon, center_lat, "span", span_lon, span_lat)

    def project(lon: float, lat: float) -> tuple[float, float]:
        x = (lon - (center_lon - span_lon * 0.5)) / span_lon * w
        y = (1.0 - (lat - (center_lat - span_lat * 0.5)) / span_lat) * h
        return x, y

    test_pts = [(630, 500), (630, 550), (610, 480)]
    blame = {pt: [] for pt in test_pts}
    for c in d["countries"]:
        for ring in c["rings"]:
            pts = [project(p[0], p[1]) for p in ring]
            for pt in test_pts:
                if pip_xy(pt[0] + 0.5, pt[1] + 0.5, pts):
                    blame[pt].append(c["name"])
    print("screen PIP blame", blame)

    # Check South Africa ring: any long closing edge?
    sa = next(c for c in d["countries"] if c["name"] == "South Africa")
    for ri, ring in enumerate(sa["rings"]):
        print("SA ring", ri, "n", len(ring), "first", ring[0], "last", ring[-1])
        worst = None
        for i in range(len(ring) - 1):
            a, b = ring[i], ring[i + 1]
            dlat = abs(a[1] - b[1])
            dlon = abs(a[0] - b[0])
            if worst is None or dlat + dlon > worst[0]:
                worst = (dlat + dlon, i, a, b, dlat, dlon)
        print("  worst edge", worst)


def pip_lonlat(lon, lat, ring) -> bool:
    n = len(ring) - (1 if ring and ring[0] == ring[-1] else 0)
    inside = False
    j = n - 1
    for i in range(n):
        xi, yi = ring[i]
        xj, yj = ring[j]
        if ((yi > lat) != (yj > lat)) and (lon < (xj - xi) * (lat - yi) / (yj - yi + 1e-15) + xi):
            inside = not inside
        j = i
    return inside


if __name__ == "__main__":
    main()
