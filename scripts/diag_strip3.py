"""Find exact strip geometry and which projected rings cover ocean."""
from __future__ import annotations

import json
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
BAD = Path(r"C:\CursorTemp\map_bad.png")
DATA = ROOT / "data" / "LOCALES" / "MAP_COUNTRIES.json"


def main() -> None:
    d = json.loads(DATA.read_text(encoding="utf-8"))
    im = Image.open(BAD).convert("RGB")
    w, h = im.size
    px = im.load()

    def land(x, y):
        return px[x, y] == (0, 85, 0)

    # Row profiles: count and x-range of pure land
    print("y  count  xmin-xmax  runs")
    for y in list(range(0, h, 25)) + [300, 320, 340, 360, 380, 400, 420, 440, 460, 480, 500, 520, 540, 560, 580]:
        xs = [x for x in range(w) if land(x, y)]
        if not xs:
            print(f"{y:4d}     0")
            continue
        runs = []
        s = p = xs[0]
        for x in xs[1:]:
            if x == p + 1:
                p = x
            else:
                runs.append((s, p))
                s = p = x
        runs.append((s, p))
        print(f"{y:4d}  {len(xs):5d}  {xs[0]}-{xs[-1]}  {runs[:6]}{'...' if len(runs)>6 else ''}")

    # Lon from Madagascar
    lon_per_px = (50.4765 - 43.2542) / (787 - 753)
    lon_at0 = 43.2542 - 753 * lon_per_px

    # Find SA tip visually: southernmost y where land span width > 150px (continent not strip)
    tip_y = None
    for y in range(h - 1, -1, -1):
        xs = [x for x in range(w) if land(x, y)]
        if len(xs) > 150:
            tip_y = y
            print("continent tip y", y, "n", len(xs), "x", xs[0], xs[-1])
            break
    if tip_y is None:
        print("no wide continent row found")
        return

    # Cape Agulhas lat -34.83; tip_y is approx that
    lat_per_px = lon_per_px
    lat_at = lambda y: -34.83 + (tip_y - y) * lat_per_px
    lon_at = lambda x: lon_at0 + x * lon_per_px
    print("lat y0", lat_at(0), "tip", lat_at(tip_y), "bottom", lat_at(h - 1))
    print("view lon", lon_at(0), lon_at(w), "center", (lon_at(0) + lon_at(w)) / 2)

    # Ocean test point: middle of strip below tip
    # Find strip x range at tip_y+40
    y_ocean = min(h - 1, tip_y + 40)
    xs = [x for x in range(w) if land(x, y_ocean)]
    print("ocean row", y_ocean, "land xs runs:", end=" ")
    if xs:
        runs = []
        s = p = xs[0]
        for x in xs[1:]:
            if x == p + 1:
                p = x
            else:
                runs.append((s, p))
                s = p = x
        runs.append((s, p))
        print(runs)
        mid = (runs[0][0] + runs[0][1]) // 2
        print("test geo", lon_at(mid), lat_at(y_ocean))
    else:
        print("none")
        # try find any strip row below tip
        for y_ocean in range(tip_y + 1, h):
            xs = [x for x in range(w) if land(x, y_ocean)]
            if len(xs) > 20:
                print("found strip at", y_ocean, "n", len(xs), xs[0], xs[-1])
                mid = (xs[0] + xs[-1]) // 2
                print("test geo", lon_at(mid), lat_at(y_ocean))
                break

    center_lon = (lon_at(0) + lon_at(w)) / 2
    center_lat = (lat_at(0) + lat_at(h)) / 2
    span_lon = lon_at(w) - lon_at(0)
    span_lat = lat_at(0) - lat_at(h)
    print("center", center_lon, center_lat, "spans", span_lon, span_lat)

    def project(lon, lat):
        x = (lon - (center_lon - span_lon * 0.5)) / span_lon * w
        y = (1.0 - (lat - (center_lat - span_lat * 0.5)) / span_lat) * h
        return (x, y)

    def pip(x, y, ring):
        n = len(ring) - (
            1
            if ring
            and abs(ring[0][0] - ring[-1][0]) < 1e-6
            and abs(ring[0][1] - ring[-1][1]) < 1e-6
            else 0
        )
        inside = False
        j = n - 1
        for i in range(n):
            xi, yi = ring[i]
            xj, yj = ring[j]
            if ((yi > y) != (yj > y)) and (
                x < (xj - xi) * (y - yi) / (yj - yi + 1e-15) + xi
            ):
                inside = not inside
            j = i
        return inside

    # Test all strip land pixels below continent tip
    strip_pixels = []
    for y in range(tip_y + 5, h - 5):
        for x in range(w):
            if land(x, y):
                strip_pixels.append((x, y))
    print("strip pixels below tip", len(strip_pixels))
    if not strip_pixels:
        # maybe tip detection wrong — use y>400
        for y in range(400, h - 5):
            for x in range(w):
                if land(x, y):
                    strip_pixels.append((x, y))
        print("strip pixels y>400", len(strip_pixels))

    sample = strip_pixels[len(strip_pixels) // 2] if strip_pixels else (630, 500)
    print("sample strip px", sample, "geo", lon_at(sample[0]), lat_at(sample[1]))

    blame = {}
    for c in d["countries"]:
        for ring in c["rings"]:
            pts = [project(p[0], p[1]) for p in ring]
            if pip(sample[0] + 0.5, sample[1] + 0.5, pts):
                blame[c["name"]] = blame.get(c["name"], 0) + 1
    print("screen blame at sample", blame)

    # Also: which countries have ANY screen-space coverage of many strip pixels
    tallies = {}
    samples = strip_pixels[:: max(1, len(strip_pixels) // 30)] if strip_pixels else []
    for c in d["countries"]:
        for ring in c["rings"]:
            pts = [project(p[0], p[1]) for p in ring]
            hit = sum(1 for x, y in samples if pip(x + 0.5, y + 0.5, pts))
            if hit:
                tallies[c["name"]] = tallies.get(c["name"], 0) + hit
    print("tallies", sorted(tallies.items(), key=lambda kv: -kv[1])[:20])

    # List countries with huge lon bbox still filled
    print("wide bbox countries:")
    for c in d["countries"]:
        if c["bbox"][2] - c["bbox"][0] > 100:
            print(" ", c["name"], c["bbox"], "rings", len(c["rings"]))


if __name__ == "__main__":
    main()
