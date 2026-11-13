//! AX.25 L2 helpers for the **KISS** driver path (no FCS — the TNC adds CRC).
//!
//! Encoding/decoding uses the [`ax25`] crate. WA8DED/TFPCX keep L2 inside the
//! TNC via hostmode and do **not** use this module.
//!
//! Monitor / unproto → **UI** frames. The path is the beacon address (`CQ`, or
//! `APMI03 WIDE1-1 WIDE2-2`). Source is the active mycall.
//! Connected channels → **SABM** / **UA** / **I** / **RR** / **REJ** / **DISC** / **DM**.

use ax25::frame::{
    Address, Ax25Frame as LibFrame, CommandResponse, Disconnect, DisconnectedMode, FrameContent,
    Information, ProtocolIdentifier, ReceiveReady, Reject, RouteEntry, SetAsynchronousBalancedMode,
    UnnumberedAcknowledge, UnnumberedInformation,
};

/// AX.25 PID: no layer 3 (`0xF0`).
pub const PID_NO_L3: u8 = 0xF0;

/// Parsed control field / frame class (stable GUI-facing enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ax25FrameType {
    Ui,
    Sabm,
    Ua,
    Disc,
    Dm,
    Rr,
    Rnr,
    Rej,
    Frmr,
    Info,
    Other,
}

/// One decoded AX.25 frame (addresses + control class + info).
#[derive(Debug, Clone)]
pub struct Ax25Frame {
    pub dest: String,
    pub src: String,
    pub via: Vec<String>,
    /// Parallel to [`Self::via`]. True after that digipeater has repeated the frame (H bit).
    pub repeated: Vec<bool>,
    pub kind: Ax25FrameType,
    /// N(S) for I frames.
    pub ns: u8,
    /// N(R) for I / S frames.
    pub nr: u8,
    /// P/F bit from the control field.
    pub poll: bool,
    /// AX.25 v2 command (vs response) from address C-bits.
    pub command: bool,
    pub info: Vec<u8>,
}

/// Classic GP / axlisten P/F mark: cmd+P `+`, cmd `^`, resp+F `-`, resp `v`.
pub fn pf_mark(command: bool, poll: bool) -> char {
    match (command, poll) {
        (true, true) => '+',
        (true, false) => '^',
        (false, true) => '-',
        (false, false) => 'v',
    }
}

/// GP DOS / axlisten header: `fm SRC to DEST [via …] ctl …`.
///
/// I/S order is N(R) then N(S) (`I00^`, `RR1v`). I/UI append `pid F0`.
pub fn monitor_ctl_line(
    src: &str,
    dest: &str,
    via: &[String],
    kind: Ax25FrameType,
    ns: u8,
    nr: u8,
    poll: bool,
    command: bool,
) -> String {
    let via = if via.is_empty() {
        String::new()
    } else {
        format!(" via {}", via.join(","))
    };
    let m = pf_mark(command, poll);
    let ctl = match kind {
        Ax25FrameType::Ui => format!("UI{m} pid F0"),
        Ax25FrameType::Sabm => format!("SABM{m}"),
        Ax25FrameType::Ua => format!("UA{m}"),
        Ax25FrameType::Disc => format!("DISC{m}"),
        Ax25FrameType::Dm => format!("DM{m}"),
        Ax25FrameType::Rr => format!("RR{nr}{m}"),
        Ax25FrameType::Rnr => format!("RNR{nr}{m}"),
        Ax25FrameType::Rej => format!("REJ{nr}{m}"),
        Ax25FrameType::Frmr => format!("FRMR{m}"),
        Ax25FrameType::Info => format!("I{nr}{ns}{m} pid F0"),
        Ax25FrameType::Other => format!("???{m}"),
    };
    format!("fm {src} to {dest}{via} ctl {ctl}")
}

/// `fm CALL…` or `fm2 CALL…`. The head includes the trailing space.
///
/// Digits glued to `fm` are the KISS port. A space and then `HH:MM` is the
/// clock, so port 0 stays `fm` and is not read as a port number.
pub fn split_monitor_fm(line: &str) -> Option<(&str, &str)> {
    let rest = line.trim_start();
    let b = rest.as_bytes();
    if b.len() < 3 || !b[..2].eq_ignore_ascii_case(b"fm") {
        return None;
    }
    let mut i = 2;
    if b[2].is_ascii_digit() {
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if b.get(i) != Some(&b' ') {
        return None;
    }
    let head_end = line.len() - rest.len() + i + 1;
    Some((&line[..head_end], &line[head_end..]))
}

/// Port 0 leaves `fm CALL`. Any other KISS port becomes `fm2 CALL`.
pub fn tag_kiss_port(line: &str, port: u8) -> String {
    if port == 0 {
        return line.to_string();
    }
    let Some((head, tail)) = split_monitor_fm(line) else {
        return line.to_string();
    };
    let word = head.trim();
    if word.len() != 2 {
        return line.to_string();
    }
    let lead_len = head.len() - head.trim_start().len();
    let lead = &head[..lead_len];
    let fm = &head[lead_len..lead_len + 2];
    format!("{lead}{fm}{port} {tail}")
}

/// `fm SRC …` → `fm HH:MM SRC …` (24-hour, already-stamped lines left as-is).
/// A non-zero KISS port stays glued to `fm`: `fm2 HH:MM SRC …`.
pub fn insert_monitor_time(line: &str, hhmm: &str) -> String {
    let Some((head, tail)) = split_monitor_fm(line) else {
        return line.to_string();
    };
    if tail.split_whitespace().next().is_some_and(is_hhmm) {
        return line.to_string();
    }
    format!("{head}{hhmm} {tail}")
}

fn is_hhmm(token: &str) -> bool {
    let Some((h, m)) = token.split_once(':') else {
        return false;
    };
    h.len() == 2
        && m.len() == 2
        && h.bytes().all(|b| b.is_ascii_digit())
        && m.bytes().all(|b| b.is_ascii_digit())
}

impl Ax25Frame {
    /// Every digipeater H bit is set, or there is no via list.
    /// AX.25 delivers the frame to the endpoint only then.
    pub fn path_complete(&self) -> bool {
        self.repeated.iter().all(|done| *done)
    }

    /// Via calls for the monitor. A repeated hop is marked `CALL*`.
    pub fn via_labels(&self) -> Vec<String> {
        self.via
            .iter()
            .enumerate()
            .map(|(i, call)| {
                if self.repeated.get(i).copied().unwrap_or(false) {
                    format!("{call}*")
                } else {
                    call.clone()
                }
            })
            .collect()
    }

    /// Monitor header line for this decoded frame.
    pub fn monitor_line(&self) -> String {
        monitor_ctl_line(
            &self.src,
            &self.dest,
            &self.via_labels(),
            self.kind,
            self.ns,
            self.nr,
            self.poll,
            self.command,
        )
    }
}

/// One indented monitor info row.
///
/// CR, LF, and every other non-printable stay on that row as `<0x0d>` /
/// `<0x1c>` instead of opening a new line. A trailing CR/LF run is the
/// frame end and is not shown. Empty info yields nothing.
pub fn monitor_info_line(text: &str) -> Option<String> {
    let text = text.trim_end_matches(['\r', '\n']);
    if text.is_empty() {
        return None;
    }
    let escaped = escape_monitor_text(text);
    if escaped.is_empty() {
        None
    } else {
        Some(with_info_indent(&escaped))
    }
}

/// Same rows `monitor_info_line` produces. Kept for callers that expect a list.
pub fn monitor_payload_lines(text: &str) -> Vec<String> {
    monitor_info_line(text).into_iter().collect()
}

/// Replace characters that must not hit the text layout.
///
/// ASCII, the DOS pictures, and every other printable character stay,
/// including UTF-8 (`ä`, `€`, emoji). C0 controls (CR/LF/TAB) and DEL
/// become `<0x1c>` so they cannot open a new monitor row.
pub fn escape_monitor_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            out.push_str(&monitor_nonprint_token(c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// Monitor rows for one hostmode payload.
///
/// An `fm …` header is its own row. The break after that header is the
/// frame structure and is not drawn. Everything after it, including further
/// CR/LF, is one indented info row with non-printables shown as hex.
pub fn monitor_display_lines_from_bytes(raw: &[u8], extended: bool) -> Vec<String> {
    let raw = trim_trailing_crlf(raw);
    if raw.is_empty() {
        return Vec::new();
    }
    if !starts_with_fm(raw) {
        return info_row(raw, extended).into_iter().collect();
    }
    let Some(header_end) = raw.iter().position(|&b| b == b'\r' || b == b'\n') else {
        return info_header(raw, extended).into_iter().collect();
    };
    let mut lines = Vec::new();
    if let Some(header) = info_header(&raw[..header_end], extended) {
        lines.push(header);
    }
    let mut i = header_end + 1;
    if raw[header_end] == b'\r' && raw.get(i) == Some(&b'\n') {
        i += 1;
    }
    if let Some(info) = info_row(trim_trailing_crlf(&raw[i..]), extended) {
        lines.push(info);
    }
    lines
}

/// Info field of a KISS frame, before the three-space indent.
///
/// ASCII stays ASCII. A complete UTF-8 character is shown as itself, with
/// the äöü button on or off. A lone high byte uses the CP437 picture, and
/// umlaut-on remaps the seven Latin-1 äöü bytes like the channel pads.
/// C0 controls and DEL become `<0xNN>`.
pub fn bytes_to_monitor_body(raw: &[u8], extended: bool) -> String {
    let mut out = String::new();
    append_monitor_bytes(&mut out, trim_trailing_crlf(raw), extended);
    out
}

fn info_header(raw: &[u8], extended: bool) -> Option<String> {
    let mut out = String::new();
    append_monitor_bytes(&mut out, raw, extended);
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn info_row(raw: &[u8], extended: bool) -> Option<String> {
    let mut out = String::new();
    append_monitor_bytes(&mut out, raw, extended);
    if out.is_empty() {
        None
    } else {
        Some(with_info_indent(&out))
    }
}

fn with_info_indent(text: &str) -> String {
    if text.starts_with("   ") {
        text.to_string()
    } else {
        format!("   {text}")
    }
}

fn starts_with_fm(raw: &[u8]) -> bool {
    raw.len() >= 3 && raw[..3].eq_ignore_ascii_case(b"fm ")
}

fn trim_trailing_crlf(raw: &[u8]) -> &[u8] {
    let mut end = raw.len();
    while end > 0 && matches!(raw[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    &raw[..end]
}

fn monitor_nonprint_token(code: u32) -> String {
    if code <= 0xFF {
        format!("<0x{code:02x}>")
    } else {
        format!("<U+{code:04X}>")
    }
}

fn push_cp437_byte(out: &mut String, b: u8, umlaut: bool) {
    let glyph = crate::ui::editor::cp437_char(b);
    if umlaut {
        out.push_str(&crate::ui::editor::show_channel_text(
            &glyph.to_string(),
            true,
        ));
    } else {
        out.push(glyph);
    }
}

fn append_monitor_bytes(out: &mut String, raw: &[u8], extended: bool) {
    let mut i = 0;
    while i < raw.len() {
        let b = raw[i];
        // C0 + DEL — never a console text glyph on the monitor.
        if b < 0x20 || b == 0x7F {
            out.push_str(&monitor_nonprint_token(u32::from(b)));
            i += 1;
            continue;
        }
        if b < 0x80 {
            out.push(b as char);
            i += 1;
            continue;
        }
        // A finished UTF-8 character, with the äöü button on or off.
        // A lone high byte falls through to the DOS picture below.
        if let Some((ch, len)) = next_utf8(&raw[i..]) {
            if len > 1 && !ch.is_control() {
                out.push(ch);
                i += len;
                continue;
            }
        }
        // Lone high byte → CP437 picture Consolas can draw for GP DOS.
        push_cp437_byte(out, b, extended);
        i += 1;
    }
}

fn next_utf8(raw: &[u8]) -> Option<(char, usize)> {
    crate::ui::editor::leading_utf8(raw)
}

/// Split `CALL` / `CALL-n` → `(BASE, ssid)`.
pub fn parse_call_ssid(call: &str) -> (String, u8) {
    let call = call.trim().to_ascii_uppercase();
    if let Some((base, ssid)) = call.split_once('-') {
        let ssid = ssid.parse::<u8>().unwrap_or(0).min(15);
        let base: String = base
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(6)
            .collect();
        (base, ssid)
    } else {
        let base: String = call
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(6)
            .collect();
        (base, 0)
    }
}

fn address(call: &str) -> Address {
    let (base, ssid) = parse_call_ssid(call);
    Address::from_parts(base, ssid).unwrap_or_default()
}

fn route_entries(via: &[&str]) -> Vec<RouteEntry> {
    via.iter()
        .map(|v| RouteEntry {
            repeater: address(v),
            has_repeated: false,
        })
        .collect()
}

fn encode(frame: LibFrame) -> Vec<u8> {
    frame.to_bytes()
}

fn command_frame(dest: &str, src: &str, via: &[&str], content: FrameContent) -> Vec<u8> {
    encode(LibFrame {
        source: address(src),
        destination: address(dest),
        route: route_entries(via),
        command_or_response: Some(CommandResponse::Command),
        content,
    })
}

fn response_frame(dest: &str, src: &str, via: &[&str], content: FrameContent) -> Vec<u8> {
    encode(LibFrame {
        source: address(src),
        destination: address(dest),
        route: route_entries(via),
        command_or_response: Some(CommandResponse::Response),
        content,
    })
}

/// Unproto path used by the monitor and the beacon.
///
/// `CQ`, `ALL DB0FN`, `APMI03 WIDE1-1 WIDE2-2`, or `APMI03 via WIDE1-1,WIDE2-2`.
/// The first call is the destination. The rest are digipeaters. Words `to` and
/// `via` are skipped. An empty path is `CQ`.
pub fn parse_unproto(raw: &str) -> (String, Vec<String>) {
    let tokens: Vec<String> = raw
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| !s.eq_ignore_ascii_case("to") && !s.eq_ignore_ascii_case("via"))
        .map(|s| s.to_ascii_uppercase())
        .collect();
    if tokens.is_empty() {
        return ("CQ".into(), Vec::new());
    }
    (tokens[0].clone(), tokens[1..].to_vec())
}

/// `to CQ` or `to APMI03 via WIDE1-1,WIDE2-2`.
pub fn unproto_to(dest: &str, vias: &[String]) -> String {
    if vias.is_empty() {
        format!("to {dest}")
    } else {
        format!("to {dest} via {}", vias.join(","))
    }
}

/// Unproto / APRS-style **UI** frame (Monitor and beacon).
pub fn build_ui(dest: &str, src: &str, via: &[&str], info: &[u8]) -> Vec<u8> {
    command_frame(
        dest,
        src,
        via,
        FrameContent::UnnumberedInformation(UnnumberedInformation {
            pid: ProtocolIdentifier::None,
            info: info.to_vec(),
            poll_or_final: false,
        }),
    )
}

/// SABM (connect), poll bit set, no digipeaters.
pub fn build_sabm(dest: &str, src: &str) -> Vec<u8> {
    build_sabm_via(dest, src, &[])
}

/// SABM through `via` (digipeaters, not yet repeated).
pub fn build_sabm_via(dest: &str, src: &str, via: &[&str]) -> Vec<u8> {
    command_frame(
        dest,
        src,
        via,
        FrameContent::SetAsynchronousBalancedMode(SetAsynchronousBalancedMode { poll: true }),
    )
}

/// DISC (disconnect), poll bit set, no digipeaters.
pub fn build_disc(dest: &str, src: &str) -> Vec<u8> {
    build_disc_via(dest, src, &[])
}

/// DISC through the same digipeater path as the link.
pub fn build_disc_via(dest: &str, src: &str, via: &[&str]) -> Vec<u8> {
    command_frame(
        dest,
        src,
        via,
        FrameContent::Disconnect(Disconnect { poll: true }),
    )
}

/// Digipeater path for a reply: reverse of the path the command arrived on.
/// The caller sends that list with every H bit clear, so the return trip is repeated.
pub fn reply_via(incoming: &[String]) -> Vec<String> {
    incoming.iter().rev().cloned().collect()
}

/// `NL0AS,NL1AS`, `NL0AS via NL1AS`, and `NL0AS V NL1AS` → call list.
/// The first call is the destination. The rest are digipeaters, in order.
pub fn split_connect_hops(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| !s.eq_ignore_ascii_case("via") && !s.eq_ignore_ascii_case("v"))
        .map(|s| s.to_ascii_uppercase())
        .collect()
}

/// If our call is the next digipeater that has not repeated, set that H bit
/// and return the frame to retransmit. Source, destination, and order stay.
/// `None` when it is not our turn.
pub fn repeat_if_ours(payload: &[u8], ours: &[String]) -> Option<Vec<u8>> {
    let mut frame = LibFrame::from_bytes(payload).ok()?;
    let idx = frame.route.iter().position(|hop| !hop.has_repeated)?;
    let call = frame.route[idx].repeater.to_string();
    if !ours.iter().any(|c| c.eq_ignore_ascii_case(&call)) {
        return None;
    }
    frame.route[idx].has_repeated = true;
    Some(frame.to_bytes())
}

/// UA response (connect / disconnect ack).
pub fn build_ua(dest: &str, src: &str, final_: bool) -> Vec<u8> {
    build_ua_via(dest, src, final_, &[])
}

/// UA through the reverse digipeater path of the command.
pub fn build_ua_via(dest: &str, src: &str, final_: bool, via: &[&str]) -> Vec<u8> {
    response_frame(
        dest,
        src,
        via,
        FrameContent::UnnumberedAcknowledge(UnnumberedAcknowledge { final_bit: final_ }),
    )
}

/// DM response (busy / refuse connect / unexpected command while disconnected).
pub fn build_dm(dest: &str, src: &str, final_: bool) -> Vec<u8> {
    build_dm_via(dest, src, final_, &[])
}

/// DM through the reverse digipeater path of the command.
pub fn build_dm_via(dest: &str, src: &str, final_: bool, via: &[&str]) -> Vec<u8> {
    response_frame(
        dest,
        src,
        via,
        FrameContent::DisconnectedMode(DisconnectedMode { final_bit: final_ }),
    )
}

/// REJ response — ask the peer to resend from `nr`.
pub fn build_rej(dest: &str, src: &str, nr: u8, final_: bool) -> Vec<u8> {
    build_rej_via(dest, src, nr, final_, &[])
}

/// REJ through the digipeater path of the link.
pub fn build_rej_via(dest: &str, src: &str, nr: u8, final_: bool, via: &[&str]) -> Vec<u8> {
    response_frame(
        dest,
        src,
        via,
        FrameContent::Reject(Reject {
            receive_sequence: nr & 7,
            poll_or_final: final_,
        }),
    )
}

/// Connected-mode **I** frame, no digipeaters.
pub fn build_i(dest: &str, src: &str, ns: u8, nr: u8, poll: bool, info: &[u8]) -> Vec<u8> {
    build_i_via(dest, src, ns, nr, poll, info, &[])
}

/// Connected-mode **I** frame through `via`.
pub fn build_i_via(
    dest: &str,
    src: &str,
    ns: u8,
    nr: u8,
    poll: bool,
    info: &[u8],
    via: &[&str],
) -> Vec<u8> {
    command_frame(
        dest,
        src,
        via,
        FrameContent::Information(Information {
            pid: ProtocolIdentifier::None,
            info: info.to_vec(),
            receive_sequence: nr & 7,
            send_sequence: ns & 7,
            poll,
        }),
    )
}

/// RR — `as_command` + poll = T3 keepalive; response + final = ack / poll reply.
pub fn build_rr(dest: &str, src: &str, nr: u8, poll_or_final: bool, as_command: bool) -> Vec<u8> {
    build_rr_via(dest, src, nr, poll_or_final, as_command, &[])
}

/// RR through the same digipeater path as the link.
pub fn build_rr_via(
    dest: &str,
    src: &str,
    nr: u8,
    poll_or_final: bool,
    as_command: bool,
    via: &[&str],
) -> Vec<u8> {
    let content = FrameContent::ReceiveReady(ReceiveReady {
        receive_sequence: nr & 7,
        poll_or_final,
    });
    if as_command {
        command_frame(dest, src, via, content)
    } else {
        response_frame(dest, src, via, content)
    }
}

fn classify(content: &FrameContent) -> (Ax25FrameType, u8, u8, bool, Vec<u8>) {
    match content {
        FrameContent::UnnumberedInformation(ui) => {
            (Ax25FrameType::Ui, 0, 0, ui.poll_or_final, ui.info.clone())
        }
        FrameContent::SetAsynchronousBalancedMode(s) => {
            (Ax25FrameType::Sabm, 0, 0, s.poll, Vec::new())
        }
        FrameContent::UnnumberedAcknowledge(ua) => {
            (Ax25FrameType::Ua, 0, 0, ua.final_bit, Vec::new())
        }
        FrameContent::Disconnect(d) => (Ax25FrameType::Disc, 0, 0, d.poll, Vec::new()),
        FrameContent::DisconnectedMode(dm) => (Ax25FrameType::Dm, 0, 0, dm.final_bit, Vec::new()),
        FrameContent::ReceiveReady(rr) => (
            Ax25FrameType::Rr,
            0,
            rr.receive_sequence,
            rr.poll_or_final,
            Vec::new(),
        ),
        FrameContent::ReceiveNotReady(rnr) => (
            Ax25FrameType::Rnr,
            0,
            rnr.receive_sequence,
            rnr.poll_or_final,
            Vec::new(),
        ),
        FrameContent::Reject(rej) => (
            Ax25FrameType::Rej,
            0,
            rej.receive_sequence,
            rej.poll_or_final,
            Vec::new(),
        ),
        FrameContent::FrameReject(fr) => (Ax25FrameType::Frmr, 0, 0, fr.final_bit, Vec::new()),
        FrameContent::Information(i) => (
            Ax25FrameType::Info,
            i.send_sequence,
            i.receive_sequence,
            i.poll,
            i.info.clone(),
        ),
        _ => (Ax25FrameType::Other, 0, 0, false, Vec::new()),
    }
}

/// What to do with an I-frame's N(S) against our V(R). Window is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecvI {
    /// N(S) is the one we are waiting for.
    Accept,
    /// They repeated the I-frame we already accepted (our ack was lost).
    Duplicate,
    /// Gap. `expected` is V(R); ask them to resend from there.
    Reject { expected: u8, got: u8 },
}

/// `accepted` is true after this link has taken at least one I-frame.
/// Before that, N(S) = 7 is not a duplicate of a frame we never had.
pub fn classify_recv_i(vr: u8, ns: u8, accepted: bool) -> RecvI {
    let vr = vr & 7;
    let ns = ns & 7;
    if ns == vr {
        RecvI::Accept
    } else if accepted && ns == (vr + 7) & 7 {
        RecvI::Duplicate
    } else {
        RecvI::Reject {
            expected: vr,
            got: ns,
        }
    }
}

/// What a REJ's N(R) means against our next V(S). The frame still out is
/// `(vs + 7) % 8`. Any other N(R) is a frame we no longer have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecvRej {
    /// They want the I-frame that is still outstanding.
    Resend,
    /// N(R) is our next V(S): they already have that frame.
    Ack,
    /// N(R) names a frame this window cannot produce.
    Reset { wanted: u8, outstanding: u8 },
}

pub fn classify_rej(vs_next: u8, rej_nr: u8) -> RecvRej {
    let vs = vs_next & 7;
    let nr = rej_nr & 7;
    let outstanding = (vs + 7) & 7;
    if nr == outstanding {
        RecvRej::Resend
    } else if nr == vs {
        RecvRej::Ack
    } else {
        RecvRej::Reset {
            wanted: nr,
            outstanding,
        }
    }
}

/// Decode a KISS payload (AX.25 without FCS).
pub fn parse_frame(payload: &[u8]) -> Option<Ax25Frame> {
    let f = LibFrame::from_bytes(payload).ok()?;
    let (kind, ns, nr, poll, info) = classify(&f.content);
    let command = !matches!(f.command_or_response, Some(CommandResponse::Response));
    Some(Ax25Frame {
        dest: f.destination.to_string(),
        src: f.source.to_string(),
        via: f.route.iter().map(|r| r.repeater.to_string()).collect(),
        repeated: f.route.iter().map(|r| r.has_repeated).collect(),
        kind,
        ns,
        nr,
        poll,
        command,
        info,
    })
}
