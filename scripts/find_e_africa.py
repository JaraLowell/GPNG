#!/usr/bin/env python3
import json
from pathlib import Path

d = json.loads(Path("data/LOCALES/MAP_COUNTRIES.json").read_text(encoding="utf-8"))
names = (
    "Mozambique",
    "Tanzania",
    "Madagascar",
    "Malawi",
    "Zambia",
    "Zimbabwe",
    "South Africa",
    "Kenya",
    "Somalia",
    "Russia",
)
for c in d["countries"]:
    if c["name"] not in names and "Moz" not in c["name"] and "Tanz" not in c["name"]:
        continue
    print("===", c["name"], "bbox", c["bbox"], "rings", len(c["rings"]))
    for i, r in enumerate(c["rings"]):
        lats = [p[1] for p in r]
        lons = [p[0] for p in r]
        md = max(abs(r[j + 1][0] - r[j][0]) for j in range(len(r) - 1))
        mlat = max(abs(r[j + 1][1] - r[j][1]) for j in range(len(r) - 1))
        print(
            f"  r{i} n={len(r)} lat={min(lats):.2f}..{max(lats):.2f} "
            f"lon={min(lons):.2f}..{max(lons):.2f} max_dlon={md:.2f} max_dlat={mlat:.2f}"
        )
        # tip of Mozambique / south points near 40E
        south = sorted(r, key=lambda p: p[1])[:3]
        print("  southernmost", south)
