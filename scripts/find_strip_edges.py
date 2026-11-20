#!/usr/bin/env python3
import json
from pathlib import Path

d = json.loads(Path("data/LOCALES/MAP_COUNTRIES.json").read_text(encoding="utf-8"))

print("=== points south of -40 outside Antarctica ===")
for c in d["countries"]:
    if "Antarct" in c["name"]:
        continue
    for i, r in enumerate(c["rings"]):
        for p in r:
            if p[1] < -40:
                print(c["name"], "r", i, p)

print("=== long N-S edges near 30-50E ===")
for c in d["countries"]:
    for i, r in enumerate(c["rings"]):
        for j in range(len(r) - 1):
            a, b = r[j], r[j + 1]
            dlat = abs(b[1] - a[1])
            dlon = abs(b[0] - a[0])
            mid = (a[0] + b[0]) / 2
            if dlon < 20 and dlat > 8 and 25 <= mid <= 55:
                print(
                    f"{c['name']} r{i} e{j}: "
                    f"({a[0]:.2f},{a[1]:.2f})->({b[0]:.2f},{b[1]:.2f}) "
                    f"dlat={dlat:.1f} dlon={dlon:.1f}"
                )

print("=== countries with lon span>100 or max edge dlon>40 ===")
for c in d["countries"]:
    for i, r in enumerate(c["rings"]):
        lons = [p[0] for p in r]
        span = max(lons) - min(lons)
        md = max(abs(r[j + 1][0] - r[j][0]) for j in range(len(r) - 1))
        if span > 100 or md > 40:
            print(c["name"], "r", i, "span", round(span, 1), "max_edge", round(md, 1))
