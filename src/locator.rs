//! Maidenhead locators and payload filters for heard-station positions.
//!
//! Stored locators are **6 or 8 characters** (`JO32KX`, `JO32MW49`). APRS
//! uncompressed, compressed, and **Mic-E** lat/lon are encoded to 8-char.
//! Plain text is scanned for 8-char first, then 6-char. A match equal to the
//! source callsign (base call, SSID stripped) is rejected. When updating
//! MHeard, a longer locator is preferred over a shorter one (keep rare 8-char
//! tags unless APRS / equal-or-longer replaces them).

/// Parsed position from a UI/APRS information field.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionFix {
    /// Uppercase Maidenhead, length 6 or 8.
    pub locator: String,
    pub lat: f64,
    pub lon: f64,
    /// True when lat/lon came from an APRS position report (not a text tag).
    pub from_aprs: bool,
}

/// Strip SSID (`CALL-n`) and uppercase ASCII for callsign comparisons.
pub fn base_call(call: &str) -> String {
    let s = call.trim();
    let base = s.split_once('-').map(|(b, _)| b).unwrap_or(s);
    base.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn scrub(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn valid_maidenhead_prefix6(b: &[u8]) -> bool {
    b.len() >= 6
        && b[0].is_ascii_uppercase()
        && b[0] <= b'R'
        && b[1].is_ascii_uppercase()
        && b[1] <= b'R'
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[4].is_ascii_uppercase()
        && b[4] <= b'X'
        && b[5].is_ascii_uppercase()
        && b[5] <= b'X'
}

/// Validate and normalize a 6-character Maidenhead locator.
pub fn normalize_locator6(raw: &str) -> Option<String> {
    let s = scrub(raw);
    if s.len() != 6 || !valid_maidenhead_prefix6(s.as_bytes()) {
        return None;
    }
    Some(s)
}

/// Validate and normalize a 6- or 8-character Maidenhead locator.
///
/// 8-char form adds two digits (`JO32MW49`).
pub fn normalize_locator(raw: &str) -> Option<String> {
    let s = scrub(raw);
    let b = s.as_bytes();
    match s.len() {
        6 if valid_maidenhead_prefix6(b) => Some(s),
        8 if valid_maidenhead_prefix6(b) && b[6].is_ascii_digit() && b[7].is_ascii_digit() => {
            Some(s)
        }
        _ => None,
    }
}

/// True when `new` should replace `old` in MHeard (more specific, equal length, or APRS).
pub fn should_replace_locator(old: Option<&str>, new: &PositionFix) -> bool {
    match old {
        None => true,
        Some(_) if new.from_aprs => true,
        Some(o) => new.locator.len() >= o.len(),
    }
}

/// Centre of a 6- or 8-character Maidenhead cell (degrees).
pub fn locator_to_lat_lon(locator: &str) -> Option<(f64, f64)> {
    let loc = normalize_locator(locator)?;
    let b = loc.as_bytes();
    let mut lon = -180.0
        + (b[0] - b'A') as f64 * 20.0
        + (b[2] - b'0') as f64 * 2.0
        + (b[4] - b'A') as f64 * (2.0 / 24.0);
    let mut lat = -90.0
        + (b[1] - b'A') as f64 * 10.0
        + (b[3] - b'0') as f64 * 1.0
        + (b[5] - b'A') as f64 * (1.0 / 24.0);
    let (dlon, dlat) = if loc.len() == 8 {
        let ext_lon = (b[6] - b'0') as f64;
        let ext_lat = (b[7] - b'0') as f64;
        // Each extended step is 1/10 of the subsquare.
        let step_lon = (2.0 / 24.0) / 10.0;
        let step_lat = (1.0 / 24.0) / 10.0;
        lon += ext_lon * step_lon;
        lat += ext_lat * step_lat;
        (step_lon / 2.0, step_lat / 2.0)
    } else {
        ((2.0 / 24.0) / 2.0, (1.0 / 24.0) / 2.0)
    };
    Some((lat + dlat, lon + dlon))
}

/// Encode lat/lon to a 6-character Maidenhead locator.
pub fn lat_lon_to_locator(lat: f64, lon: f64) -> Option<String> {
    lat_lon_to_locator_n(lat, lon, 6)
}

/// Encode lat/lon to an 8-character Maidenhead locator.
pub fn lat_lon_to_locator8(lat: f64, lon: f64) -> Option<String> {
    lat_lon_to_locator_n(lat, lon, 8)
}

fn lat_lon_to_locator_n(lat: f64, lon: f64, len: usize) -> Option<String> {
    if lat <= -90.0 || lat >= 90.0 || lon < -180.0 || lon > 180.0 {
        return None;
    }
    let mut lon = lon;
    if lon >= 180.0 {
        lon = 179.999999;
    }
    let adj_lon = lon + 180.0;
    let adj_lat = lat + 90.0;
    let field_lon = (adj_lon / 20.0).floor() as u8;
    let field_lat = (adj_lat / 10.0).floor() as u8;
    let square_lon = ((adj_lon % 20.0) / 2.0).floor() as u8;
    let square_lat = (adj_lat % 10.0).floor() as u8;
    let rem_lon = (adj_lon % 20.0) % 2.0;
    let rem_lat = adj_lat % 10.0 % 1.0;
    let sub_lon = (rem_lon / (2.0 / 24.0)).floor() as u8;
    let sub_lat = (rem_lat / (1.0 / 24.0)).floor() as u8;
    if field_lon > 17
        || field_lat > 17
        || square_lon > 9
        || square_lat > 9
        || sub_lon > 23
        || sub_lat > 23
    {
        return None;
    }
    let mut out = String::with_capacity(len);
    out.push((b'A' + field_lon) as char);
    out.push((b'A' + field_lat) as char);
    out.push((b'0' + square_lon) as char);
    out.push((b'0' + square_lat) as char);
    out.push((b'A' + sub_lon.min(23)) as char);
    out.push((b'A' + sub_lat.min(23)) as char);
    if len >= 8 {
        let step_lon = (2.0 / 24.0) / 10.0;
        let step_lat = (1.0 / 24.0) / 10.0;
        let rem2_lon = rem_lon - sub_lon as f64 * (2.0 / 24.0);
        let rem2_lat = rem_lat - sub_lat as f64 * (1.0 / 24.0);
        let ext_lon = (rem2_lon / step_lon).floor().clamp(0.0, 9.0) as u8;
        let ext_lat = (rem2_lat / step_lat).floor().clamp(0.0, 9.0) as u8;
        out.push((b'0' + ext_lon) as char);
        out.push((b'0' + ext_lat) as char);
    }
    Some(out)
}

/// Extract the best position from an APRS/UI information field.
///
/// `info_raw` is the unmodified AX.25 information bytes (required for Mic-E).
/// `dest_call` is the AX.25 destination (Mic-E encodes latitude there).
///
/// Priority: Mic-E → APRS uncompressed/compressed → Maidenhead text tag.
pub fn extract_position(
    info_raw: &[u8],
    src_call: &str,
    dest_call: &str,
) -> Option<PositionFix> {
    if info_raw.is_empty() {
        return None;
    }
    if let Some(fix) = parse_mice(dest_call, info_raw) {
        return Some(fix);
    }
    // Text parsers: lossy UTF-8 is fine for uncompressed / Maidenhead ASCII.
    let info = String::from_utf8_lossy(info_raw);
    if let Some(fix) = parse_aprs_position(&info) {
        return Some(fix);
    }
    extract_maidenhead_tag(&info, src_call)
}

/// Mic-E current/old position DTI bytes (`'`, `` ` ``, 0x1c, 0x1d).
fn mice_dti(b: u8) -> bool {
    matches!(b, b'\'' | b'`' | 0x1c | 0x1d)
}

/// Decode Mic-E position (APRS 101 ch. 10). Needs dest call + info bytes.
fn parse_mice(dest_call: &str, info: &[u8]) -> Option<PositionFix> {
    if info.len() < 9 || !mice_dti(info[0]) {
        return None;
    }
    let dest = mice_dest6(dest_call)?;
    let body = &info[1..]; // longitude… after DTI
    if body.len() < 8 {
        return None;
    }

    // Latitude digits + flags from destination characters.
    let mut digits = [0u8; 6];
    let mut spaces = 0usize;
    for (i, &ch) in dest.iter().enumerate() {
        let (digit, space) = mice_dest_digit(ch, i)?;
        digits[i] = digit;
        if space {
            spaces += 1;
        } else if spaces > 0 {
            // Spaces (ambiguity) may only trail.
            return None;
        }
    }
    // Centre ambiguous trailing digits (aprslib convention).
    if spaces > 0 {
        if spaces >= 4 {
            digits[2] = 3;
        } else {
            digits[6 - spaces] = 5;
        }
    }

    let lat_deg = (digits[0] * 10 + digits[1]) as f64;
    let lat_min = (digits[2] * 10 + digits[3]) as f64
        + (digits[4] * 10 + digits[5]) as f64 / 100.0;
    let mut lat = lat_deg + lat_min / 60.0;
    // Char 4: 0-9/L = South, P-Y/Z = North.
    if dest[3] <= b'L' {
        lat = -lat;
    }

    // Longitude from info bytes 2–4 (body[0..3]), offset/EW from dest 5–6.
    let mut d = body[0] as i32 - 28;
    if dest[4] >= b'P' {
        d += 100;
    }
    if (180..=189).contains(&d) {
        d -= 80;
    }
    if (190..=199).contains(&d) {
        d -= 190;
    }
    let mut m = (body[1] as i32 - 28) as f64;
    if m >= 60.0 {
        m -= 60.0;
    }
    let h = (body[2] as i32 - 28) as f64;
    // Ambiguity centring on longitude minutes.
    let m = match spaces {
        4 => 30.0,
        3 => (m / 10.0).floor() * 10.0 + 5.0,
        2 => m.floor() + 0.5,
        1 => (m * 10.0).floor() / 10.0 + 0.05,
        _ => m + h / 100.0,
    };
    let mut lon = d as f64 + m / 60.0;
    // Char 6: P-Z = West.
    if dest[5] >= b'P' {
        lon = -lon;
    }

    if !(-90.0..90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    let locator = lat_lon_to_locator8(lat, lon)?;
    Some(PositionFix {
        locator,
        lat,
        lon,
        from_aprs: true,
    })
}

fn mice_dest6(dest_call: &str) -> Option<[u8; 6]> {
    let base = dest_call
        .split_once('-')
        .map(|(b, _)| b)
        .unwrap_or(dest_call)
        .trim();
    let b = base.as_bytes();
    if b.len() < 6 {
        return None;
    }
    let mut out = [0u8; 6];
    out.copy_from_slice(&b[..6]);
    for c in &mut out {
        *c = c.to_ascii_uppercase();
    }
    Some(out)
}

/// One Mic-E destination character → latitude digit (0–9) and whether it is a space.
fn mice_dest_digit(ch: u8, index: usize) -> Option<(u8, bool)> {
    match ch {
        b'0'..=b'9' => Some((ch - b'0', false)),
        b'A'..=b'J' if index < 3 => Some((ch - b'A', false)),
        b'K' if index < 3 => Some((0, true)),
        b'L' => Some((0, true)),
        b'P'..=b'Y' => Some((ch - b'P', false)),
        b'Z' => Some((0, true)),
        _ => None,
    }
}

fn extract_maidenhead_tag(info: &str, src_call: &str) -> Option<PositionFix> {
    let reject = base_call(src_call);
    let bytes = info.as_bytes();
    // Prefer 8-char matches when present.
    for len in [8usize, 6] {
        let mut i = 0;
        while i + len <= bytes.len() {
            if let Some(loc) = try_locator_at(bytes, i, len) {
                if loc != reject {
                    if let Some((lat, lon)) = locator_to_lat_lon(&loc) {
                        return Some(PositionFix {
                            locator: loc,
                            lat,
                            lon,
                            from_aprs: false,
                        });
                    }
                }
                i += len;
                continue;
            }
            i += 1;
        }
    }
    None
}

fn try_locator_at(bytes: &[u8], i: usize, len: usize) -> Option<String> {
    if i + len > bytes.len() {
        return None;
    }
    if i > 0 && bytes[i - 1].is_ascii_alphanumeric() {
        return None;
    }
    if i + len < bytes.len() && bytes[i + len].is_ascii_alphanumeric() {
        return None;
    }
    let slice = std::str::from_utf8(&bytes[i..i + len]).ok()?;
    normalize_locator(slice)
}

/// Parse APRS uncompressed and compressed positions from an info field.
fn parse_aprs_position(info: &str) -> Option<PositionFix> {
    let bytes = info.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let first = bytes[0] as char;
    let rest = if first == '/' || first == '@' {
        skip_aprs_timestamp(&info[1..])?
    } else if first == '!' || first == '=' {
        &info[1..]
    } else {
        if let Some(fix) = scan_uncompressed_anywhere(info) {
            return Some(fix);
        }
        return None;
    };

    if let Some(fix) = parse_uncompressed_body(rest) {
        return Some(fix);
    }
    if let Some(fix) = parse_compressed_body(rest) {
        return Some(fix);
    }
    scan_uncompressed_anywhere(info)
}

fn skip_aprs_timestamp(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    if b.len() >= 7 {
        let t = &b[..7];
        let digits_ok = t[..6].iter().all(|c| c.is_ascii_digit());
        let kind = t[6];
        if digits_ok && (kind == b'z' || kind == b'/' || kind == b'h') {
            return Some(&s[7..]);
        }
    }
    Some(s)
}

fn scan_uncompressed_anywhere(info: &str) -> Option<PositionFix> {
    let bytes = info.as_bytes();
    let mut i = 0;
    while i + 19 <= bytes.len() {
        if let Some(fix) = parse_uncompressed_body(std::str::from_utf8(&bytes[i..]).ok()?) {
            return Some(fix);
        }
        i += 1;
    }
    None
}

/// `DDMM.mmN/DDDMM.mmW` (19+ chars with symbol table).
fn parse_uncompressed_body(s: &str) -> Option<PositionFix> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    if !(b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[4] == b'.'
        && b[5].is_ascii_digit()
        && b[6].is_ascii_digit())
    {
        return None;
    }
    let ns = (b[7] as char).to_ascii_uppercase();
    if ns != 'N' && ns != 'S' {
        return None;
    }
    let lon_off = 9;
    if !(b[lon_off].is_ascii_digit()
        && b[lon_off + 1].is_ascii_digit()
        && b[lon_off + 2].is_ascii_digit()
        && b[lon_off + 3].is_ascii_digit()
        && b[lon_off + 4].is_ascii_digit()
        && b[lon_off + 5] == b'.'
        && b[lon_off + 6].is_ascii_digit()
        && b[lon_off + 7].is_ascii_digit())
    {
        return None;
    }
    let ew = (b[lon_off + 8] as char).to_ascii_uppercase();
    if ew != 'E' && ew != 'W' {
        return None;
    }

    let lat_deg: f64 = std::str::from_utf8(&b[0..2]).ok()?.parse().ok()?;
    let lat_min: f64 = std::str::from_utf8(&b[2..7]).ok()?.parse().ok()?;
    let mut lat = lat_deg + lat_min / 60.0;
    if ns == 'S' {
        lat = -lat;
    }

    let lon_deg: f64 = std::str::from_utf8(&b[lon_off..lon_off + 3]).ok()?.parse().ok()?;
    let lon_min: f64 = std::str::from_utf8(&b[lon_off + 3..lon_off + 8])
        .ok()?
        .parse()
        .ok()?;
    let mut lon = lon_deg + lon_min / 60.0;
    if ew == 'W' {
        lon = -lon;
    }

    // APRS has minute precision — store 8-char.
    let locator = lat_lon_to_locator8(lat, lon)?;
    Some(PositionFix {
        locator,
        lat,
        lon,
        from_aprs: true,
    })
}

fn parse_compressed_body(s: &str) -> Option<PositionFix> {
    let b = s.as_bytes();
    if b.len() < 13 {
        return None;
    }
    let table = b[0];
    if table != b'/' && table != b'\\' {
        return None;
    }
    for &c in &b[1..9] {
        if !(b'!'..=b'z').contains(&c) {
            return None;
        }
    }
    let y1 = b[1] as i32 - 33;
    let y2 = b[2] as i32 - 33;
    let y3 = b[3] as i32 - 33;
    let y4 = b[4] as i32 - 33;
    let x1 = b[5] as i32 - 33;
    let x2 = b[6] as i32 - 33;
    let x3 = b[7] as i32 - 33;
    let x4 = b[8] as i32 - 33;
    let lat_c = y1 * 91i32.pow(3) + y2 * 91i32.pow(2) + y3 * 91 + y4;
    let lon_c = x1 * 91i32.pow(3) + x2 * 91i32.pow(2) + x3 * 91 + x4;
    let lat = 90.0 - (lat_c as f64) / 380926.0;
    let lon = (lon_c as f64) / 190463.0 - 180.0;
    let locator = lat_lon_to_locator8(lat, lon)?;
    Some(PositionFix {
        locator,
        lat,
        lon,
        from_aprs: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_jo32kx_area() {
        let (lat, lon) = locator_to_lat_lon("JO32KX").unwrap();
        let back = lat_lon_to_locator(lat, lon).unwrap();
        assert_eq!(back, "JO32KX");
    }

    #[test]
    fn roundtrip_eight_char() {
        let (lat, lon) = locator_to_lat_lon("JO32MW49").unwrap();
        let back = lat_lon_to_locator8(lat, lon).unwrap();
        assert_eq!(back, "JO32MW49");
    }

    #[test]
    fn normalize_accepts_six_and_eight() {
        assert!(normalize_locator("JO32").is_none());
        assert_eq!(normalize_locator("jo32kx").as_deref(), Some("JO32KX"));
        assert_eq!(normalize_locator("JO32mw49").as_deref(), Some("JO32MW49"));
        assert!(normalize_locator6("JO32MW49").is_none());
    }

    fn pos(info: &str, src: &str) -> Option<PositionFix> {
        extract_position(info.as_bytes(), src, "APRS")
    }

    #[test]
    fn rejects_callsign_looking_locator() {
        let info = "hello PI12NN world";
        assert!(pos(info, "PI12NN").is_none());
        assert!(pos(info, "PI12NN-7").is_none());
        let fix = pos(info, "PA0ABC").unwrap();
        assert_eq!(fix.locator, "PI12NN");
        assert!(!fix.from_aprs);
    }

    #[test]
    fn finds_eight_char_tag() {
        let fix = pos("QTH JO32mw49 73", "N0CALL").unwrap();
        assert_eq!(fix.locator, "JO32MW49");
        assert_eq!(fix.locator.len(), 8);
    }

    #[test]
    fn prefers_eight_over_six_in_same_payload() {
        let fix = pos("near JO32KX and JO32KX12", "N0CALL").unwrap();
        assert_eq!(fix.locator, "JO32KX12");
    }

    #[test]
    fn finds_jo32kx_in_comment() {
        let fix = pos("QTH JO32KX 73", "N0CALL").unwrap();
        assert_eq!(fix.locator, "JO32KX");
    }

    #[test]
    fn aprs_uncompressed_bang() {
        let info = "!5231.00N/01324.00E-test";
        let fix = pos(info, "DB0ABC").unwrap();
        assert!(fix.from_aprs);
        assert_eq!(fix.locator.len(), 8);
        assert_eq!(&fix.locator[..4], "JO62");
        assert!((fix.lat - 52.51666).abs() < 0.01);
        assert!((fix.lon - 13.4).abs() < 0.01);
    }

    #[test]
    fn aprs_wins_over_text_tag() {
        let info = "!5231.00N/01324.00E- also JO32KX";
        let fix = pos(info, "N0CALL").unwrap();
        assert!(fix.from_aprs);
        assert_ne!(&fix.locator[..6], "JO32KX");
    }

    /// Spec-style Mic-E: dest `S32UVT`, info `` `(_fn"Oj/` `` → Phoenix area.
    #[test]
    fn mice_phoenix_example() {
        let info = b"`(_fn\"Oj/";
        let fix = extract_position(info, "N7YV-9", "S32UVT").unwrap();
        assert!(fix.from_aprs);
        assert_eq!(fix.locator.len(), 8);
        assert!((fix.lat - 33.427333).abs() < 0.001);
        assert!((fix.lon - (-112.129)).abs() < 0.001);
    }

    #[test]
    fn mice_needs_dest_and_raw() {
        // Same bytes with a non-Mic-E dest must not invent a position.
        assert!(extract_position(b"`(_fn\"Oj/", "N7YV-9", "APRS").is_none());
    }

    #[test]
    fn replace_keeps_more_specific() {
        let short = PositionFix {
            locator: "JO32KX".into(),
            lat: 0.0,
            lon: 0.0,
            from_aprs: false,
        };
        let long = PositionFix {
            locator: "JO32KX12".into(),
            lat: 0.0,
            lon: 0.0,
            from_aprs: false,
        };
        assert!(!should_replace_locator(Some("JO32KX12"), &short));
        assert!(should_replace_locator(Some("JO32KX"), &long));
        let aprs = PositionFix {
            locator: "JO62AA00".into(),
            lat: 52.0,
            lon: 13.0,
            from_aprs: true,
        };
        assert!(should_replace_locator(Some("JO32KX12"), &aprs));
    }

    #[test]
    fn base_call_strips_ssid() {
        assert_eq!(base_call("pi12nn-9"), "PI12NN");
        assert_eq!(base_call("N0CALL"), "N0CALL");
    }
}
