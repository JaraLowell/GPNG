//! AX.25 L2 helpers for the **KISS** driver path (no FCS — the TNC adds CRC).
//!
//! Encoding/decoding uses the [`ax25`] crate. WA8DED/TFPCX keep L2 inside the
//! TNC via hostmode and do **not** use this module.
//!
//! Monitor / unproto → **UI** frames. The path is the beacon address (`CQ`, or
//! `APMI03 WIDE1-1 WIDE2-2`). Source is the active mycall.
//! Connected channels → **SABM** / **I** / **RR** / **DISC**.

use ax25::frame::{
    Address, Ax25Frame as LibFrame, CommandResponse, Disconnect, DisconnectedMode, FrameContent,
    Information, ProtocolIdentifier, ReceiveReady, RouteEntry, SetAsynchronousBalancedMode,
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
    Info,
    Other,
}

/// One decoded AX.25 frame (addresses + control class + info).
#[derive(Debug, Clone)]
pub struct Ax25Frame {
    pub dest: String,
    pub src: String,
    pub via: Vec<String>,
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
        Ax25FrameType::Info => format!("I{nr}{ns}{m} pid F0"),
        Ax25FrameType::Other => format!("???{m}"),
    };
    format!("fm {src} to {dest}{via} ctl {ctl}")
}

/// `fm SRC …` → `fm HH:MM SRC …` (24-hour, already-stamped lines left as-is).
pub fn insert_monitor_time(line: &str, hhmm: &str) -> String {
    let rest = line.trim_start();
    let lead = &line[..line.len() - rest.len()];
    let (head, tail) = if let Some(tail) = rest.strip_prefix("fm ") {
        ("fm ", tail)
    } else if let Some(tail) = rest.strip_prefix("FM ") {
        ("FM ", tail)
    } else {
        return line.to_string();
    };
    if tail
        .split_whitespace()
        .next()
        .is_some_and(is_hhmm)
    {
        return line.to_string();
    }
    format!("{lead}{head}{hhmm} {tail}")
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
    /// Monitor header line for this decoded frame.
    pub fn monitor_line(&self) -> String {
        monitor_ctl_line(
            &self.src,
            &self.dest,
            &self.via,
            self.kind,
            self.ns,
            self.nr,
            self.poll,
            self.command,
        )
    }
}

/// Split I/UI info on `\r`, `\n`, or `\r\n` (DOS TNCs often use CR only).
/// Each payload line is indented with three spaces for the Monitor pad.
pub fn monitor_payload_lines(text: &str) -> Vec<String> {
    let text = text.trim_end_matches(['\r', '\n']);
    if text.is_empty() {
        return Vec::new();
    }
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                out.push(format!("   {}", &text[start..i]));
                i += 1;
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
                start = i;
            }
            b'\n' => {
                out.push(format!("   {}", &text[start..i]));
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    if start <= text.len() {
        out.push(format!("   {}", &text[start..]));
    }
    out
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

/// UA response (connect / disconnect ack).
pub fn build_ua(dest: &str, src: &str, final_: bool) -> Vec<u8> {
    response_frame(
        dest,
        src,
        &[],
        FrameContent::UnnumberedAcknowledge(UnnumberedAcknowledge {
            final_bit: final_,
        }),
    )
}

/// DM response (busy / refuse connect).
pub fn build_dm(dest: &str, src: &str, final_: bool) -> Vec<u8> {
    response_frame(
        dest,
        src,
        &[],
        FrameContent::DisconnectedMode(DisconnectedMode {
            final_bit: final_,
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
        FrameContent::DisconnectedMode(dm) => {
            (Ax25FrameType::Dm, 0, 0, dm.final_bit, Vec::new())
        }
        FrameContent::ReceiveReady(rr) => {
            (Ax25FrameType::Rr, 0, rr.receive_sequence, rr.poll_or_final, Vec::new())
        }
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

/// Decode a KISS payload (AX.25 without FCS).
pub fn parse_frame(payload: &[u8]) -> Option<Ax25Frame> {
    let f = LibFrame::from_bytes(payload).ok()?;
    let (kind, ns, nr, poll, info) = classify(&f.content);
    let command = !matches!(f.command_or_response, Some(CommandResponse::Response));
    Some(Ax25Frame {
        dest: f.destination.to_string(),
        src: f.source.to_string(),
        via: f.route.iter().map(|r| r.repeater.to_string()).collect(),
        kind,
        ns,
        nr,
        poll,
        command,
        info,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aprs_path_is_dest_then_digis() {
        let (dest, vias) = parse_unproto("APMI03 WIDE1-1 WIDE2-2");
        assert_eq!(dest, "APMI03");
        assert_eq!(vias, ["WIDE1-1", "WIDE2-2"]);
        assert_eq!(unproto_to(&dest, &vias), "to APMI03 via WIDE1-1,WIDE2-2");
        let (dest, vias) = parse_unproto("apmi03 via WIDE1-1,WIDE2-2");
        assert_eq!(dest, "APMI03");
        assert_eq!(vias, ["WIDE1-1", "WIDE2-2"]);
        assert_eq!(parse_unproto("  ").0, "CQ");
        let raw = build_ui("APMI03", "NL0MSK", &["WIDE1-1", "WIDE2-2"], b"hi");
        let f = parse_frame(&raw).unwrap();
        assert_eq!(f.dest, "APMI03");
        assert_eq!(f.via, ["WIDE1-1", "WIDE2-2"]);
        assert_eq!(
            monitor_ctl_line("NL0MSK", &f.dest, &f.via, f.kind, 0, 0, false, true),
            "fm NL0MSK to APMI03 via WIDE1-1,WIDE2-2 ctl UI^ pid F0"
        );
    }

    #[test]
    fn ui_cq_roundtrip_addrs() {
        let raw = build_ui("CQ", "NL0MSK", &[], b"hello");
        let f = parse_frame(&raw).expect("parse");
        assert_eq!(f.dest, "CQ");
        assert_eq!(f.src, "NL0MSK");
        assert_eq!(f.kind, Ax25FrameType::Ui);
        assert_eq!(f.info, b"hello");
    }

    #[test]
    fn sabm_disc_classify() {
        let sabm = build_sabm("DB0ABC", "NL0MSK");
        let f = parse_frame(&sabm).unwrap();
        assert_eq!(f.kind, Ax25FrameType::Sabm);
        let disc = build_disc("DB0ABC", "NL0MSK");
        assert_eq!(parse_frame(&disc).unwrap().kind, Ax25FrameType::Disc);
    }

    #[test]
    fn i_frame_seq() {
        let raw = build_i("DB0ABC", "NL0MSK", 2, 5, false, b"x");
        let f = parse_frame(&raw).unwrap();
        assert_eq!(f.kind, Ax25FrameType::Info);
        assert_eq!(f.ns, 2);
        assert_eq!(f.nr, 5);
        assert_eq!(f.info, b"x");
    }

    #[test]
    fn rr_command_vs_response() {
        let cmd = build_rr("DB0ABC", "NL0MSK", 3, true, true);
        let rsp = build_rr("DB0ABC", "NL0MSK", 3, true, false);
        assert_ne!(cmd, rsp);
        let f = parse_frame(&cmd).unwrap();
        assert_eq!(f.kind, Ax25FrameType::Rr);
        assert_eq!(f.nr, 3);
        assert!(f.poll);
    }

    #[test]
    fn ua_dm_bits() {
        let ua = parse_frame(&build_ua("A", "B", true)).unwrap();
        assert_eq!(ua.kind, Ax25FrameType::Ua);
        assert!(ua.poll);
        let dm = parse_frame(&build_dm("A", "B", false)).unwrap();
        assert_eq!(dm.kind, Ax25FrameType::Dm);
        assert!(!dm.poll);
    }

    #[test]
    fn gp_monitor_lines() {
        let sabm = parse_frame(&build_sabm("NL0MSK", "NL0MSK")).unwrap();
        assert_eq!(
            sabm.monitor_line(),
            "fm NL0MSK to NL0MSK ctl SABM+"
        );
        let ua = parse_frame(&build_ua("NL0MSK", "NL0MSK", true)).unwrap();
        assert_eq!(ua.monitor_line(), "fm NL0MSK to NL0MSK ctl UA-");
        let i = parse_frame(&build_i("NL0MSK", "NL0MSK", 0, 0, false, b"hi")).unwrap();
        assert_eq!(
            i.monitor_line(),
            "fm NL0MSK to NL0MSK ctl I00^ pid F0"
        );
        let rr = parse_frame(&build_rr("NL0MSK", "NL0MSK", 1, false, false)).unwrap();
        assert_eq!(rr.monitor_line(), "fm NL0MSK to NL0MSK ctl RR1v");
        let via = build_ui("CQ", "NL0MSK", &["DB0ABC"], b"x");
        let ui = parse_frame(&via).unwrap();
        assert_eq!(
            ui.monitor_line(),
            "fm NL0MSK to CQ via DB0ABC ctl UI^ pid F0"
        );
    }

    #[test]
    fn stamps_hhmm_after_fm() {
        assert_eq!(
            insert_monitor_time("fm NL0MSK-2 to NL0MSK ctl I07^ pid F0", "03:22"),
            "fm 03:22 NL0MSK-2 to NL0MSK ctl I07^ pid F0"
        );
        assert_eq!(
            insert_monitor_time("fm 03:22 NL0MSK-2 to NL0MSK ctl I07^ pid F0", "04:01"),
            "fm 03:22 NL0MSK-2 to NL0MSK ctl I07^ pid F0"
        );
        assert_eq!(insert_monitor_time("   payload", "03:22"), "   payload");
    }

    #[test]
    fn payload_indent_cr_lf() {
        assert_eq!(monitor_payload_lines("hello"), vec!["   hello".to_string()]);
        assert_eq!(
            monitor_payload_lines("a\rb\nc\r\nd\r"),
            vec![
                "   a".to_string(),
                "   b".to_string(),
                "   c".to_string(),
                "   d".to_string(),
            ]
        );
        assert!(monitor_payload_lines("\r\n").is_empty());
    }
}
