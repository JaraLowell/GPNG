//! `NAMES.GP` — station names and the DOS autorouter.
//!
//! Line shape from Graphic Packet 1.61, chapter 10:
//!
//! ```text
//! TYPE>CALL NAME; PATH
//! TYPE>IDENT:CALL NAME; PATH
//! ```
//!
//! `TYPE` is `B` mailbox, `D` L2 digi, `F` FlexNet, `N` NetRom, `!` (`C!`),
//! or `T` terminal. A path hop is the same `TYPE>CALL`. A hop that names
//! another station is replaced by that station's path. `D>0` is an SP-gateway
//! port, not a digipeater. `<IF qrg>` … `<END>` keeps a block only when `qrg`
//! equals the session QRG string (`430.625`, not the comma form). `PORTx`
//! supplies `%DIGI%`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::normalize_mycall;
use crate::ui::editor::{decode_cp437, encode_cp437};

const NAME_CHARS: usize = 22;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StationKind {
    Mailbox,
    Digi,
    Flex,
    NetRom,
    /// Later DigiWare (`W>`). Same character-index password as TheNet.
    Ware,
    Bang,
    Terminal,
}

impl StationKind {
    fn from_char(c: char) -> Option<Self> {
        match c.to_ascii_uppercase() {
            'B' => Some(Self::Mailbox),
            'D' => Some(Self::Digi),
            'F' => Some(Self::Flex),
            'N' => Some(Self::NetRom),
            'W' => Some(Self::Ware),
            '!' => Some(Self::Bang),
            'T' => Some(Self::Terminal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Hop {
    kind: StationKind,
    token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Station {
    kind: StationKind,
    call: String,
    ident: Option<String>,
    name: String,
    hops: Vec<Hop>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Port {
    digi: String,
    qrg: String,
}

/// One connect. `link_call` is the AX.25 / TNC target. `downlinks` are the
/// `C` / `C!` lines sent after that link is up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub call: String,
    pub name: String,
    pub link_call: String,
    pub link_digis: Vec<String>,
    pub downlinks: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct NamesFile {
    path: PathBuf,
    lines: Vec<String>,
    newline: String,
}

impl NamesFile {
    pub fn load_default() -> Self {
        Self::load_from(crate::paths::data_file("NAMES.GP"))
    }

    pub fn load_from(path: PathBuf) -> Self {
        let raw = fs::read(&path).unwrap_or_default();
        let text = if raw.is_empty() {
            String::new()
        } else {
            decode_cp437(&raw)
        };
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let lines = split_lines(&text);
        Self {
            path,
            lines,
            newline: newline.to_string(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Name stored for `call` at this QRG (`430.625`). Empty QRG still sees
    /// lines that sit outside `<IF>`.
    pub fn name_for(&self, call: &str, qrg: &str) -> Option<String> {
        let call = normalize_mycall(call).ok()?;
        self.active(qrg)
            .into_iter()
            .find(|s| s.call.eq_ignore_ascii_case(&call))
            .map(|s| s.name)
            .filter(|n| !n.is_empty())
    }

    /// `NAMES.GP` type for this call on the current QRG, if the station is listed.
    pub fn kind_of(&self, call: &str, qrg: &str) -> Option<StationKind> {
        let call = normalize_mycall(call).ok()?;
        self.active(qrg)
            .into_iter()
            .find(|s| s.call.eq_ignore_ascii_case(&call))
            .map(|s| s.kind)
    }

    /// Call, ident, or name → route. `Ok(None)` means the token is not in the file.
    pub fn route_for(&self, query: &str, qrg: &str) -> Result<Option<Route>, String> {
        let Some(station) = self.find_query(query, qrg) else {
            return Ok(None);
        };
        let hops = self.expand_station(&station.call, qrg, &mut Vec::new())?;
        Ok(Some(plan(&station, &hops)))
    }

    /// ALT-N / `//Name`. Updates the first line with this call, or appends
    /// `T>CALL NAME;`. The name is the first 22 characters, as in DOS GP.
    pub fn set_name(&mut self, call: &str, name: &str) -> Result<String, String> {
        let call = normalize_mycall(call)?;
        let name = clean_name(name)?;
        if let Some(idx) = self.line_for_call(&call) {
            self.lines[idx] = splice_name(&self.lines[idx], &name);
        } else {
            if self.lines.iter().all(|l| l.trim().is_empty()) {
                self.lines.clear();
                self.lines
                    .push("*** Rufzeichenliste für Graphic-Packet ***".into());
                self.lines.push(String::new());
            } else if self.lines.last().is_some_and(|l| !l.is_empty()) {
                self.lines.push(String::new());
            }
            self.lines.push(format!("T>{call} {name};"));
        }
        self.save()?;
        Ok(name)
    }

    fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let text = self.lines.join(&self.newline);
        let bytes = encode_cp437(&text);
        fs::write(&self.path, bytes).map_err(|e| format!("NAMES.GP: {e}"))
    }

    fn line_for_call(&self, call: &str) -> Option<usize> {
        self.lines
            .iter()
            .position(|line| parse_station(line).is_some_and(|s| s.call.eq_ignore_ascii_case(call)))
    }

    fn active(&self, qrg: &str) -> Vec<Station> {
        let mut out = Vec::new();
        let mut include = true;
        let mut in_if = false;
        for line in &self.lines {
            let trimmed = line.trim();
            if let Some(key) = if_key(trimmed) {
                in_if = true;
                include = key == qrg.trim();
                continue;
            }
            if is_tag(trimmed, "ELSE") && in_if {
                include = !include;
                continue;
            }
            if is_tag(trimmed, "END") {
                in_if = false;
                include = true;
                continue;
            }
            if !include {
                continue;
            }
            if let Some(station) = parse_station(trimmed) {
                out.push(station);
            }
        }
        out
    }

    fn ports(&self) -> Vec<Port> {
        self.lines
            .iter()
            .filter_map(|l| parse_port(l.trim()))
            .collect()
    }

    fn find_query(&self, query: &str, qrg: &str) -> Option<Station> {
        let query = query.trim();
        if query.is_empty() {
            return None;
        }
        let stations = self.active(qrg);
        if let Ok(call) = normalize_mycall(query) {
            if let Some(st) = stations.iter().find(|s| s.call.eq_ignore_ascii_case(&call)) {
                return Some(st.clone());
            }
        }
        if let Some(st) = stations.iter().find(|s| {
            s.ident
                .as_deref()
                .is_some_and(|id| id.eq_ignore_ascii_case(query))
        }) {
            return Some(st.clone());
        }
        let want = clip_name(query);
        stations
            .into_iter()
            .find(|s| clip_name(&s.name).eq_ignore_ascii_case(&want) && !s.name.is_empty())
    }

    fn by_call<'a>(&self, stations: &'a [Station], call: &str) -> Option<&'a Station> {
        stations.iter().find(|s| s.call.eq_ignore_ascii_case(call))
    }

    fn by_ident<'a>(&self, stations: &'a [Station], token: &str) -> Option<&'a Station> {
        stations.iter().find(|s| {
            s.ident
                .as_deref()
                .is_some_and(|id| id.eq_ignore_ascii_case(token))
        })
    }

    fn expand_station(
        &self,
        call: &str,
        qrg: &str,
        stack: &mut Vec<String>,
    ) -> Result<Vec<Hop>, String> {
        let stations = self.active(qrg);
        let Some(st) = self.by_call(&stations, call) else {
            return Ok(Vec::new());
        };
        if st.hops.is_empty() {
            return Ok(vec![Hop {
                kind: st.kind,
                token: st.call.clone(),
            }]);
        }
        if stack.iter().any(|c| c.eq_ignore_ascii_case(call)) {
            return Ok(vec![Hop {
                kind: st.kind,
                token: st.call.clone(),
            }]);
        }
        stack.push(call.to_string());
        let mut out = Vec::new();
        for hop in &st.hops {
            let token = self.subst_token(&hop.token, qrg, &stations)?;
            if token.eq_ignore_ascii_case(call) {
                out.push(Hop {
                    kind: hop.kind,
                    token,
                });
                continue;
            }
            let known = self.by_call(&stations, &token);
            if known.is_some_and(|s| !s.hops.is_empty())
                && !stack.iter().any(|c| c.eq_ignore_ascii_case(&token))
            {
                out.extend(self.expand_station(&token, qrg, stack)?);
            } else {
                out.push(Hop {
                    kind: hop.kind,
                    token,
                });
            }
        }
        stack.pop();
        Ok(out)
    }

    fn subst_token(&self, token: &str, qrg: &str, stations: &[Station]) -> Result<String, String> {
        if token.eq_ignore_ascii_case("%DIGI%") {
            return self
                .ports()
                .into_iter()
                .find(|p| p.qrg == qrg.trim())
                .map(|p| p.digi)
                .ok_or_else(|| format!("NAMES.GP: no PORT matches QRG {qrg}"));
        }
        if let Some(st) = self.by_ident(stations, token) {
            return Ok(st.call.clone());
        }
        if let Ok(call) = normalize_mycall(token) {
            return Ok(call);
        }
        Ok(token.to_ascii_uppercase())
    }
}

fn plan(station: &Station, hops: &[Hop]) -> Route {
    let dest = station.call.clone();
    let mut digis = Vec::new();
    let mut port: Option<String> = None;
    let mut steps: Vec<(bool, String, Vec<String>)> = Vec::new();
    for hop in hops {
        if hop.kind == StationKind::Digi && hop.token.chars().all(|c| c.is_ascii_digit()) {
            port = Some(hop.token.clone());
            continue;
        }
        if hop.kind == StationKind::Digi && !hop.token.eq_ignore_ascii_case(&dest) {
            digis.push(hop.token.clone());
            continue;
        }
        let call = apply_port(&hop.token, port.take());
        let bang = hop.kind == StationKind::Bang;
        steps.push((bang, call, std::mem::take(&mut digis)));
    }
    if steps.is_empty() {
        steps.push((false, apply_port(&dest, port), digis));
    } else if !digis.is_empty() {
        if steps
            .last()
            .is_some_and(|(_, call, _)| same_station(call, &dest))
        {
            steps.last_mut().unwrap().2.extend(digis);
        } else {
            steps.push((false, dest.clone(), digis));
        }
    } else if steps
        .last()
        .is_some_and(|(_, call, _)| !same_station(call, &dest))
    {
        steps.push((false, dest.clone(), Vec::new()));
    }
    let (link_bang_ignored, link_call, link_digis) = steps.remove(0);
    let _ = link_bang_ignored;
    let downlinks = steps
        .into_iter()
        .map(|(bang, call, digis)| {
            let verb = if bang { "C!" } else { "C" };
            if digis.is_empty() {
                format!("{verb} {call}")
            } else {
                format!("{verb} {call} {}", digis.join(" "))
            }
        })
        .collect();
    Route {
        call: dest,
        name: station.name.clone(),
        link_call,
        link_digis,
        downlinks,
    }
}

/// `DB0XYZ-0` is the same station as `DB0XYZ` after an SP-gateway port.
fn same_station(call: &str, dest: &str) -> bool {
    if call.eq_ignore_ascii_case(dest) {
        return true;
    }
    let Some((base, ssid)) = call.split_once('-') else {
        return false;
    };
    base.eq_ignore_ascii_case(dest) && !ssid.is_empty() && ssid.chars().all(|c| c.is_ascii_digit())
}

fn apply_port(call: &str, port: Option<String>) -> String {
    let Some(port) = port else {
        return call.to_string();
    };
    let base = call.split_once('-').map(|(b, _)| b).unwrap_or(call);
    format!("{base}-{port}")
}

fn parse_station(line: &str) -> Option<Station> {
    let line = line.trim();
    if line.is_empty() || line.starts_with("***") || line.starts_with("PORT") {
        return None;
    }
    let (kind, rest) = split_type(line)?;
    let (head, path) = match rest.split_once(';') {
        Some((h, p)) => (h.trim(), p.trim()),
        None => (rest.trim(), ""),
    };
    let mut parts = head.split_whitespace();
    let token = parts.next()?;
    let name = parts.collect::<Vec<_>>().join(" ");
    let (ident, call) = split_ident_call(token)?;
    let mut hops = Vec::new();
    if !path.is_empty() {
        for tok in path.split_whitespace() {
            if let Some((hk, hr)) = split_type(tok) {
                if hr.is_empty() {
                    return None;
                }
                hops.push(Hop {
                    kind: hk,
                    token: hr.to_string(),
                });
                continue;
            }
            // Bare digis after `;` — `NL0PEL,NL1WIM` or `NL0PEL NL1WIM`.
            for piece in tok.split(',') {
                let piece = piece.trim();
                if piece.is_empty() {
                    continue;
                }
                let call = normalize_mycall(piece).ok()?;
                hops.push(Hop {
                    kind: StationKind::Digi,
                    token: call,
                });
            }
        }
    }
    Some(Station {
        kind,
        call,
        ident,
        name,
        hops,
    })
}

fn split_type(text: &str) -> Option<(StationKind, &str)> {
    let mut chars = text.chars();
    let kind = StationKind::from_char(chars.next()?)?;
    let rest = chars.as_str();
    let rest = rest.strip_prefix('>')?;
    Some((kind, rest))
}

fn split_ident_call(token: &str) -> Option<(Option<String>, String)> {
    if let Some((ident, call)) = token.split_once(':') {
        if ident.is_empty() {
            return None;
        }
        let call = normalize_mycall(call).ok()?;
        return Some((Some(ident.to_ascii_uppercase()), call));
    }
    let call = normalize_mycall(token).ok()?;
    Some((None, call))
}

fn parse_port(line: &str) -> Option<Port> {
    if line.len() < 5 || !line[..4].eq_ignore_ascii_case("PORT") {
        return None;
    }
    let rest = line[4..].trim_start();
    let (num, rest) = rest.split_once('=')?;
    if !num.trim().chars().all(|c| c.is_ascii_digit()) || num.trim().is_empty() {
        return None;
    }
    let rest = rest.trim();
    let (digi, qrg) = rest.split_once(',')?;
    let digi = normalize_mycall(digi.trim()).ok()?;
    let qrg = qrg.trim().to_string();
    if qrg.is_empty() {
        return None;
    }
    Some(Port { digi, qrg })
}

fn if_key(line: &str) -> Option<&str> {
    let inner = line.strip_prefix('<')?.strip_suffix('>')?.trim();
    let rest = inner
        .get(..2)
        .filter(|w| w.eq_ignore_ascii_case("IF"))
        .map(|_| inner[2..].trim())?;
    if rest.is_empty() {
        None
    } else {
        Some(rest)
    }
}

fn is_tag(line: &str, tag: &str) -> bool {
    let Some(inner) = line.strip_prefix('<').and_then(|s| s.strip_suffix('>')) else {
        return false;
    };
    inner.trim().eq_ignore_ascii_case(tag)
}

fn clean_name(raw: &str) -> Result<String, String> {
    let name = clip_name(raw);
    if name.is_empty() || name.contains(';') || name.contains('>') {
        return Err("NAMES.GP: give a name".into());
    }
    Ok(name)
}

fn clip_name(raw: &str) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .chars()
        .take(NAME_CHARS)
        .collect::<String>()
        .trim_end()
        .to_string()
}

fn splice_name(line: &str, name: &str) -> String {
    let Some(gt) = line.find('>') else {
        return line.to_string();
    };
    let after = gt + 1;
    let rest = &line[after..];
    let token_len = rest
        .find(|c: char| c.is_whitespace() || c == ';')
        .unwrap_or(rest.len());
    let token_end = after + token_len;
    let tail = &line[token_end..];
    if tail.starts_with(';') || tail.is_empty() {
        return format!("{} {name}{tail}", &line[..token_end]);
    }
    let mut i = token_end;
    while i < line.len() {
        let ch = line[i..].chars().next().unwrap();
        if ch == '\n' || ch == '\r' || !ch.is_whitespace() {
            break;
        }
        i += ch.len_utf8();
    }
    let end = line[i..]
        .find([';', '\r', '\n'])
        .map(|p| i + p)
        .unwrap_or(line.len());
    format!("{}{name}{}", &line[..i], &line[end..])
}

fn split_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n')
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(text: &str) -> NamesFile {
        NamesFile {
            path: PathBuf::from("NAMES.GP"),
            lines: split_lines(text),
            newline: "\n".into(),
        }
    }

    #[test]
    fn user_file_names() {
        let n = book(
            "*** Rufzeichenliste für Graphic-Packet ***\n\
             \n\
             T>NL0MSK Jara;\n\
             T>NL1AS  Bas;\n\
             T>NL1WNB Wim;\n\
             T>NL0PEL Tale;\n",
        );
        assert_eq!(n.name_for("nl0msk", "").as_deref(), Some("Jara"));
        let route = n.route_for("Jara", "").unwrap().unwrap();
        assert_eq!(route.link_call, "NL0MSK");
        assert!(route.link_digis.is_empty());
        assert!(route.downlinks.is_empty());
        assert_eq!(n.route_for("Tale", "").unwrap().unwrap().call, "NL0PEL");
    }

    #[test]
    fn digipeater_path_is_one_connect() {
        let n = book("T>DH1DAE Ulf; D>DB0NWS D>DB0FN T>DH1DAE\n");
        let route = n.route_for("DH1DAE", "").unwrap().unwrap();
        assert_eq!(route.link_call, "DH1DAE");
        assert_eq!(route.link_digis, vec!["DB0NWS", "DB0FN"]);
        assert!(route.downlinks.is_empty());
        assert_eq!(route.name, "Ulf");
    }

    #[test]
    fn bare_comma_digis_after_semicolon() {
        // Classic short form: name, then digis after `;` (no D> prefix).
        let n = book("T>NL1AS  Bas; NL0PEL,NL1WIM\n");
        let route = n.route_for("NL1AS", "").unwrap().unwrap();
        assert_eq!(route.name, "Bas");
        assert_eq!(route.link_call, "NL1AS");
        assert_eq!(route.link_digis, vec!["NL0PEL", "NL1WIM"]);
    }

    #[test]
    fn node_path_sends_downlinks() {
        let n = book("T>DH1DAE Ulf; N>DB0HSK F>DB0DOZ D>DB0FN T>DH1DAE\n");
        let route = n.route_for("ULF", "").unwrap();
        // "ULF" is the name, not an ident.
        let route = route.unwrap();
        assert_eq!(route.link_call, "DB0HSK");
        assert!(route.link_digis.is_empty());
        assert_eq!(
            route.downlinks,
            vec!["C DB0DOZ".to_string(), "C DH1DAE DB0FN".to_string()]
        );
    }

    #[test]
    fn bang_path_uses_c_bang_after_the_first_link() {
        let n = book("T>DF3VI Patrick; D>DB0FN-9 !>DB0II !>ON5PL !>ON5ZS F>LX0PAC D>DB0HOM\n");
        let route = n.route_for("DF3VI", "").unwrap().unwrap();
        assert_eq!(route.link_call, "DB0II");
        assert_eq!(route.link_digis, vec!["DB0FN-9"]);
        assert_eq!(
            route.downlinks,
            vec![
                "C! ON5PL".to_string(),
                "C! ON5ZS".to_string(),
                "C LX0PAC".to_string(),
                "C DF3VI DB0HOM".to_string(),
            ]
        );
    }

    #[test]
    fn recursive_paths_and_ident() {
        let n = book(
            "F>DB0FN Digi Siegen\n\
             F>SIEGEN:DB0FN Digi Siegen\n\
             F>DB0WST Netzknoten West; F>DB0FN F>DB0WST\n\
             F>DB0ME Digi Solingen; F>DB0WST F>DB0ME\n\
             B>DB0IZ Mailbox Solingen; F>DB0ME B>DB0IZ\n\
             T>ULF:DH1DAE Ulf; D>SIEGEN T>DH1DAE\n",
        );
        let route = n.route_for("DB0IZ", "").unwrap().unwrap();
        assert_eq!(route.link_call, "DB0FN");
        assert!(route.link_digis.is_empty());
        assert_eq!(
            route.downlinks,
            vec![
                "C DB0WST".to_string(),
                "C DB0ME".to_string(),
                "C DB0IZ".to_string(),
            ]
        );
        let ulf = n.route_for("ULF", "").unwrap().unwrap();
        assert_eq!(ulf.link_call, "DH1DAE");
        assert_eq!(ulf.link_digis, vec!["DB0FN"]);
    }

    #[test]
    fn sp_gateway_port_and_qrg_if() {
        let n = book(
            "PORT0 = DB0FN, 430.625\n\
             PORT1 = DB0NWS, 433.775\n\
             <IF 430.625>\n\
             B>DB0SGL Mailbox Siegen; D>%DIGI% B>DB0SGL\n\
             <END>\n\
             <IF 433.775>\n\
             B>DB0SGL Mailbox Siegen; D>DB0NWS B>DB0SGL\n\
             <END>\n\
             N>DB0XYZ Testdigi; T>DL9ZZZ D>0 N>DB0XYZ\n",
        );
        let sgl = n.route_for("DB0SGL", "430.625").unwrap().unwrap();
        assert_eq!(sgl.link_call, "DB0SGL");
        assert_eq!(sgl.link_digis, vec!["DB0FN"]);
        let other = n.route_for("DB0SGL", "433.775").unwrap().unwrap();
        assert_eq!(other.link_digis, vec!["DB0NWS"]);
        assert!(n.route_for("DB0SGL", "144.800").unwrap().is_none());
        let gw = n.route_for("DB0XYZ", "430.625").unwrap().unwrap();
        assert_eq!(gw.link_call, "DL9ZZZ");
        assert_eq!(gw.downlinks, vec!["C DB0XYZ-0".to_string()]);
    }

    #[test]
    fn set_name_updates_and_appends_without_losing_the_header() {
        let path = std::env::temp_dir().join(format!("gpng-names-{}.GP", std::process::id()));
        let _ = fs::remove_file(&path);
        let raw = b"*** Rufzeichenliste f\x81r Graphic-Packet ***\r\n\r\nT>NL0MSK Jara;\r\n";
        fs::write(&path, raw).unwrap();
        let mut n = NamesFile::load_from(path.clone());
        assert_eq!(n.name_for("NL0MSK", "").as_deref(), Some("Jara"));
        n.set_name("nl0msk", "Jaroslav").unwrap();
        n.set_name("NL1AS", "Bas").unwrap();
        let back = fs::read(&path).unwrap();
        assert!(back.windows(3).any(|w| w == b"f\x81r"), "header umlaut");
        let text = decode_cp437(&back);
        assert!(text.contains("T>NL0MSK Jaroslav;"));
        assert!(text.contains("T>NL1AS Bas;"));
        assert!(text.contains("\r\n"));
        let _ = fs::remove_file(&path);
    }
}
