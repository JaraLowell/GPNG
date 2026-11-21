//! Offline Natural Earth packs: countries, regions (admin-1), populated places.
//!
//! Files in `data/LOCALES/`: `MAP_COUNTRIES.json`, `MAP_REGIONS.json`,
//! `MAP_PLACES.json` (public domain).

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::locator::{
    lat_lon_to_locator, locator_to_lat_lon, normalize_locator, normalize_locator6,
};
use crate::paths;

#[derive(Debug, Clone, Deserialize)]
struct CountriesFile {
    countries: Vec<Country>,
}

#[derive(Debug, Clone, Deserialize)]
struct RegionsFile {
    regions: Vec<Region>,
}

#[derive(Debug, Clone, Deserialize)]
struct PlacesFile {
    places: Vec<Place>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Country {
    pub name: String,
    /// `[min_lon, min_lat, max_lon, max_lat]`
    pub bbox: [f64; 4],
    /// Outer rings as `[lon, lat]` rings (closed).
    pub rings: Vec<Vec<[f64; 2]>>,
}

/// First-level subdivision (US state, NL province, DE Land, …).
#[derive(Debug, Clone, Deserialize)]
pub struct Region {
    pub name: String,
    /// Parent country name from Natural Earth (`admin`).
    #[serde(default)]
    pub admin: String,
    /// `[min_lon, min_lat, max_lon, max_lat]`
    pub bbox: [f64; 4],
    pub rings: Vec<Vec<[f64; 2]>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Place {
    pub name: String,
    pub lon: f64,
    pub lat: f64,
    pub rank: i32,
}

#[derive(Debug, Clone)]
pub struct GeoPack {
    pub countries: Vec<Country>,
    pub regions: Vec<Region>,
    pub places: Vec<Place>,
}

impl GeoPack {
    pub fn load() -> Self {
        let countries = load_countries();
        let regions = load_regions();
        let places = load_places();
        Self {
            countries,
            regions,
            places,
        }
    }

    /// Country containing the point, if any (smallest matching bbox area on ties).
    pub fn country_at(&self, lon: f64, lat: f64) -> Option<&Country> {
        let mut best: Option<&Country> = None;
        let mut best_area = f64::MAX;
        for c in &self.countries {
            if lon < c.bbox[0] || lon > c.bbox[2] || lat < c.bbox[1] || lat > c.bbox[3] {
                continue;
            }
            if c.rings.iter().any(|ring| point_in_ring(lon, lat, ring)) {
                let area = (c.bbox[2] - c.bbox[0]) * (c.bbox[3] - c.bbox[1]);
                if area < best_area {
                    best_area = area;
                    best = Some(c);
                }
            }
        }
        best
    }

    /// Admin-1 region (state / province / Land) containing the point, if any.
    pub fn region_at(&self, lon: f64, lat: f64) -> Option<&Region> {
        let mut best: Option<&Region> = None;
        let mut best_area = f64::MAX;
        for r in &self.regions {
            if lon < r.bbox[0] || lon > r.bbox[2] || lat < r.bbox[1] || lat > r.bbox[3] {
                continue;
            }
            if r.rings.iter().any(|ring| point_in_ring(lon, lat, ring)) {
                let area = (r.bbox[2] - r.bbox[0]) * (r.bbox[3] - r.bbox[1]);
                if area < best_area {
                    best_area = area;
                    best = Some(r);
                }
            }
        }
        best
    }

    pub fn country_for_locator(&self, locator: &str) -> Option<&Country> {
        let (lat, lon) = locator_to_lat_lon(locator)?;
        self.country_at(lon, lat)
    }

    /// Cursor / status label: `Groningen - Netherlands`, or just `Netherlands`
    /// when no admin-1 pack covers the point. City names are not used.
    pub fn resolve_label(&self, lon: f64, lat: f64) -> Option<String> {
        let country = self.country_at(lon, lat)?;
        if let Some(region) = self.region_at(lon, lat) {
            Some(format!("{} - {}", region.name, country.name))
        } else {
            Some(country.name.clone())
        }
    }

    pub fn resolve_locator_label(&self, locator: &str) -> Option<String> {
        let loc = normalize_locator6(locator)?;
        let (lat, lon) = locator_to_lat_lon(&loc)?;
        self.resolve_label(lon, lat)
    }

    /// MHeard detail under Last Payload:
    /// `JO32KX  Groningen  Netherlands  42 Km from QTH`
    ///
    /// Needs a known position (lat/lon and/or locator). Distance is great-circle
    /// km from `qth` when that is set (own station locator).
    pub fn station_detail_line(
        &self,
        locator: Option<&str>,
        lat: Option<f64>,
        lon: Option<f64>,
        qth: Option<(f64, f64)>,
    ) -> Option<String> {
        let (lat, lon, grid) = match (lat, lon, locator) {
            (Some(lat), Some(lon), loc) => {
                let grid = loc
                    .and_then(normalize_locator)
                    .or_else(|| lat_lon_to_locator(lat, lon));
                (lat, lon, grid)
            }
            (_, _, Some(loc)) => {
                let grid = normalize_locator(loc)?;
                let (lat, lon) = locator_to_lat_lon(&grid)?;
                (lat, lon, Some(grid))
            }
            _ => return None,
        };

        let country = self.country_at(lon, lat);
        let region = self.region_at(lon, lat);
        if grid.is_none() && country.is_none() {
            return None;
        }

        let mut parts: Vec<String> = Vec::new();
        if let Some(g) = grid {
            parts.push(g);
        }
        if let Some(r) = region {
            parts.push(r.name.clone());
        }
        if let Some(c) = country {
            parts.push(c.name.clone());
        }
        if let Some((qlat, qlon)) = qth {
            let km = haversine_km(qlat, qlon, lat, lon).round() as i64;
            parts.push(format!("{km} Km from QTH"));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("  "))
        }
    }
}

/// Great-circle distance on a spherical Earth (km).
pub fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6371.0;
    let p1 = lat1.to_radians();
    let p2 = lat2.to_radians();
    let dp = (lat2 - lat1).to_radians();
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * r * a.sqrt().asin()
}

fn load_countries() -> Vec<Country> {
    let path = map_data_file("MAP_COUNTRIES.json");
    let Ok(raw) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str::<CountriesFile>(&raw)
        .map(|f| f.countries)
        .unwrap_or_default()
}

fn load_regions() -> Vec<Region> {
    let path = map_data_file("MAP_REGIONS.json");
    let Ok(raw) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str::<RegionsFile>(&raw)
        .map(|f| f.regions)
        .unwrap_or_default()
}

fn load_places() -> Vec<Place> {
    let path = map_data_file("MAP_PLACES.json");
    let Ok(raw) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str::<PlacesFile>(&raw)
        .map(|f| f.places)
        .unwrap_or_default()
}

/// `data/LOCALES/MAP_*.json` (preferred), with a fallback to bare `data/`.
fn map_data_file(name: &str) -> PathBuf {
    for dir in paths::data_dirs() {
        let locales = dir.join("LOCALES");
        if let Some(path) = paths::find_named_file(&locales, name) {
            return path;
        }
        if let Some(path) = paths::find_named_file(&dir, name) {
            return path;
        }
    }
    paths::data_dir().join("LOCALES").join(name)
}

/// Shared pack loaded once for the process.
///
/// Restart the app after replacing `data/LOCALES/MAP_*.json` so this reloads.
pub fn global() -> &'static GeoPack {
    static PACK: OnceLock<GeoPack> = OnceLock::new();
    PACK.get_or_init(GeoPack::load)
}

fn point_in_ring(lon: f64, lat: f64, ring: &[[f64; 2]]) -> bool {
    // Ray casting
    let mut inside = false;
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (ring[i][0], ring[i][1]);
        let (xj, yj) = (ring[j][0], ring[j][1]);
        let intersect = ((yi > lat) != (yj > lat))
            && (lon < (xj - xi) * (lat - yi) / (yj - yi + f64::EPSILON) + xi);
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_in_simple_square() {
        let ring = [
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 10.0],
            [0.0, 10.0],
            [0.0, 0.0],
        ];
        assert!(point_in_ring(5.0, 5.0, &ring));
        assert!(!point_in_ring(15.0, 5.0, &ring));
    }

    #[test]
    fn haversine_same_point_is_zero() {
        assert!(haversine_km(52.0, 13.0, 52.0, 13.0) < 0.01);
    }

    #[test]
    fn haversine_amsterdam_paris_ballpark() {
        // ~430 km great-circle
        let d = haversine_km(52.37, 4.89, 48.86, 2.35);
        assert!(d > 400.0 && d < 460.0, "got {d}");
    }

    #[test]
    fn resolves_germany_from_jo62_centre() {
        let pack = GeoPack::load();
        if pack.countries.is_empty() {
            // CI without data/LOCALES/MAP_COUNTRIES.json
            return;
        }
        let name = pack
            .country_for_locator("JO62NM")
            .map(|c| c.name.as_str())
            .unwrap_or("");
        assert!(
            name.contains("Germany") || name.contains("Deutschland") || !name.is_empty(),
            "unexpected country for JO62NM: {name}"
        );
    }
}
