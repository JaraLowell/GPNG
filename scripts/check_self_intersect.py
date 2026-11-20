"""Find self-intersecting country rings (geo space)."""
from __future__ import annotations

import json
from pathlib import Path

DATA = Path(__file__).resolve().parents[1] / "data" / "LOCALES" / "MAP_COUNTRIES.json"


def orient(a, b, c) -> float:
    return (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])


def on_seg(a, b, c, eps=1e-9) -> bool:
    return (
        min(a[0], b[0]) - eps <= c[0] <= max(a[0], b[0]) + eps
        and min(a[1], b[1]) - eps <= c[1] <= max(a[1], b[1]) + eps
    )


def segments_intersect(a, b, c, d) -> bool:
    o1, o2 = orient(a, b, c), orient(a, b, d)
    o3, o4 = orient(c, d, a), orient(c, d, b)
    if o1 * o2 < 0 and o3 * o4 < 0:
        return True
    # skip colinear touches for now
    return False


def ring_self_intersects(ring: list) -> list[tuple]:
    body = ring[:-1] if ring and ring[0] == ring[-1] else list(ring)
    n = len(body)
    hits = []
    for i in range(n):
        a, b = body[i], body[(i + 1) % n]
        for j in range(i + 1, n):
            if abs(i - j) <= 1 or (i == 0 and j == n - 1) or (j == 0 and i == n - 1):
                continue
            # also skip adjacent wrap already handled
            if (j + 1) % n == i:
                continue
            c, d = body[j], body[(j + 1) % n]
            if segments_intersect(a, b, c, d):
                hits.append((i, j, a, b, c, d))
                if len(hits) >= 5:
                    return hits
    return hits


def main() -> None:
    d = json.loads(DATA.read_text(encoding="utf-8"))
    for c in d["countries"]:
        for ri, ring in enumerate(c["rings"]):
            hits = ring_self_intersects(ring)
            if hits:
                print(c["name"], "ring", ri, "n", len(ring), "intersections", len(hits))
                print("  eg", hits[0][:4])


if __name__ == "__main__":
    main()
