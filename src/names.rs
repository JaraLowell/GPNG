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
//! matches the integer MHz part of the session QRG (`430` for `430.625`). `PORTx`
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
    pub const ALL: [Self; 7] = [
        Self::Terminal,
        Self::Mailbox,
        Self::Digi,
        Self::Flex,
        Self::NetRom,
        Self::Ware,
        Self::Bang,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Terminal => "T - Terminal",
            Self::Mailbox => "B - Mailbox / BBS",
            Self::Digi => "D - L2 digipeater",
            Self::Flex => "F - FlexNet",
            Self::NetRom => "N - NetRom",
            Self::Ware => "W - DigiWare",
            Self::Bang => "! - C! node",
        }
    }

    fn code(self) -> char {
        match self {
            Self::Terminal => 'T',
            Self::Mailbox => 'B',
            Self::Digi => 'D',
            Self::Flex => 'F',
            Self::NetRom => 'N',
            Self::Ware => 'W',
            Self::Bang => '!',
        }
    }

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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum NameScope {
    #[default]
    All,
    Qrg(String),
    OtherQrg(String),
}

impl NameScope {
    pub fn label(&self) -> String {
        match self {
            Self::All => "All QRGs".into(),
            Self::Qrg(qrg) => format!("QRG {}", qrg_key(qrg)),
            Self::OtherQrg(qrg) => format!("Other QRGs (not {})", qrg_key(qrg)),
        }
    }

    pub fn matches(&self, qrg: &str) -> bool {
        match self {
            Self::All => true,
            Self::Qrg(key) => qrg_key(key) == qrg_key(qrg),
            Self::OtherQrg(key) => qrg_key(key) != qrg_key(qrg),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameEntry {
    pub kind: StationKind,
    pub call: String,
    pub ident: String,
    pub name: String,
    /// Raw DOS path, retaining typed node hops and macros.
    pub via: String,
    pub scope: NameScope,
}

impl Default for NameEntry {
    fn default() -> Self {
        Self {
            kind: StationKind::Terminal,
            call: String::new(),
            ident: String::new(),
            name: String::new(),
            via: String::new(),
            scope: NameScope::All,
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

/// One named station on the connect-dialog list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedName {
    pub call: String,
    pub name: String,
    pub scope: NameScope,
    /// Route for this exact entry, not another entry with the same CALL.
    pub route: Result<Route, String>,
}

impl ListedName {
    /// `CALL (name)` when a name is stored, otherwise the call alone.
    pub fn button_label(&self) -> String {
        let name = self.name.trim();
        if name.is_empty() {
            self.call.clone()
        } else {
            format!("{} ({name})", self.call)
        }
    }
}

#[derive(Debug, Clone)]
pub struct NamesFile {
    path: PathBuf,
    lines: Vec<String>,
    newline: String,
}

struct IfBlock {
    start: usize,
    otherwise: Option<usize>,
    end: usize,
    key: String,
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

    /// Editable stations, including inactive IF/ELSE blocks. The index identifies
    /// the original line so editing never overwrites another QRG's entry.
    pub fn entries(&self) -> Vec<(usize, NameEntry)> {
        let mut scope = NameScope::All;
        let mut out = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            let trimmed = line.trim();
            if let Some(key) = if_key(trimmed) {
                scope = NameScope::Qrg(key.to_string());
            } else if is_tag(trimmed, "ELSE") {
                if let NameScope::Qrg(key) = &scope {
                    scope = NameScope::OtherQrg(key.clone());
                }
            } else if is_tag(trimmed, "END") {
                scope = NameScope::All;
            } else if let Some(station) = parse_station(trimmed) {
                out.push((
                    index,
                    NameEntry {
                        kind: station.kind,
                        call: station.call,
                        ident: station.ident.unwrap_or_default(),
                        name: station.name,
                        via: trimmed
                            .split_once(';')
                            .map(|(_, via)| via.trim())
                            .unwrap_or("")
                            .to_string(),
                        scope: scope.clone(),
                    },
                ));
            }
        }
        out
    }

    pub fn save_entry(&mut self, source: Option<usize>, entry: &NameEntry) -> Result<(), String> {
        let call = normalize_mycall(&entry.call)?;
        let name = clip_name(&entry.name);
        if entry.name.contains([';', '>', '\r', '\n']) {
            return Err("NAMES.GP: name cannot contain ;, > or a newline".into());
        }
        let ident = entry.ident.trim().to_ascii_uppercase();
        if ident
            .chars()
            .any(|c| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
        {
            return Err("NAMES.GP: ident must contain only letters, digits, - or _".into());
        }
        let scope = match &entry.scope {
            NameScope::All => NameScope::All,
            NameScope::Qrg(key) | NameScope::OtherQrg(key) => {
                let key = qrg_key(key);
                if key.is_empty() || !key.chars().all(|c| c.is_ascii_digit()) {
                    return Err("NAMES.GP: QRG must be an integer MHz number".into());
                }
                if matches!(entry.scope, NameScope::OtherQrg(_)) {
                    NameScope::OtherQrg(key.to_string())
                } else {
                    NameScope::Qrg(key.to_string())
                }
            }
        };
        let via = entry.via.trim().to_ascii_uppercase();
        if entry.via.contains([';', '\r', '\n', '<'])
            || via.split_whitespace().any(|hop| {
                if !hop.contains('>') {
                    return false;
                }
                let Some((_, token)) = split_type(hop) else {
                    return true;
                };
                token.is_empty()
                    || token.contains('>')
                    || (token.contains(':') && split_ident_call(token).is_none())
                    || (token.contains('-')
                        && !token.contains(':')
                        && normalize_mycall(token).is_err()
                        && !self.entries().iter().any(|(_, entry)| entry.ident == token))
                    || (!token.eq_ignore_ascii_case("%DIGI%")
                        && !token
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':')))
            })
        {
            return Err("NAMES.GP: VIA must contain calls or TYPE>CALL hops".into());
        }
        let token = if ident.is_empty() {
            call.clone()
        } else {
            format!("{ident}:{call}")
        };
        let line = format!("{}>{token} {name}; {via}", entry.kind.code());
        if parse_station(&line).is_none_or(|station| !via.is_empty() && station.hops.is_empty()) {
            return Err("NAMES.GP: invalid VIA path".into());
        }
        if decode_cp437(&encode_cp437(&line)) != line {
            return Err("NAMES.GP: use characters available in DOS CP437".into());
        }
        let entries = self.entries();
        let original = source
            .map(|index| {
                entries
                    .iter()
                    .find(|(i, _)| *i == index)
                    .ok_or_else(|| "NAMES.GP: selected entry no longer exists".to_string())
            })
            .transpose()?;
        let same_scope = |other: &NameScope| match (&scope, other) {
            (NameScope::All, NameScope::All) => true,
            (NameScope::Qrg(a), NameScope::Qrg(b))
            | (NameScope::OtherQrg(a), NameScope::OtherQrg(b)) => qrg_key(a) == qrg_key(b),
            _ => false,
        };
        if entries.iter().any(|(i, other)| {
            Some(*i) != source && other.call.eq_ignore_ascii_case(&call) && same_scope(&other.scope)
        }) {
            return Err(
                "NAMES.GP: that CALL already exists for this QRG; select it to edit".into(),
            );
        }
        let mut next = self.clone();
        let mut insert = None;
        if let Some((index, _)) = original.filter(|(_, old)| same_scope(&old.scope)) {
            next.lines[*index] = line;
        } else {
            if let Some((index, _)) = original {
                next.lines.remove(*index);
            }
            insert = Some(line);
        }
        next.merge_if_blocks()?;
        if let Some(line) = insert {
            next.insert_scoped_line(&scope, line)?;
        }
        next.save()?;
        *self = next;
        Ok(())
    }

    fn if_blocks(&self) -> Result<Vec<IfBlock>, String> {
        let mut blocks = Vec::new();
        let mut current: Option<IfBlock> = None;
        for (index, line) in self.lines.iter().enumerate() {
            let trimmed = line.trim();
            if let Some(key) = if_key(trimmed) {
                if current.is_some() {
                    return Err(format!("NAMES.GP: nested IF at line {}", index + 1));
                }
                current = Some(IfBlock {
                    start: index,
                    otherwise: None,
                    end: index,
                    key: qrg_key(key).to_string(),
                });
            } else if is_tag(trimmed, "ELSE") {
                let block = current
                    .as_mut()
                    .ok_or_else(|| format!("NAMES.GP: ELSE without IF at line {}", index + 1))?;
                if block.otherwise.replace(index).is_some() {
                    return Err(format!("NAMES.GP: repeated ELSE at line {}", index + 1));
                }
            } else if is_tag(trimmed, "END") {
                let mut block = current
                    .take()
                    .ok_or_else(|| format!("NAMES.GP: END without IF at line {}", index + 1))?;
                block.end = index;
                blocks.push(block);
            }
        }
        if let Some(block) = current {
            return Err(format!(
                "NAMES.GP: IF at line {} has no END",
                block.start + 1
            ));
        }
        Ok(blocks)
    }

    fn merge_if_blocks(&mut self) -> Result<(), String> {
        let blocks = self.if_blocks()?;
        let mut lines = Vec::new();
        let mut cursor = 0;
        for (index, block) in blocks.iter().enumerate() {
            lines.extend_from_slice(&self.lines[cursor..block.start]);
            cursor = block.end + 1;
            if blocks[..index].iter().any(|other| other.key == block.key) {
                continue;
            }
            let group: Vec<_> = blocks
                .iter()
                .filter(|other| other.key == block.key)
                .collect();
            if group.len() == 1 {
                lines.extend_from_slice(&self.lines[block.start..=block.end]);
                continue;
            }
            lines.push(self.lines[block.start].clone());
            for other in &group {
                lines.extend_from_slice(
                    &self.lines[other.start + 1..other.otherwise.unwrap_or(other.end)],
                );
            }
            if let Some(otherwise) = group.iter().find_map(|other| other.otherwise) {
                lines.push(self.lines[otherwise].clone());
                for other in &group {
                    if let Some(otherwise) = other.otherwise {
                        lines.extend_from_slice(&self.lines[otherwise + 1..other.end]);
                    }
                }
            }
            lines.push(self.lines[block.end].clone());
        }
        lines.extend_from_slice(&self.lines[cursor..]);
        self.lines = lines;
        Ok(())
    }

    fn insert_scoped_line(&mut self, scope: &NameScope, line: String) -> Result<(), String> {
        if let NameScope::Qrg(key) | NameScope::OtherQrg(key) = scope {
            if let Some(block) = self
                .if_blocks()?
                .into_iter()
                .find(|block| block.key == *key)
            {
                if matches!(scope, NameScope::Qrg(_)) {
                    self.lines
                        .insert(block.otherwise.unwrap_or(block.end), line);
                } else if block.otherwise.is_some() {
                    self.lines.insert(block.end, line);
                } else {
                    self.lines
                        .splice(block.end..block.end, ["<ELSE>".into(), line]);
                }
                return Ok(());
            }
        }
        if self.lines.last().is_some_and(|l| !l.is_empty()) {
            self.lines.push(String::new());
        }
        match scope {
            NameScope::All => self.lines.push(line),
            NameScope::Qrg(key) => self
                .lines
                .extend([format!("<IF {key}>"), line, "<END>".into()]),
            NameScope::OtherQrg(key) => {
                self.lines
                    .extend([format!("<IF {key}>"), "<ELSE>".into(), line, "<END>".into()])
            }
        }
        Ok(())
    }

    pub fn remove_entry(&mut self, source: usize) -> Result<String, String> {
        let call = self
            .entries()
            .into_iter()
            .find(|(index, _)| *index == source)
            .map(|(_, entry)| entry.call)
            .ok_or_else(|| "NAMES.GP: selected entry no longer exists".to_string())?;
        let mut next = self.clone();
        next.lines.remove(source);
        next.save()?;
        *self = next;
        Ok(call)
    }

    /// Name stored for `call` at this QRG (integer MHz). Empty QRG still sees
    /// lines that sit outside `<IF>`.
    ///
    /// A redirected connect arrives as `NL1AS-1`. That uses the name stored
    /// for `NL1AS`, and the other way round: a line written as `NL1AS-10`
    /// (a BBS on that SSID) still names a connect to `NL1AS`. An exact
    /// SSID in the file wins over the bare call.
    pub fn name_for(&self, call: &str, qrg: &str) -> Option<String> {
        let call = normalize_mycall(call).ok()?;
        let stations = self.active(qrg);
        let take = |matches: &dyn Fn(&Station) -> bool| {
            stations
                .iter()
                .find(|s| matches(s) && !s.name.is_empty())
                .map(|s| s.name.clone())
        };
        if let Some(name) = take(&|s| s.call.eq_ignore_ascii_case(&call)) {
            return Some(name);
        }
        let base = call_base(&call);
        if let Some(name) = take(&|s| s.call.eq_ignore_ascii_case(base)) {
            return Some(name);
        }
        take(&|s| call_base(&s.call).eq_ignore_ascii_case(base))
    }

    /// `NAMES.GP` type for this call on the current QRG, if the station is listed.
    pub fn kind_of(&self, call: &str, qrg: &str) -> Option<StationKind> {
        let call = normalize_mycall(call).ok()?;
        self.active(qrg)
            .into_iter()
            .find(|s| s.call.eq_ignore_ascii_case(&call))
            .map(|s| s.kind)
    }

    /// Global stations first, then stations active on this QRG. File order is
    /// retained within each group. Routing errors stay attached to their row.
    /// A station with no name is still listed; the button shows the call only.
    pub fn name_list(&self, qrg: &str) -> Vec<ListedName> {
        let mut listed: Vec<_> = self
            .entries()
            .into_iter()
            .filter(|(_, entry)| entry.scope.matches(qrg))
            .map(|(index, entry)| {
                let route = self
                    .lines
                    .get(index)
                    .and_then(|line| parse_station(line))
                    .ok_or_else(|| "NAMES.GP: invalid selected station".to_string())
                    .and_then(|station| self.route_for_station(&station, qrg));
                ListedName {
                    call: entry.call,
                    name: entry.name,
                    scope: entry.scope,
                    route,
                }
            })
            .collect();
        listed.sort_by_key(|entry| entry.scope != NameScope::All);
        listed
    }

    /// Call, ident, or name → route. `Ok(None)` means the token is not in the file.
    pub fn route_for(&self, query: &str, qrg: &str) -> Result<Option<Route>, String> {
        let Some(station) = self.find_query(query, qrg) else {
            return Ok(None);
        };
        self.route_for_station(&station, qrg).map(Some)
    }

    fn route_for_station(&self, station: &Station, qrg: &str) -> Result<Route, String> {
        let hops = self.expand_hops(station, &self.active(qrg), qrg, &mut Vec::new())?;
        Ok(plan(station, &hops))
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
            fs::create_dir_all(parent).map_err(|e| format!("NAMES.GP: {e}"))?;
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
                include = qrg_key(key) == qrg_key(qrg);
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
        self.expand_hops(st, &stations, qrg, stack)
    }

    fn expand_hops(
        &self,
        st: &Station,
        stations: &[Station],
        qrg: &str,
        stack: &mut Vec<String>,
    ) -> Result<Vec<Hop>, String> {
        let call = &st.call;
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
            let token = self.subst_token(&hop.token, qrg, stations)?;
            if token.eq_ignore_ascii_case(call) {
                out.push(Hop {
                    kind: hop.kind,
                    token,
                });
                continue;
            }
            let known = self.by_call(stations, &token);
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
        if token.contains(':') {
            return split_ident_call(token)
                .map(|(_, call)| call)
                .ok_or_else(|| format!("NAMES.GP: invalid path hop {token}"));
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

pub fn qrg_key(qrg: &str) -> &str {
    qrg.trim()
        .split_once('.')
        .map(|(mhz, _)| mhz)
        .unwrap_or(qrg.trim())
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

/// `NL1AS-1` → `NL1AS`. A suffix that is not a numeric SSID stays put.
fn call_base(call: &str) -> &str {
    match call.split_once('-') {
        Some((base, ssid))
            if !base.is_empty() && !ssid.is_empty() && ssid.chars().all(|c| c.is_ascii_digit()) =>
        {
            base
        }
        _ => call,
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

    fn with_names(text: &str, test: impl FnOnce(&mut NamesFile)) {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("gpng-names-{}-{unique}.GP", std::process::id()));
        fs::write(&path, encode_cp437(text)).unwrap();
        let mut names = NamesFile::load_from(path.clone());
        test(&mut names);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn qrg_integer_matching_selects_if_else_and_typed_routes() {
        with_names(
            "<IF 145.500>\nT>N0PEER VHF; N>NODE:N0NODE-1 D>N0DIGI T>N0PEER\n\
                <ELSE>\nT>N0PEER UHF; D>N1DIGI\n<END>\nT>N2PEER Global;",
            |names| {
                assert_eq!(
                    names.name_for("N0PEER-2", "145.825").as_deref(),
                    Some("VHF")
                );
                assert_eq!(names.name_for("N0PEER", "430.625").as_deref(), Some("UHF"));
                assert_eq!(names.name_for("N2PEER", "").as_deref(), Some("Global"));
                let route = names.route_for("N0PEER", "145").unwrap().unwrap();
                assert_eq!(route.link_call, "N0NODE-1");
                assert_eq!(route.downlinks, ["C N0PEER N0DIGI"]);
            },
        );
    }

    #[test]
    fn editing_one_qrg_preserves_other_entries_comments_ports_and_cp437() {
        with_names(
            "*** Comment\r\nPORT0 = N0DIGI,145.500\r\n\
                <IF 145.500>\r\nT>N0PEER Old; D>N0DIGI\r\n<END>\r\n\
                <IF 430>\r\nT>N0PEER UHF; D>N1DIGI\r\n<END>\r\n",
            |names| {
                let (index, mut entry) = names.entries().remove(0);
                entry.name = "André".into();
                entry.kind = StationKind::Mailbox;
                entry.via = "N>N2NODE B>N0PEER".into();
                entry.ident = "PEER".into();
                entry.via = "N>NODE:N2NODE-1 B>N0PEER".into();
                names.save_entry(Some(index), &entry).unwrap();
                let reloaded = NamesFile::load_from(names.path.clone());
                assert_eq!(
                    reloaded.name_for("N0PEER", "145.825").as_deref(),
                    Some("André")
                );
                assert_eq!(
                    reloaded.kind_of("N0PEER", "145"),
                    Some(StationKind::Mailbox)
                );
                assert_eq!(
                    reloaded.name_for("N0PEER", "430.625").as_deref(),
                    Some("UHF")
                );
                let text = decode_cp437(&fs::read(names.path()).unwrap());
                assert!(text.starts_with("*** Comment\r\nPORT0 = N0DIGI,145.500\r\n"));
                assert!(text.contains("<IF 430>\r\nT>N0PEER UHF; D>N1DIGI\r\n<END>\r\n"));
                assert_eq!(
                    reloaded
                        .route_for("PEER", "145")
                        .unwrap()
                        .unwrap()
                        .link_call,
                    "N2NODE-1"
                );
            },
        );
    }

    #[test]
    fn adding_and_moving_scopes_preserves_else_semantics() {
        with_names(
            "<IF 145>\nT>N0PEER VHF;\n<ELSE>\nT>N1PEER Other;\n<END>",
            |names| {
                let (index, mut entry) = names.entries().remove(1);
                entry.name = "Renamed".into();
                names.save_entry(Some(index), &entry).unwrap();
                assert_eq!(names.name_for("N1PEER", "430").as_deref(), Some("Renamed"));
                assert!(names.name_for("N1PEER", "145").is_none());
                entry.scope = NameScope::Qrg("430.625".into());
                names.save_entry(Some(index), &entry).unwrap();
                assert!(names.name_for("N1PEER", "144").is_none());
                assert_eq!(
                    names.name_for("N1PEER", "430.825").as_deref(),
                    Some("Renamed")
                );
                let new = NameEntry {
                    call: "N2PEER".into(),
                    name: "Manual".into(),
                    via: "n0digi,n1digi".into(),
                    ..NameEntry::default()
                };
                names.save_entry(None, &new).unwrap();
                let reloaded = NamesFile::load_from(names.path.clone());
                assert_eq!(
                    reloaded
                        .route_for("N2PEER", "145")
                        .unwrap()
                        .unwrap()
                        .link_digis,
                    ["N0DIGI", "N1DIGI"]
                );
            },
        );
    }

    #[test]
    fn connect_names_list_groups_globals_before_active_qrg_and_keeps_each_rows_route() {
        with_names(
            "<IF 27>\nT>N0AAA Band; D>N0BAND\n<END>\n\
            T>N1BBB Global first; D>N0DIGI\nT>N0AAA Global twin; D>N1DIGI\n\
            <IF 145>\nT>N2CCC Hidden;\n<ELSE>\nT>N3DDD Fallback;\n<END>\n\
            T>N4EEE Global last;",
            |names| {
                let listed = names.name_list("27.235");
                let labels: Vec<_> = listed.iter().map(|entry| entry.button_label()).collect();
                assert_eq!(
                    labels,
                    [
                        "N1BBB (Global first)",
                        "N0AAA (Global twin)",
                        "N4EEE (Global last)",
                        "N0AAA (Band)",
                        "N3DDD (Fallback)",
                    ]
                );
                assert!(listed[..3]
                    .iter()
                    .all(|entry| entry.scope == NameScope::All));
                assert!(listed[3..]
                    .iter()
                    .all(|entry| entry.scope != NameScope::All));
                assert_eq!(listed[1].route.as_ref().unwrap().link_digis, ["N1DIGI"]);
                assert_eq!(listed[3].route.as_ref().unwrap().link_digis, ["N0BAND"]);
                assert!(names
                    .name_list("145.825")
                    .iter()
                    .any(|entry| entry.name == "Hidden"));
                assert!(!names
                    .name_list("145.825")
                    .iter()
                    .any(|entry| entry.name == "Band" || entry.name == "Fallback"));
            },
        );
    }

    #[test]
    fn saving_reuses_and_merges_qrg_blocks_when_adding_editing_and_moving_stations() {
        with_names(
            "*** Comment\r\nPORT0 = N0DIGI,27.235\r\n\
            <IF 27.235>\r\nT>N0AAA First; D>N0DIGI\r\n<END>\r\n\
            T>N3DDD Global;\r\n\
            <IF 145>\r\nT>N2CCC Move;\r\n<END>\r\n\
            <IF 27>\r\n*** Second block\r\nT>N1BBB Second;\r\n<END>\r\n",
            |names| {
                let entry = NameEntry {
                    call: "N4EEE".into(),
                    name: "New".into(),
                    scope: NameScope::Qrg("27.825".into()),
                    ..NameEntry::default()
                };
                names.save_entry(None, &entry).unwrap();
                let blocks = names.if_blocks().unwrap();
                assert_eq!(blocks.iter().filter(|block| block.key == "27").count(), 1);
                let block = blocks.iter().find(|block| block.key == "27").unwrap();
                let calls: Vec<_> = names.lines[block.start + 1..block.end]
                    .iter()
                    .filter_map(|line| parse_station(line).map(|station| station.call))
                    .collect();
                assert_eq!(calls, ["N0AAA", "N1BBB", "N4EEE"]);
                let (index, mut second) = names
                    .entries()
                    .into_iter()
                    .find(|(_, entry)| entry.call == "N1BBB")
                    .unwrap();
                second.name = "Edited".into();
                names.save_entry(Some(index), &second).unwrap();
                let (index, mut moved) = names
                    .entries()
                    .into_iter()
                    .find(|(_, entry)| entry.call == "N2CCC")
                    .unwrap();
                moved.scope = NameScope::Qrg("27".into());
                names.save_entry(Some(index), &moved).unwrap();
                let reloaded = NamesFile::load_from(names.path.clone());
                assert_eq!(
                    reloaded
                        .if_blocks()
                        .unwrap()
                        .iter()
                        .filter(|block| block.key == "27")
                        .count(),
                    1
                );
                for (call, name) in [
                    ("N0AAA", "First"),
                    ("N1BBB", "Edited"),
                    ("N2CCC", "Move"),
                    ("N4EEE", "New"),
                ] {
                    assert_eq!(reloaded.name_for(call, "27.235").as_deref(), Some(name));
                    assert!(reloaded.name_for(call, "145").is_none());
                }
                assert_eq!(reloaded.name_for("N3DDD", "145").as_deref(), Some("Global"));
                assert_eq!(
                    reloaded
                        .route_for("N0AAA", "27.235")
                        .unwrap()
                        .unwrap()
                        .link_digis,
                    ["N0DIGI"]
                );
                let text = decode_cp437(&fs::read(names.path()).unwrap());
                assert!(text.starts_with("*** Comment\r\nPORT0 = N0DIGI,27.235\r\n"));
                assert!(text.contains("*** Second block\r\n"));
                assert!(!text.replace("\r\n", "").contains('\n'));
            },
        );
    }

    #[test]
    fn merged_if_else_blocks_keep_branches_and_accept_entries_in_both() {
        with_names(
            "<IF 27>\nT>N0AAA First;\n<ELSE>\nT>N1BBB Other;\n<END>\n\
            <IF 27.235>\nT>N2CCC Second;\n<ELSE>\nT>N3DDD Fallback;\n<END>",
            |names| {
                let mut entry = NameEntry {
                    call: "N4EEE".into(),
                    name: "Added".into(),
                    scope: NameScope::Qrg("27".into()),
                    ..NameEntry::default()
                };
                names.save_entry(None, &entry).unwrap();
                entry.call = "N5FFF".into();
                entry.scope = NameScope::OtherQrg("27".into());
                names.save_entry(None, &entry).unwrap();
                let reloaded = NamesFile::load_from(names.path.clone());
                assert_eq!(reloaded.if_blocks().unwrap().len(), 1);
                assert_eq!(
                    reloaded
                        .lines
                        .iter()
                        .filter(|line| is_tag(line, "ELSE"))
                        .count(),
                    1
                );
                for call in ["N0AAA", "N2CCC", "N4EEE"] {
                    assert!(reloaded.name_for(call, "27.825").is_some());
                    assert!(reloaded.name_for(call, "145").is_none());
                }
                for call in ["N1BBB", "N3DDD", "N5FFF"] {
                    assert!(reloaded.name_for(call, "27.825").is_none());
                    assert!(reloaded.name_for(call, "145").is_some());
                }
                let block = &reloaded.if_blocks().unwrap()[0];
                let before_else: Vec<_> = reloaded.lines[block.start + 1..block.otherwise.unwrap()]
                    .iter()
                    .filter_map(|line| parse_station(line).map(|s| s.call))
                    .collect();
                assert_eq!(before_else, ["N0AAA", "N2CCC", "N4EEE"]);
            },
        );
    }

    #[test]
    fn saving_unclosed_if_blocks_reports_error_without_overwriting_file() {
        with_names("<IF 27>\nT>N0AAA First;", |names| {
            let before = names.lines.clone();
            let bytes = fs::read(names.path()).unwrap();
            let entry = NameEntry {
                call: "N1BBB".into(),
                scope: NameScope::Qrg("27".into()),
                ..NameEntry::default()
            };
            assert!(names
                .save_entry(None, &entry)
                .unwrap_err()
                .contains("has no END"));
            assert_eq!(names.lines, before);
            assert_eq!(fs::read(names.path()).unwrap(), bytes);
        });
    }

    #[test]
    fn removing_selected_station_preserves_other_qrgs_and_dos_file_structure() {
        let text = "*** Comment\r\nPORT0 = N0DIGI,27.235\r\n\
            <IF 27>\r\nT>N0PEER Eleven; D>N0DIGI\r\n<END>\r\n\
            <IF 145>\r\nT>N0PEER VHF;\r\n<END>\r\n";
        with_names(text, |names| {
            let index = names.entries()[0].0;
            names.remove_entry(index).unwrap();
            let expected = text.replace("T>N0PEER Eleven; D>N0DIGI\r\n", "");
            assert_eq!(fs::read(names.path()).unwrap(), encode_cp437(&expected));
            let reloaded = NamesFile::load_from(names.path.clone());
            assert!(reloaded.name_for("N0PEER", "27.235").is_none());
            assert_eq!(
                reloaded.name_for("N0PEER", "145.825").as_deref(),
                Some("VHF")
            );
        });
    }

    #[test]
    fn invalid_entries_and_write_failures_do_not_change_the_loaded_names() {
        with_names("T>N0PEER Existing;", |names| {
            let before = names.lines.clone();
            let bytes = fs::read(names.path()).unwrap();
            for via in ["D>", "D>N0DIGI\nT>N1PEER", "D>N0DIGI-16", ",", "D>N0DIGI;"] {
                let entry = NameEntry {
                    call: "N1PEER".into(),
                    via: via.into(),
                    ..NameEntry::default()
                };
                assert!(names.save_entry(None, &entry).is_err(), "{via}");
            }
            let duplicate = NameEntry {
                call: "N0PEER".into(),
                ..NameEntry::default()
            };
            assert!(names.save_entry(None, &duplicate).is_err());
            assert!(names.remove_entry(99).is_err());
            assert_eq!(names.lines, before);
            assert_eq!(fs::read(names.path()).unwrap(), bytes);
            let original_path = names.path.clone();
            names.path = original_path.join("NAMES.GP");
            let entry = NameEntry {
                call: "N1PEER".into(),
                ..NameEntry::default()
            };
            assert!(names.save_entry(None, &entry).is_err());
            assert!(names.remove_entry(0).is_err());
            assert_eq!(names.lines, before);
            names.path = original_path;
        });
    }
}
