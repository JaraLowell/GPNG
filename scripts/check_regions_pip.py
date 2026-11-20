import json
from pathlib import Path

d = json.loads(Path("data/LOCALES/MAP_REGIONS.json").read_text(encoding="utf-8"))


def pip(lon, lat, ring):
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


def find(lon, lat):
    hits = []
    for r in d["regions"]:
        bb = r["bbox"]
        if lon < bb[0] or lon > bb[2] or lat < bb[1] or lat > bb[3]:
            continue
        if any(pip(lon, lat, ring) for ring in r["rings"]):
            hits.append(f"{r['name']} - {r['admin']}")
    return hits


for name, lon, lat in [
    ("Groningen", 6.57, 53.22),
    ("Idaho", -114.7, 44.0),
    ("Bayern", 11.5, 48.5),
    ("Tokyo", 139.7, 35.7),
    ("Normandie", 0.1, 49.0),
    ("Catalunya", 2.1, 41.4),
    ("Lombardia", 9.2, 45.5),
    ("Scotland", -4.0, 56.5),
    ("California", -120.0, 37.0),
]:
    print(name, find(lon, lat)[:3])
