//! Minimal AX.25 L2 helpers for KISS TX/RX (no FCS — the TNC adds CRC).
//!
//! Monitor / unproto → **UI** frames (`dest=CQ`, `src=MYCALL` by default).
//! Connected channels → **SABM** / **I** / **RR** / **DISC**.

/// AX.25 PID: no layer 3.
pub const PID_NO_L3: u8 = 0xF0;

/// Parsed control field / frame class.
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
    pub info: Vec<u8>,
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

/// Encode one 7-byte AX.25 address.
///
/// `command`: HDLC C-bit (dest of a command frame, or src of a response).
/// `last`: address-extension bit clear on all but the final address.
pub fn encode_address(call: &str, ssid: u8, command: bool, last: bool) -> [u8; 7] {
    let (base, parsed_ssid) = parse_call_ssid(call);
    let ssid = if call.contains('-') { parsed_ssid } else { ssid };
    let mut out = [0u8; 7];
    let mut chars = base.chars().map(|c| c as u8);
    for i in 0..6 {
        let b = chars.next().unwrap_or(b' ');
        out[i] = b << 1;
    }
    let mut ssid_byte = (ssid & 0x0f) << 1;
    ssid_byte |= 0x60; // reserved bits = 11
    if command {
        ssid_byte |= 0x80;
    }
    if last {
        ssid_byte |= 0x01;
    }
    out[6] = ssid_byte;
    out
}

fn push_addr_path(out: &mut Vec<u8>, dest: &str, src: &str, via: &[&str], cmd: bool) {
    let n = via.len();
    out.extend_from_slice(&encode_address(dest, 0, cmd, false));
    out.extend_from_slice(&encode_address(src, 0, !cmd, n == 0));
    for (i, v) in via.iter().enumerate() {
        out.extend_from_slice(&encode_address(v, 0, false, i + 1 == n));
    }
}

/// Unproto / APRS-style **UI** frame (Monitor → CQ, etc.).
pub fn build_ui(dest: &str, src: &str, via: &[&str], info: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(16 + via.len() * 7 + info.len());
    push_addr_path(&mut f, dest, src, via, true);
    f.push(0x03); // UI
    f.push(PID_NO_L3);
    f.extend_from_slice(info);
    f
}

/// SABM (connect), poll bit set.
pub fn build_sabm(dest: &str, src: &str) -> Vec<u8> {
    let mut f = Vec::with_capacity(16);
    push_addr_path(&mut f, dest, src, &[], true);
    f.push(0x3F); // SABM P=1
    f
}

/// DISC (disconnect), poll bit set.
pub fn build_disc(dest: &str, src: &str) -> Vec<u8> {
    let mut f = Vec::with_capacity(16);
    push_addr_path(&mut f, dest, src, &[], true);
    f.push(0x53); // DISC P=1
    f
}

/// UA response (connect / disconnect ack).
pub fn build_ua(dest: &str, src: &str, final_: bool) -> Vec<u8> {
    let mut f = Vec::with_capacity(16);
    push_addr_path(&mut f, dest, src, &[], false);
    f.push(if final_ { 0x73 } else { 0x63 });
    f
}

/// DM response (busy / refuse connect).
pub fn build_dm(dest: &str, src: &str, final_: bool) -> Vec<u8> {
    let mut f = Vec::with_capacity(16);
    push_addr_path(&mut f, dest, src, &[], false);
    f.push(if final_ { 0x1F } else { 0x0F });
    f
}

/// Connected-mode **I** frame.
pub fn build_i(dest: &str, src: &str, ns: u8, nr: u8, poll: bool, info: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(16 + info.len());
    push_addr_path(&mut f, dest, src, &[], true);
    let mut ctrl = ((nr & 7) << 5) | ((ns & 7) << 1);
    if poll {
        ctrl |= 0x10;
    }
    f.push(ctrl);
    f.push(PID_NO_L3);
    f.extend_from_slice(info);
    f
}

/// RR — `as_command` + poll = T3 keepalive; response + final = ack / poll reply.
pub fn build_rr(dest: &str, src: &str, nr: u8, poll_or_final: bool, as_command: bool) -> Vec<u8> {
    let mut f = Vec::with_capacity(16);
    push_addr_path(&mut f, dest, src, &[], as_command);
    let mut ctrl = ((nr & 7) << 5) | 0x01;
    if poll_or_final {
        ctrl |= 0x10;
    }
    f.push(ctrl);
    f
}

fn decode_address(chunk: &[u8; 7]) -> (String, bool) {
    let mut call = String::new();
    for b in &chunk[..6] {
        let c = (*b >> 1) as char;
        if c != ' ' && c.is_ascii_graphic() {
            call.push(c);
        }
    }
    let ssid = (chunk[6] >> 1) & 0x0f;
    let last = chunk[6] & 0x01 != 0;
    if ssid > 0 {
        call.push_str(&format!("-{ssid}"));
    }
    (call, last)
}

fn classify_control(ctrl: u8) -> (Ax25FrameType, u8, u8, bool) {
    let pf = ctrl & 0x10 != 0;
    if ctrl & 0x01 == 0 {
        // I frame
        let ns = (ctrl >> 1) & 7;
        let nr = (ctrl >> 5) & 7;
        return (Ax25FrameType::Info, ns, nr, pf);
    }
    if ctrl & 0x03 == 0x01 {
        // S frame
        let nr = (ctrl >> 5) & 7;
        let s = (ctrl >> 2) & 3;
        let kind = match s {
            0 => Ax25FrameType::Rr,
            _ => Ax25FrameType::Other,
        };
        return (kind, 0, nr, pf);
    }
    // U frame
    let u = ctrl & 0xEF; // mask P/F
    let kind = match u {
        0x03 => Ax25FrameType::Ui,
        0x2F => Ax25FrameType::Sabm,
        0x63 => Ax25FrameType::Ua,
        0x43 => Ax25FrameType::Disc,
        0x0F => Ax25FrameType::Dm,
        _ => Ax25FrameType::Other,
    };
    (kind, 0, 0, pf)
}

/// Decode a KISS payload (AX.25 without FCS).
pub fn parse_frame(payload: &[u8]) -> Option<Ax25Frame> {
    if payload.len() < 15 {
        return None;
    }
    let mut i = 0usize;
    let mut addrs = Vec::new();
    loop {
        if i + 7 > payload.len() {
            return None;
        }
        let chunk: [u8; 7] = payload[i..i + 7].try_into().ok()?;
        let (call, last) = decode_address(&chunk);
        if call.is_empty() {
            return None;
        }
        addrs.push(call);
        i += 7;
        if last || addrs.len() >= 10 {
            break;
        }
    }
    if addrs.len() < 2 {
        return None;
    }
    if i >= payload.len() {
        return None;
    }
    let ctrl = payload[i];
    i += 1;
    let (kind, ns, nr, poll) = classify_control(ctrl);
    let mut info = Vec::new();
    match kind {
        Ax25FrameType::Ui | Ax25FrameType::Info => {
            if i < payload.len() {
                let _pid = payload[i];
                i += 1;
                if i < payload.len() {
                    info = payload[i..].to_vec();
                }
            }
        }
        _ => {}
    }
    Some(Ax25Frame {
        dest: addrs[0].clone(),
        src: addrs[1].clone(),
        via: addrs[2..].to_vec(),
        kind,
        ns,
        nr,
        poll,
        info,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
