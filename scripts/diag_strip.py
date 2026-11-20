"""Diagnose green strip south of Africa in map screenshots."""
from __future__ import annotations

import json
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
BAD = Path(r"C:\CursorTemp\map_bad.png")
OK = Path(r"C:\CursorTemp\map_ok.png")
DATA = ROOT / "data" / "LOCALES" / "MAP_COUNTRIES.json"


def is_land(c: tuple[int, int, int]) -> bool:
    r, g, b = c
    return g > 80 and g > r + 20 and g > b + 20 and b < 50


def pip(lon: float, lat: float, ring: list) -> bool:
    n = len(ring) - (1 if ring and ring[0] == ring[-1] else 0)
    inside = False
    j = n - 1
    for i in range(n):
        xi, yi = ring[i]
        xj, yj = ring[j]
        if ((yi > lat) != (yj > lat)) and (
            lon < (xj - xi) * (lat - yi) / (yj - yi + 1e-15) + xi
        ):
            inside = not inside
        j = i
    return inside


def main() -> None:
    d = json.loads(DATA.read_text(encoding="utf-8"))
    im = Image.open(BAD).convert("RGB")
    w, h = im.size
    px = im.load()
    print("bad size", w, h)

    # Strip ocean band: y=500, green x
    xs = [x for x in range(w) if is_land(px[x, 500])]
    print("y500 land x", xs[0], xs[-1], "width", xs[-1] - xs[0] + 1, "color", px[xs[len(xs) // 2], 500])

    # Madagascar run for lon calibration (rightmost green cluster at mid-lat)
    xs250 = [x for x in range(w) if is_land(px[x, 250])]
    runs: list[tuple[int, int]] = []
    s = p = xs250[0]
    for x in xs250[1:]:
        if x == p + 1:
            p = x
        else:
            runs.append((s, p))
            s = p = x
    runs.append((s, p))
    print("y250 green runs", runs)

    # Infer view: use SA tip ~ lon 20, lat -35 at bottom of contiguous africa
    # Find y where main continent ends (wide green shrinks to strip only)
    for y in range(300, 450):
        xs = [x for x in range(w) if is_land(px[x, y])]
        if not xs:
            print("first empty land row", y)
            break
        runs = []
        s = p = xs[0]
        for x in xs[1:]:
            if x == p + 1:
                p = x
            else:
                runs.append((s, p))
                s = p = x
        runs.append((s, p))
        if y % 20 == 0 or len(runs) <= 2:
            print(f"y{y} runs {[(a,b,b-a+1) for a,b in runs]}")

    print("--- ocean PIP (no country should hit) ---")
    for lat in range(-36, -60, -2):
        for lon in range(10, 40, 2):
            hits = [
                c["name"]
                for c in d["countries"]
                for r in c["rings"]
                if pip(lon, lat, r)
            ]
            if hits:
                print(lon, lat, hits)

    print("--- screen-space fan spike check ---")
    # Fake a view similar to bad shot: center ~25E, -10; span_lon ~90
    center_lon, center_lat, span_lon = 25.0, -12.0, 95.0
    aspect = w / h
    span_lat = span_lon / aspect

    def project(lon: float, lat: float) -> tuple[float, float]:
        x = (lon - (center_lon - span_lon * 0.5)) / span_lon * w
        y = (1.0 - (lat - (center_lat - span_lat * 0.5)) / span_lat) * h
        return x, y

    # Rasterize each country into an offscreen buffer for southern ocean rows
    # and report who paints the strip columns
    strip_x0, strip_x1 = 591, 668
    blame: dict[str, int] = {}
    for c in d["countries"]:
        # bbox cull like map.rs
        half_lon = span_lon * 0.5 + 2
        half_lat = span_lat * 0.5 + 2
        bb = c["bbox"]
        if bb[2] < center_lon - half_lon or bb[0] > center_lon + half_lon:
            continue
        if bb[3] < center_lat - half_lat or bb[1] > center_lat + half_lat:
            continue
        antarctic = c["name"].lower() == "antarctica" or (
            bb[3] < -55.0 and (bb[2] - bb[0]) > 40.0
        )
        for ring in c["rings"]:
            if antarctic:
                continue
            # simple scanline fill of projected polygon for y=500 only
            pts = [project(p[0], p[1]) for p in ring]
            # ray cast in screen space for strip pixels
            for x in range(strip_x0, strip_x1 + 1):
                if pip_xy(x + 0.5, 500.5, pts):
                    blame[c["name"]] = blame.get(c["name"], 0) + 1
    print("who paints strip at y500 (view guess):", sorted(blame.items(), key=lambda kv: -kv[1])[:15])
    print("view span_lat", span_lat, "lat range", center_lat - span_lat / 2, center_lat + span_lat / 2)


def pip_xy(x: float, y: float, ring: list[tuple[float, float]]) -> bool:
    n = len(ring) - (1 if ring and ring[0] == ring[-1] else 0)
    inside = False
    j = n - 1
    for i in range(n):
        xi, yi = ring[i]
        xj, yj = ring[j]
        if ((yi > y) != (yj > y)) and (x < (xj - xi) * (y - yi) / (yj - yi + 1e-15) + xi):
            inside = not inside
        j = i
    return inside


if __name__ == "__main__":
    main()
