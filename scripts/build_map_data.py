#!/usr/bin/env python3
"""Build offline map JSON from Natural Earth (public domain).

Downloads 110m countries + places, plus filtered/dissolved 10m admin-1
(states, provinces, prefectures, Länder, governorates, UK nations,
FR/ES/IT régions, …) for Cursor `Region - Country` labels. Writes:
  data/LOCALES/MAP_COUNTRIES.json
  data/LOCALES/MAP_PLACES.json
  data/LOCALES/MAP_REGIONS.json

Run from the repo root:  python scripts/build_map_data.py
"""

from __future__ import annotations

import json
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "data" / "LOCALES"

COUNTRIES_URL = (
    "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/"
    "master/geojson/ne_110m_admin_0_countries.geojson"
)
PLACES_URL = (
    "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/"
    "master/geojson/ne_110m_populated_places.geojson"
)
# 110m/50m admin_1 is basically USA (+ a few). Europe needs 10m.
REGIONS_URL = (
    "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/"
    "master/geojson/ne_10m_admin_1_states_provinces.geojson"
)

# Keep rings usable at country zoom without megabytes of detail.
MAX_RING_POINTS = 160
MAX_REGION_RING_POINTS = 80
MIN_RING_POINTS = 4

# Natural Earth type_en values that are true first-level admin
# (state / province / prefecture / Land / governorate / …).
FIRST_LEVEL_TYPES: set[str] = {
    "State",
    "Province",
    "Prefecture",
    "Urban Prefecture",
    "Metropolis",
    "Circuit",
    "Region",
    "Canton",
    "Territory",
    "Governorate",
    "Voivodeship|Province",
    "Federal District",
    "Federal Territory",
    "Federal City",
    "Union Territory",
    "Autonomous Region",
    "Autonomous Province",
    "Autonomous Republic",
    "Autonomous Territory",
    "Republic",  # Russia federal subjects
    "Capital Region",
    "Capital Metropolitan City",
    "Metropolitan City",
    "Special self-governing city",
    "Capital City",
    "Capital District",
    "National District",
    "National Territory",
    "National Capital Area",
    "Captial District",  # NE typo (Peru)
    "Departamento",
    "Department",  # LATAM / Africa — not France (dissolved separately)
    "Intendancy",
    "Commissiary",
    "Division",
    "Administrative State",
    "Special region",
    "Special district",
    "Autonomous Province",
    "Regional Council",  # New Zealand
    "County",  # Nordic / Baltics / Balkans / Ireland — not UK
    "Urban county",
    "Oblast'",
    "Municipality|Governarate",  # Libya
    "District",  # only for allowlisted adm0 below
}

# County/District are first-level in these countries only.
COUNTY_ADM0 = {
    "SWE",
    "NOR",
    "FIN",
    "EST",
    "LTU",
    "ALB",
    "HRV",
    "IRL",
    "HUN",
    "ROU",
    "LBR",
}
DISTRICT_ADM0 = {
    "ISR",  # mehozot
}

# Skip: only municipalities / boroughs / NE type mess without a dissolve key.
SKIP_ADM0 = {
    "GBR",  # dissolved via geounit → England/Scotland/Wales/N.Ireland
    "FRA",  # dissolved via region → régions
    "ESP",  # dissolved via region → comunidades
    "ITA",  # dissolved via region → regioni
    "SVN",  # communes
    "LVA",  # municipalities
    "MKD",
    "MNE",
    "MLT",
    "HKG",
    "SGP",
    "SYC",
    "MDV",
    "PLW",
    "NRU",
    "AIA",
    "BMU",
    "COK",
    "WSM",
    "ALD",
    "LIE",
}

# Fine-grained NE units dissolved into coarser first-level names.
# FRA/ESP/ITA: property `region`.  GBR: property `geonunit`.
DISSOLVE_REGION_ADM0 = {"FRA", "ESP", "ITA"}
DISSOLVE_GEOUNIT_ADM0 = {"GBR"}


def fetch(url: str) -> dict:
    print(f"fetch {url}")
    with urllib.request.urlopen(url, timeout=300) as r:
        return json.loads(r.read().decode("utf-8"))


def dedupe(ring: list[list[float]]) -> list[list[float]]:
    out: list[list[float]] = []
    for p in ring:
        if out and abs(out[-1][0] - p[0]) < 1e-7 and abs(out[-1][1] - p[1]) < 1e-7:
            continue
        out.append(p)
    return out


def close_ring(ring: list[list[float]]) -> list[list[float]]:
    if len(ring) < 3:
        return ring
    if ring[0] != ring[-1]:
        ring = ring + [ring[0]]
    return ring


def split_dateline(ring: list[list[float]]) -> list[list[list[float]]]:
    """Cut a ring wherever consecutive vertices jump more than 90° of longitude.

    Antarctica's Natural Earth outline has one edge from lon 180 → −177; keeping
    that edge makes filled polygons paint a strip through the Atlantic.
    """
    ring = dedupe(ring)
    if len(ring) >= 2 and ring[0] == ring[-1]:
        ring = ring[:-1]
    if len(ring) < 3:
        return []
    parts: list[list[list[float]]] = []
    cur: list[list[float]] = [ring[0]]
    for p in ring[1:]:
        if abs(p[0] - cur[-1][0]) > 90.0:
            if len(cur) >= 3:
                parts.append(close_ring(cur))
            cur = [p]
        else:
            cur.append(p)
    if len(cur) >= 3:
        parts.append(close_ring(cur))
    return parts


def simplify_ring(ring: list[list[float]], max_pts: int) -> list[list[float]]:
    ring = dedupe(ring)
    if len(ring) >= 2 and ring[0] == ring[-1]:
        body, closer = ring[:-1], True
    else:
        body, closer = ring, False
    if len(body) <= max_pts:
        out = [[round(p[0], 4), round(p[1], 4)] for p in body]
    else:
        step = max(1, len(body) // max_pts)
        out = [body[i] for i in range(0, len(body), step)]
        if out[-1] != body[-1]:
            out.append(body[-1])
        out = [[round(p[0], 4), round(p[1], 4)] for p in out]
    if closer or (out and out[0] != out[-1]):
        out = close_ring(out)
    return out


def rings_from_geom(geom: dict, max_pts: int = MAX_RING_POINTS) -> list[list[list[float]]]:
    gtype = geom.get("type")
    coords = geom.get("coordinates") or []
    raw: list[list[list[float]]] = []
    if gtype == "Polygon":
        raw.extend(coords)
    elif gtype == "MultiPolygon":
        for poly in coords:
            # Outer ring only (index 0); skip holes for our retro fill.
            if poly:
                raw.append(poly[0])
    rings: list[list[list[float]]] = []
    for ring in raw:
        if len(ring) < MIN_RING_POINTS:
            continue
        for part in split_dateline(ring):
            for safe in ensure_safe_rings(part):
                simple = simplify_ring(safe, max_pts)
                if len(simple) >= MIN_RING_POINTS and ring_lon_span(simple) <= 180.0:
                    rings.append(simple)
    return rings


def ring_lon_span(ring: list[list[float]]) -> float:
    lons = [p[0] for p in ring]
    return max(lons) - min(lons)


def intersect_lon(a: list[float], b: list[float], lon: float) -> list[float]:
    denom = b[0] - a[0]
    if abs(denom) < 1e-12:
        return [lon, a[1]]
    t = (lon - a[0]) / denom
    return [lon, a[1] + t * (b[1] - a[1])]


def clip_halfplane(poly: list[list[float]], ge: bool, edge: float) -> list[list[float]]:
    if not poly:
        return []
    if poly[0] == poly[-1]:
        poly = poly[:-1]
    inside = (lambda lon: lon >= edge - 1e-9) if ge else (lambda lon: lon <= edge + 1e-9)
    out: list[list[float]] = []
    for i in range(len(poly)):
        s = poly[i]
        e = poly[(i + 1) % len(poly)]
        s_in = inside(s[0])
        e_in = inside(e[0])
        if s_in and e_in:
            out.append(e)
        elif s_in and not e_in:
            out.append(intersect_lon(s, e, edge))
        elif not s_in and e_in:
            out.append(intersect_lon(s, e, edge))
            out.append(e)
    return out


def clip_lon_band(ring: list[list[float]], lo: float, hi: float) -> list[list[float]]:
    clipped = clip_halfplane(ring, True, lo)
    clipped = clip_halfplane(clipped, False, hi)
    if len(clipped) < 3:
        return []
    return close_ring(dedupe(clipped))


def ensure_safe_rings(ring: list[list[float]]) -> list[list[list[float]]]:
    """Split globe-wrapping rings (Antarctica) into longitude bands.

    The map painter uses earcut (not egui's triangle-fan), so these bands fill
    as southern polar sectors instead of a spike through Africa.
    """
    if ring_lon_span(ring) <= 180.0:
        return [ring]
    out: list[list[list[float]]] = []
    # 90° bands keep each piece well under a hemisphere.
    for lo in range(-180, 180, 90):
        hi = lo + 90
        part = clip_lon_band(ring, float(lo), float(hi))
        if len(part) >= MIN_RING_POINTS and ring_lon_span(part) <= 180.0:
            out.append(part)
    return out


def bbox_of(rings: list[list[list[float]]]) -> list[float]:
    xs, ys = [], []
    for ring in rings:
        for x, y in ring:
            xs.append(x)
            ys.append(y)
    return [min(xs), min(ys), max(xs), max(ys)]


def ring_area(ring: list[list[float]]) -> float:
    a = 0.0
    for i in range(len(ring) - 1):
        a += ring[i][0] * ring[i + 1][1] - ring[i + 1][0] * ring[i][1]
    return abs(a) * 0.5


def ring_centroid(ring: list[list[float]]) -> list[float]:
    n = max(1, len(ring) - 1)
    pts = ring[:n]
    a = 0.0
    cx = 0.0
    cy = 0.0
    for i in range(len(pts)):
        j = (i + 1) % len(pts)
        cross = pts[i][0] * pts[j][1] - pts[j][0] * pts[i][1]
        a += cross
        cx += (pts[i][0] + pts[j][0]) * cross
        cy += (pts[i][1] + pts[j][1]) * cross
    a2 = a * 0.5
    if abs(a2) < 1e-12:
        return [
            round(sum(p[0] for p in pts) / len(pts), 4),
            round(sum(p[1] for p in pts) / len(pts), 4),
        ]
    return [round(cx / (6.0 * a2), 4), round(cy / (6.0 * a2), 4)]


def country_name(props: dict) -> str:
    for key in ("NAME", "NAME_EN", "ADMIN", "name"):
        v = props.get(key)
        if isinstance(v, str) and v.strip():
            return v.strip()
    return "Unknown"


def build_countries(geo: dict) -> list[dict]:
    out = []
    for feat in geo.get("features", []):
        props = feat.get("properties") or {}
        geom = feat.get("geometry") or {}
        rings = rings_from_geom(geom)
        if not rings:
            continue
        # Mainland first by polygon area (France before Guiana, etc.).
        rings.sort(key=ring_area, reverse=True)
        rings = rings[:12]
        label = ring_centroid(rings[0])
        out.append(
            {
                "name": country_name(props),
                "bbox": bbox_of(rings),
                "label": label,
                "rings": rings,
            }
        )
    out.sort(key=lambda c: c["name"])
    return out


def build_places(geo: dict) -> list[dict]:
    out = []
    for feat in geo.get("features", []):
        props = feat.get("properties") or {}
        geom = feat.get("geometry") or {}
        if geom.get("type") != "Point":
            continue
        coords = geom.get("coordinates") or []
        if len(coords) < 2:
            continue
        name = props.get("NAME") or props.get("name") or props.get("NAMEASCII")
        if not name:
            continue
        rank = props.get("SCALERANK", props.get("scalerank", 10))
        try:
            rank = int(rank)
        except (TypeError, ValueError):
            rank = 10
        # Keep only more significant places to keep the file small.
        if rank > 6:
            continue
        out.append(
            {
                "name": str(name).strip(),
                "lon": round(float(coords[0]), 4),
                "lat": round(float(coords[1]), 4),
                "rank": rank,
            }
        )
    out.sort(key=lambda p: (p["rank"], p["name"]))
    return out


def region_type_wanted(a3: str, type_en: str) -> bool:
    if not type_en or type_en == "?":
        return False
    if type_en == "County" or type_en == "Urban county":
        return a3 in COUNTY_ADM0
    if type_en == "District":
        return a3 in DISTRICT_ADM0
    if type_en == "Special Municipality":
        # Cuba's Isla de la Juventud; skip NL Caribbean specials.
        return a3 == "CUB"
    if type_en == "Department":
        # France departments are dissolved; keep LATAM/Africa departments.
        return a3 not in DISSOLVE_REGION_ADM0
    return type_en in FIRST_LEVEL_TYPES


def append_region(out: list[dict], name: str, admin: str, rings: list) -> None:
    if not name or not rings:
        return
    rings = sorted(rings, key=ring_area, reverse=True)[:12]
    out.append(
        {
            "name": name,
            "admin": admin,
            "bbox": bbox_of(rings),
            "label": ring_centroid(rings[0]),
            "rings": rings,
        }
    )


def build_regions(geo: dict) -> list[dict]:
    """First-level admin worldwide for Cursor: state / province / prefecture / …

    Most countries use NE type_en. France, Spain, Italy dissolve departments /
    provinces into `region`; UK dissolves into England/Scotland/Wales/N.Ireland.
    """
    out: list[dict] = []
    # (adm0_a3, dissolve_key) -> {admin, rings}
    buckets: dict[tuple[str, str], dict] = {}

    for feat in geo.get("features", []):
        props = feat.get("properties") or {}
        a3 = (props.get("adm0_a3") or props.get("ADM0_A3") or "").strip().upper()
        if not a3:
            continue
        admin = (props.get("admin") or props.get("ADMIN") or "").strip()
        geom = feat.get("geometry") or {}
        rings = rings_from_geom(geom, MAX_REGION_RING_POINTS)
        if not rings:
            continue

        if a3 in DISSOLVE_REGION_ADM0:
            key = (props.get("region") or "").strip()
            if not key:
                continue
            bucket = buckets.setdefault((a3, key), {"admin": admin, "rings": []})
            bucket["rings"].extend(rings)
            continue

        if a3 in DISSOLVE_GEOUNIT_ADM0:
            key = (
                props.get("geonunit") or props.get("geounit") or ""
            ).strip()
            if not key:
                continue
            bucket = buckets.setdefault((a3, key), {"admin": admin, "rings": []})
            bucket["rings"].extend(rings)
            continue

        if a3 in SKIP_ADM0:
            continue

        type_en = (props.get("type_en") or props.get("type") or "").strip()
        if not region_type_wanted(a3, type_en):
            continue

        append_region(out, country_name(props), admin, rings)

    for (a3, key), bucket in buckets.items():
        append_region(out, key, bucket["admin"], bucket["rings"])

    out.sort(key=lambda r: (r["admin"], r["name"]))
    return out


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    countries = build_countries(fetch(COUNTRIES_URL))
    places = build_places(fetch(PLACES_URL))
    regions = build_regions(fetch(REGIONS_URL))
    cpath = OUT / "MAP_COUNTRIES.json"
    ppath = OUT / "MAP_PLACES.json"
    rpath = OUT / "MAP_REGIONS.json"
    cpath.write_text(json.dumps({"countries": countries}, separators=(",", ":")), encoding="utf-8")
    ppath.write_text(json.dumps({"places": places}, separators=(",", ":")), encoding="utf-8")
    rpath.write_text(json.dumps({"regions": regions}, separators=(",", ":")), encoding="utf-8")
    print(f"wrote {cpath} ({cpath.stat().st_size} bytes, {len(countries)} countries)")
    print(f"wrote {ppath} ({ppath.stat().st_size} bytes, {len(places)} places)")
    print(f"wrote {rpath} ({rpath.stat().st_size} bytes, {len(regions)} regions)")


if __name__ == "__main__":
    main()
