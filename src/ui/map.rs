//! Offline retro world map: countries, Maidenhead grid (to 6-char), markers.

use egui::epaint::Mesh;
use egui::{Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};

use crate::geo::GeoPack;
use crate::locator::{lat_lon_to_locator, locator_to_lat_lon, normalize_locator};

/// Pan / zoom state for the Map pane.
#[derive(Debug, Clone)]
pub struct MapView {
    /// Longitude of viewport centre (−180..180).
    pub center_lon: f64,
    /// Latitude of viewport centre (−90..90).
    pub center_lat: f64,
    /// Degrees of longitude spanned by the pane width.
    pub span_lon: f64,
    /// Max zoom-out (= last `fit_world` span). Never zoom out past the landing world.
    pub world_span_lon: f64,
}

impl Default for MapView {
    fn default() -> Self {
        Self {
            center_lon: 0.0,
            center_lat: 0.0,
            // Wide enough for a typical pane; first paint calls `fit_world`.
            span_lon: 360.0,
            world_span_lon: 360.0,
        }
    }
}

impl MapView {
    /// Closest zoom (~one 6-char cell across the pane).
    pub const SPAN_MIN: f64 = 0.5;
    /// Absolute ceiling used only while computing `fit_world` for very wide panes.
    const FIT_SPAN_CAP: f64 = 720.0;

    /// ~5% per mouse-wheel notch — many steps from world down to SPAN_MIN.
    const ZOOM_STEP: f64 = 0.95;
    /// egui wheel delta units treated as one notch (Windows is often ~50).
    const SCROLL_PER_NOTCH: f32 = 50.0;

    pub fn span_lat(&self, aspect: f64) -> f64 {
        // Keep °/pixel isotropic. May exceed 180° so a non-2:1 pane letterboxes
        // the globe with sea above/below instead of stretching.
        (self.span_lon / aspect.max(0.1)).max(0.25)
    }

    /// Longitude span that shows the full world for this pane aspect (landing zoom).
    pub fn world_span_for_aspect(aspect: f64) -> f64 {
        let aspect = aspect.max(0.1);
        (180.0 * aspect).max(360.0).min(Self::FIT_SPAN_CAP)
    }

    /// Centre the full world (−180…180, −90…90) in the pane.
    pub fn fit_world(&mut self, aspect: f64) {
        self.center_lon = 0.0;
        self.center_lat = 0.0;
        self.span_lon = Self::world_span_for_aspect(aspect);
        self.world_span_lon = self.span_lon;
    }

    pub fn zoom_at(&mut self, factor: f64, pivot_lon: f64, pivot_lat: f64) {
        let max_span = self.world_span_lon.max(360.0);
        let old = self.span_lon.max(1e-6);
        self.span_lon = (self.span_lon * factor).clamp(Self::SPAN_MIN, max_span);
        if (self.span_lon - max_span).abs() < 0.05 {
            // Fully zoomed out: same framing as the landing world view.
            self.center_lon = 0.0;
            self.center_lat = 0.0;
        } else {
            // Keep pivot under the cursor.
            let t = 1.0 - self.span_lon / old;
            self.center_lon += (pivot_lon - self.center_lon) * t;
            self.center_lat += (pivot_lat - self.center_lat) * t;
        }
        self.clamp();
    }

    /// Apply a wheel/trackpad scroll delta with gentle per-notch steps.
    pub fn zoom_by_scroll(&mut self, scroll_y: f32, pivot_lon: f64, pivot_lat: f64) {
        if scroll_y.abs() < 0.1 {
            return;
        }
        // Cap so one fling cannot jump several “gears” at once.
        let notches = (scroll_y / Self::SCROLL_PER_NOTCH).clamp(-1.5, 1.5) as f64;
        let step = Self::ZOOM_STEP.powf(notches.abs());
        // egui: positive scroll_y = scroll up = zoom in.
        let factor = if scroll_y > 0.0 { step } else { 1.0 / step };
        self.zoom_at(factor, pivot_lon, pivot_lat);
    }

    pub fn pan_deg(&mut self, dlon: f64, dlat: f64) {
        self.center_lon += dlon;
        self.center_lat += dlat;
        self.clamp();
    }

    pub fn clamp(&mut self) {
        self.center_lon = ((self.center_lon + 180.0).rem_euclid(360.0)) - 180.0;
        self.center_lat = self.center_lat.clamp(-90.0, 90.0);
        let max_span = self.world_span_lon.max(360.0);
        self.span_lon = self.span_lon.clamp(Self::SPAN_MIN, max_span);
    }

    pub fn fit_locator(&mut self, locator: &str) {
        if let Some((lat, lon)) = locator_to_lat_lon(locator) {
            self.center_lat = lat;
            self.center_lon = lon;
            self.span_lon = 8.0;
            self.clamp();
        }
    }

    pub fn project(&self, lon: f64, lat: f64, rect: Rect) -> Pos2 {
        let aspect = rect.width() / rect.height().max(1.0);
        let span_lat = self.span_lat(aspect as f64);
        let x = ((lon - (self.center_lon - self.span_lon * 0.5)) / self.span_lon) as f32;
        let y = 1.0 - ((lat - (self.center_lat - span_lat * 0.5)) / span_lat) as f32;
        Pos2::new(
            rect.left() + x * rect.width(),
            rect.top() + y * rect.height(),
        )
    }

    pub fn unproject(&self, pos: Pos2, rect: Rect) -> (f64, f64) {
        let aspect = rect.width() / rect.height().max(1.0);
        let span_lat = self.span_lat(aspect as f64);
        let x = ((pos.x - rect.left()) / rect.width()) as f64;
        let y = ((pos.y - rect.top()) / rect.height()) as f64;
        let lon = self.center_lon - self.span_lon * 0.5 + x * self.span_lon;
        let lat = self.center_lat + span_lat * 0.5 - y * span_lat;
        (lon, lat)
    }

}

#[derive(Debug, Clone, Copy)]
pub struct MapColors {
    pub background: Color32,
    pub land: Color32,
    pub grid: Color32,
    pub qth: Color32,
    pub heard: Color32,
}

#[derive(Debug, Clone)]
pub struct MapMarker {
    pub call: String,
    pub lon: f64,
    pub lat: f64,
    pub locator: String,
    pub is_qth: bool,
}

/// Paint the map into an already-allocated `rect` and handle pan/zoom.
/// Returns hover locator if any.
pub fn paint_map(
    ui: &mut Ui,
    rect: Rect,
    view: &mut MapView,
    geo: &GeoPack,
    colors: MapColors,
    markers: &[MapMarker],
    font: FontId,
) -> Option<String> {
    let resp = ui.interact(rect, ui.id().with("map_pane"), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, colors.background);

    // Pan
    if resp.dragged() {
        let delta = resp.drag_delta();
        let aspect = rect.width() / rect.height().max(1.0);
        let span_lat = view.span_lat(aspect as f64);
        let dlon = -(delta.x as f64 / rect.width() as f64) * view.span_lon;
        let dlat = (delta.y as f64 / rect.height() as f64) * span_lat;
        view.pan_deg(dlon, dlat);
    }

    // Zoom toward pointer — gentle steps; max out = landing world.
    let aspect = rect.width() / rect.height().max(1.0);
    view.world_span_lon = MapView::world_span_for_aspect(aspect as f64);
    if view.span_lon > view.world_span_lon {
        view.span_lon = view.world_span_lon;
    }
    let scroll = ui.input(|i| i.raw_scroll_delta.y);
    if scroll.abs() > 0.1 && resp.hovered() {
        let pointer = ui.input(|i| i.pointer.hover_pos()).unwrap_or(rect.center());
        let (lon, lat) = view.unproject(pointer, rect);
        view.zoom_by_scroll(scroll, lon, lat);
    }

    // Countries — filled DarkGreen land on Blue sea, thin coast stroke.
    // IMPORTANT: do not use PathShape fill — egui fans from vertex 0, which
    // mis-fills concave coasts (green strip down the 40°E / K–L meridian off
    // Mozambique when zoomed out).
    let half_lon = view.span_lon * 0.5 + 2.0;
    let aspect = rect.width() / rect.height().max(1.0);
    let half_lat = view.span_lat(aspect as f64) * 0.5 + 2.0;
    let coast = Stroke::new(1.0, Color32::from_rgb(0, 60, 0));
    let label_fg = Color32::from_rgb(220, 220, 180);
    let mut label_cands: Vec<CountryLabel> = Vec::new();
    for c in &geo.countries {
        if c.bbox[2] < view.center_lon - half_lon
            || c.bbox[0] > view.center_lon + half_lon
            || c.bbox[3] < view.center_lat - half_lat
            || c.bbox[1] > view.center_lat + half_lat
        {
            continue;
        }
        for ring in &c.rings {
            let ring = dedupe_ring_points(ring);
            if ring.len() < 3 {
                continue;
            }
            if ring_crosses_dateline(&ring) {
                paint_ring_outline_no_wrap(&painter, view, &ring, rect, coast);
                continue;
            }
            // Polar lon-band slices (Antarctica mainland) are valid earcut
            // sectors near the pole — fill them. Skip only true dateline wraps.
            paint_ring_filled_earcut(&painter, view, &ring, rect, colors.land, coast);
        }
        // Label at the largest ring's centroid (not full bbox — France+Guiana
        // would otherwise sit in the Atlantic and vanish when zoomed on Europe).
        if let Some((clon, clat, ring_w_deg, ring_h_deg)) = largest_ring_label_anchor(&c.rings) {
            let screen_w = (ring_w_deg / view.span_lon) * rect.width() as f64;
            let screen_h = (ring_h_deg / view.span_lat(aspect as f64)) * rect.height() as f64;
            if screen_w > 36.0 && screen_h > 14.0 {
                let pos = view.project(clon, clat, rect);
                if rect.contains(pos) {
                    label_cands.push(CountryLabel {
                        name: c.name.clone(),
                        pos,
                        area: screen_w * screen_h,
                    });
                }
            }
        }
    }

    // Regions are not drawn — Cursor / MHeard status resolve them via geo.
    paint_country_labels(&painter, &mut label_cands, font.clone(), label_fg);

    // No Maidenhead grid/labels — they crowd the map once many stations are plotted.
    // Hover still reports the locator under the cursor (status line).

    // Markers — QTH: VGA red dot only. Heard: green + call centered above.
    const QTH_RED: Color32 = Color32::from_rgb(170, 0, 0);
    for m in markers {
        let pos = view.project(m.lon, m.lat, rect);
        if !rect.expand(8.0).contains(pos) {
            continue;
        }
        if m.is_qth {
            let r = 5.0;
            painter.circle_filled(pos, r, QTH_RED);
            painter.circle_stroke(pos, r, Stroke::new(1.0, Color32::BLACK));
            continue;
        }
        let col = colors.heard;
        let r = 3.5;
        painter.circle_filled(pos, r, col);
        painter.circle_stroke(pos, r, Stroke::new(1.0, Color32::BLACK));
        if view.span_lon < 40.0 {
            painter.text(
                pos + Vec2::new(0.0, -(r + 2.0)),
                egui::Align2::CENTER_BOTTOM,
                &m.call,
                font.clone(),
                col,
            );
        }
    }

    // Hover locator
    let mut hover = None;
    if let Some(pos) = resp.hover_pos() {
        let (lon, lat) = view.unproject(pos, rect);
        if let Some(loc) = lat_lon_to_locator(lat, lon) {
            hover = Some(loc);
        }
    }
    hover
}

struct CountryLabel {
    name: String,
    pos: Pos2,
    area: f64,
}

fn ring_lon_span(ring: &[[f64; 2]]) -> f64 {
    let mut min_lon = f64::MAX;
    let mut max_lon = f64::MIN;
    for p in ring {
        min_lon = min_lon.min(p[0]);
        max_lon = max_lon.max(p[0]);
    }
    if min_lon > max_lon {
        0.0
    } else {
        max_lon - min_lon
    }
}

fn ring_crosses_dateline(ring: &[[f64; 2]]) -> bool {
    if ring_lon_span(ring) > 180.0 {
        return true;
    }
    for i in 0..ring.len().saturating_sub(1) {
        if (ring[i + 1][0] - ring[i][0]).abs() > 90.0 {
            return true;
        }
    }
    false
}

fn dedupe_ring_points(ring: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(ring.len());
    for &p in ring {
        if out
            .last()
            .is_some_and(|q| (q[0] - p[0]).abs() < 1e-7 && (q[1] - p[1]).abs() < 1e-7)
        {
            continue;
        }
        out.push(p);
    }
    if out.len() >= 2 {
        let first = out[0];
        let last = *out.last().unwrap();
        if (first[0] - last[0]).abs() > 1e-7 || (first[1] - last[1]).abs() > 1e-7 {
            out.push(first);
        }
    }
    out
}

/// Fill a lon/lat ring with earcut triangles (projected to screen).
/// Drops triangles whose lon/lat centroid falls outside the ring — guards
/// against self-intersecting simplified coasts (e.g. Sudan).
fn paint_ring_filled_earcut(
    painter: &egui::Painter,
    view: &MapView,
    ring: &[[f64; 2]],
    rect: Rect,
    fill: Color32,
    stroke: Stroke,
) {
    // earcut wants an open ring (no repeated closing vertex).
    let body: Vec<[f64; 2]> = if ring.len() >= 2
        && (ring[0][0] - ring[ring.len() - 1][0]).abs() < 1e-7
        && (ring[0][1] - ring[ring.len() - 1][1]).abs() < 1e-7
    {
        ring[..ring.len() - 1].to_vec()
    } else {
        ring.to_vec()
    };
    if body.len() < 3 {
        return;
    }

    let mut flat: Vec<f64> = Vec::with_capacity(body.len() * 2);
    for p in &body {
        flat.push(p[0]);
        flat.push(p[1]);
    }
    let Ok(indices) = earcutr::earcut(&flat, &[], 2) else {
        paint_ring_outline_no_wrap(painter, view, ring, rect, stroke);
        return;
    };

    let mut mesh = Mesh::default();
    mesh.vertices.reserve(body.len());
    mesh.indices.reserve(indices.len());
    for p in &body {
        mesh.colored_vertex(view.project(p[0], p[1], rect), fill);
    }
    for tri in indices.chunks_exact(3) {
        let i0 = tri[0];
        let i1 = tri[1];
        let i2 = tri[2];
        if i0 >= body.len() || i1 >= body.len() || i2 >= body.len() {
            continue;
        }
        let clon = (body[i0][0] + body[i1][0] + body[i2][0]) / 3.0;
        let clat = (body[i0][1] + body[i1][1] + body[i2][1]) / 3.0;
        if !point_in_ring(clon, clat, &body) {
            continue;
        }
        mesh.add_triangle(i0 as u32, i1 as u32, i2 as u32);
    }
    if !mesh.is_empty() {
        painter.add(Shape::mesh(mesh));
    }
    paint_ring_outline_no_wrap(painter, view, ring, rect, stroke);
}

fn point_in_ring(lon: f64, lat: f64, ring: &[[f64; 2]]) -> bool {
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (ring[i][0], ring[i][1]);
        let (xj, yj) = (ring[j][0], ring[j][1]);
        if ((yi > lat) != (yj > lat))
            && (lon < (xj - xi) * (lat - yi) / (yj - yi + 1e-15) + xi)
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Coastline for wrap-around rings. Skips edges that jump across ±180°
/// (those project as a vertical chord through Africa / the ocean).
fn paint_ring_outline_no_wrap(
    painter: &egui::Painter,
    view: &MapView,
    ring: &[[f64; 2]],
    rect: Rect,
    stroke: Stroke,
) {
    let n = ring.len();
    if n < 2 {
        return;
    }
    for i in 0..n.saturating_sub(1) {
        let a = ring[i];
        let b = ring[i + 1];
        if (b[0] - a[0]).abs() > 90.0 {
            continue;
        }
        let pa = view.project(a[0], a[1], rect);
        let pb = view.project(b[0], b[1], rect);
        if (pa.x - pb.x).abs() > rect.width() * 0.45 {
            continue;
        }
        painter.line_segment([pa, pb], stroke);
    }
}

/// Centroid + bbox of the ring with the largest absolute area (mainland first).
fn largest_ring_label_anchor(rings: &[Vec<[f64; 2]>]) -> Option<(f64, f64, f64, f64)> {
    let mut best: Option<(f64, f64, f64, f64, f64)> = None; // area, lon, lat, w, h
    for ring in rings {
        if ring.len() < 3 {
            continue;
        }
        let area = ring_area(ring);
        if best.map(|(a, ..)| area <= a).unwrap_or(false) {
            continue;
        }
        let (lon, lat) = ring_centroid(ring)?;
        let mut min_lon = f64::MAX;
        let mut max_lon = f64::MIN;
        let mut min_lat = f64::MAX;
        let mut max_lat = f64::MIN;
        for p in ring {
            min_lon = min_lon.min(p[0]);
            max_lon = max_lon.max(p[0]);
            min_lat = min_lat.min(p[1]);
            max_lat = max_lat.max(p[1]);
        }
        best = Some((
            area,
            lon,
            lat,
            (max_lon - min_lon).max(0.01),
            (max_lat - min_lat).max(0.01),
        ));
    }
    best.map(|(_, lon, lat, w, h)| (lon, lat, w, h))
}

fn ring_area(ring: &[[f64; 2]]) -> f64 {
    let mut a = 0.0;
    for i in 0..ring.len().saturating_sub(1) {
        a += ring[i][0] * ring[i + 1][1] - ring[i + 1][0] * ring[i][1];
    }
    a.abs() * 0.5
}

fn ring_centroid(ring: &[[f64; 2]]) -> Option<(f64, f64)> {
    // Prefer polygon centroid; fall back to vertex average.
    let n = ring.len().saturating_sub(1); // last point often duplicates first
    if n < 3 {
        return None;
    }
    let mut a = 0.0;
    let mut cx = 0.0;
    let mut cy = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        let cross = ring[i][0] * ring[j][1] - ring[j][0] * ring[i][1];
        a += cross;
        cx += (ring[i][0] + ring[j][0]) * cross;
        cy += (ring[i][1] + ring[j][1]) * cross;
    }
    let a2 = a * 0.5;
    if a2.abs() < 1e-12 {
        let mut sx = 0.0;
        let mut sy = 0.0;
        for p in ring.iter().take(n) {
            sx += p[0];
            sy += p[1];
        }
        return Some((sx / n as f64, sy / n as f64));
    }
    Some((cx / (6.0 * a2), cy / (6.0 * a2)))
}

/// Draw larger countries first; skip a name when its box overlaps one already drawn.
fn paint_country_labels(
    painter: &egui::Painter,
    cands: &mut [CountryLabel],
    font: FontId,
    color: Color32,
) {
    cands.sort_by(|a, b| b.area.partial_cmp(&a.area).unwrap_or(std::cmp::Ordering::Equal));
    let mut placed: Vec<Rect> = Vec::new();
    let pad = 4.0;
    for lab in cands.iter() {
        let galley = painter.layout_no_wrap(lab.name.clone(), font.clone(), color);
        let size = galley.size() + Vec2::splat(pad);
        let box_r = Rect::from_center_size(lab.pos, size);
        if !painter.clip_rect().intersects(box_r) {
            continue;
        }
        if placed.iter().any(|p| p.intersects(box_r.expand(2.0))) {
            continue;
        }
        placed.push(box_r);
        painter.galley(box_r.min + Vec2::splat(pad * 0.5), galley, color);
    }
}

/// Build markers from own QTH + heard stations that have a locator.
pub fn collect_markers(
    own_locator: &str,
    stations: impl Iterator<Item = (String, Option<String>, Option<f64>, Option<f64>)>,
) -> Vec<MapMarker> {
    let mut out = Vec::new();
    if let Some(loc) = normalize_locator(own_locator) {
        if let Some((lat, lon)) = locator_to_lat_lon(&loc) {
            out.push(MapMarker {
                call: "QTH".into(),
                lon,
                lat,
                locator: loc,
                is_qth: true,
            });
        }
    }
    for (call, locator, lat, lon) in stations {
        let Some(loc) = locator.as_deref().and_then(normalize_locator) else {
            continue;
        };
        let (lat, lon) = match (lat, lon) {
            (Some(la), Some(lo)) => (la, lo),
            _ => match locator_to_lat_lon(&loc) {
                Some(p) => p,
                None => continue,
            },
        };
        out.push(MapMarker {
            call,
            lon,
            lat,
            locator: loc,
            is_qth: false,
        });
    }
    out
}
