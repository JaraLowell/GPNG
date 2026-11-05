//! AutoBin / `#BIN#` file transfer — interoperable with DOS GP, SP, THP.
//!
//! See `docs/TRANSFER.md` and DK4NB’s 1994 protocol bulletin.

use super::crc::crc_ccitt;

/// Parsed offer from a peer (or something we are about to send).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinOffer {
    pub length: u64,
    pub filename: Option<String>,
    /// Borland `struct ftime` as 32-bit hex, if present.
    pub ftime_hex: Option<String>,
    /// Whole-file CCITT CRC (decimal in header), if present.
    pub crc: Option<u16>,
    /// `true` if header contained `?` (sender supports resume).
    pub resumable: bool,
    /// Magic used: `#BIN#` or `#BIN_V2#`.
    pub magic: BinMagic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinMagic {
    Bin,
    BinV2,
}

impl BinMagic {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bin => "#BIN#",
            Self::BinV2 => "#BIN_V2#",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinControl {
    Offer(BinOffer),
    Ok {
        filename: Option<String>,
        /// Resume: bytes already held + their CRC.
        resume: Option<(u64, u16)>,
    },
    No(String),
    Abort,
}

/// TX/RX progress for one AutoBin session on a channel.
#[derive(Debug)]
pub enum AutoBinState {
    Idle,
    /// We sent an offer; waiting for `#OK#` / `#NO#`.
    WaitPeerAck {
        offer: BinOffer,
        sent: Vec<u8>,
    },
    /// We are transmitting file bytes.
    Sending {
        offer: BinOffer,
        offset: u64,
        data: Vec<u8>,
    },
    /// Peer offered; we may auto-ACK if `auto_accept`.
    WaitLocalDecision {
        offer: BinOffer,
    },
    /// Receiving raw bytes into `sink`.
    Receiving {
        offer: BinOffer,
        got: u64,
        crc: u16,
        sink: Vec<u8>,
    },
}

impl Default for AutoBinState {
    fn default() -> Self {
        Self::Idle
    }
}

/// Build a basic (length-only) offer line, CRLF optional — wire uses `\r`.
pub fn format_basic_offer(length: u64) -> String {
    format!("#BIN#{length}\r")
}

/// Build an extended offer (GP/SP style) for maximum interoperability.
pub fn format_extended_offer(
    length: u64,
    filename: &str,
    ftime_hex: u32,
    file_crc: u16,
    resumable: bool,
) -> String {
    let name = filename.rsplit(['/', '\\']).next().unwrap_or(filename);
    let q = if resumable { "?" } else { "" };
    format!("#BIN#{length}#|{name}#${ftime_hex:08X}{q}#{}\r", file_crc)
}

pub fn format_ok() -> String {
    "#OK#\r".into()
}

pub fn format_ok_named(name: &str) -> String {
    format!("#OK#{name}\r")
}

pub fn format_ok_resume(name: &str, frag_len: u64, frag_crc: u16) -> String {
    format!("#OK#{name}#{frag_len}#{frag_crc}\r")
}

pub fn format_no(reason: &str) -> String {
    format!("#NO#{reason}\r")
}

pub fn format_abort() -> String {
    "\r#ABORT#\r".into()
}

/// Try to parse a single control line (without requiring trailing `\r`).
pub fn parse_control_line(line: &str) -> Option<BinControl> {
    let line = line.trim_end_matches(['\r', '\n']);

    if line == "#ABORT#" || line.ends_with("#ABORT#") && line.len() <= 9 {
        return Some(BinControl::Abort);
    }
    if let Some(rest) = line.strip_prefix("#NO#") {
        return Some(BinControl::No(rest.to_string()));
    }
    if let Some(rest) = line.strip_prefix("#OK#") {
        return Some(parse_ok_body(rest));
    }

    let (magic, rest) = if let Some(r) = line.strip_prefix("#BIN_V2#") {
        (BinMagic::BinV2, r)
    } else if let Some(r) = line.strip_prefix("#BIN#") {
        (BinMagic::Bin, r)
    } else {
        return None;
    };

    Some(BinControl::Offer(parse_offer_body(magic, rest)?))
}

fn parse_ok_body(rest: &str) -> BinControl {
    if rest.is_empty() {
        return BinControl::Ok {
            filename: None,
            resume: None,
        };
    }
    // #OK#name  or  #OK#name#$len#crc  (resume) — also seen without $
    // Spec: #OK#ssss#$nnnnn#ccccc
    if let Some((name, after)) = rest.split_once("#$") {
        let mut parts = after.splitn(2, '#');
        let len = parts.next().and_then(|s| s.parse().ok());
        let crc = parts.next().and_then(|s| s.parse().ok());
        if let (Some(l), Some(c)) = (len, crc) {
            return BinControl::Ok {
                filename: Some(name.to_string()),
                resume: Some((l, c)),
            };
        }
    }
    // Fallback: #OK#name#len#crc (no $)
    let mut parts = rest.split('#');
    let name = parts.next().unwrap_or("");
    if let (Some(l), Some(c)) = (
        parts.next().and_then(|s| s.parse().ok()),
        parts.next().and_then(|s| s.parse().ok()),
    ) {
        return BinControl::Ok {
            filename: Some(name.to_string()),
            resume: Some((l, c)),
        };
    }
    BinControl::Ok {
        filename: Some(rest.to_string()),
        resume: None,
    }
}

fn parse_offer_body(magic: BinMagic, rest: &str) -> Option<BinOffer> {
    // Forms:
    //   nnnnn
    //   nnnnn#|name#$FFFFFFFF?#crc
    //   nnnnn#|name#$FFFFFFFF#crc   (no resume)
    let mut filename = None;
    let mut ftime_hex = None;
    let mut crc = None;
    let mut resumable = false;

    let length = if let Some((len_s, after)) = rest.split_once("#|") {
        if let Some((name, after_name)) = after.split_once("#$") {
            filename = Some(name.to_string());
            let body = after_name;
            if let Some(qpos) = body.find('?') {
                let (hex_part, rest2) = body.split_at(qpos);
                ftime_hex = Some(hex_part.to_string());
                resumable = true;
                let rest2 = rest2.trim_start_matches('?');
                if let Some(c) = rest2.strip_prefix('#') {
                    crc = c.parse().ok();
                }
            } else if let Some((hex_part, c)) = body.split_once('#') {
                ftime_hex = Some(hex_part.to_string());
                crc = c.parse().ok();
            } else {
                ftime_hex = Some(body.to_string());
            }
        } else {
            filename = Some(after.to_string());
        }
        len_s.parse().ok()?
    } else {
        rest.parse().ok()?
    };

    Some(BinOffer {
        length,
        filename,
        ftime_hex,
        crc,
        resumable,
        magic,
    })
}

impl AutoBinState {
    /// Feed a complete text line (channel ASCII). Returns replies to TX, if any.
    pub fn on_text_line(&mut self, line: &str, auto_accept: bool) -> Vec<String> {
        let Some(ctrl) = parse_control_line(line) else {
            return Vec::new();
        };
        match (&mut *self, ctrl) {
            (AutoBinState::Idle, BinControl::Offer(offer)) => {
                if auto_accept {
                    let reply = if let Some(name) = &offer.filename {
                        format_ok_named(name)
                    } else {
                        format_ok()
                    };
                    *self = AutoBinState::Receiving {
                        offer,
                        got: 0,
                        crc: 0,
                        sink: Vec::new(),
                    };
                    vec![reply]
                } else {
                    *self = AutoBinState::WaitLocalDecision { offer };
                    Vec::new()
                }
            }
            (AutoBinState::WaitPeerAck { offer, sent }, BinControl::Ok { resume, .. }) => {
                let mut offset = 0u64;
                if let Some((frag_len, frag_crc)) = resume {
                    let prefix = &sent[..sent.len().min(frag_len as usize)];
                    if crc_ccitt(prefix) == frag_crc {
                        offset = frag_len;
                    }
                }
                let data = std::mem::take(sent);
                let offer = offer.clone();
                *self = AutoBinState::Sending {
                    offer,
                    offset,
                    data,
                };
                Vec::new()
            }
            (AutoBinState::WaitPeerAck { .. }, BinControl::No(reason)) => {
                tracing::warn!("AutoBin rejected: {reason}");
                *self = AutoBinState::Idle;
                Vec::new()
            }
            (_, BinControl::Abort) => {
                *self = AutoBinState::Idle;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Feed raw RX bytes while in [`Receiving`].
    pub fn on_binary(&mut self, chunk: &[u8]) -> Option<BinReceiveDone> {
        let AutoBinState::Receiving {
            offer,
            got,
            crc,
            sink,
        } = self
        else {
            return None;
        };
        for &b in chunk {
            *crc = super::crc::updcrc(b, *crc);
            sink.push(b);
            *got += 1;
            if *got >= offer.length {
                let expected = offer.crc;
                let actual = *crc;
                let filename = offer.filename.clone();
                let data = std::mem::take(sink);
                *self = AutoBinState::Idle;
                return Some(BinReceiveDone {
                    filename,
                    data,
                    crc_ok: expected.map(|e| e == actual).unwrap_or(true),
                    crc: actual,
                });
            }
        }
        None
    }

    /// Start sending `data` under an extended header.
    pub fn begin_send(&mut self, filename: &str, data: Vec<u8>, resumable: bool) -> String {
        let crc = crc_ccitt(&data);
        let offer = BinOffer {
            length: data.len() as u64,
            filename: Some(filename.to_string()),
            ftime_hex: Some(format!("{:08X}", 0u32)), // caller may fill real ftime
            crc: Some(crc),
            resumable,
            magic: BinMagic::Bin,
        };
        let header = format_extended_offer(offer.length, filename, 0, crc, resumable);
        *self = AutoBinState::WaitPeerAck { offer, sent: data };
        header
    }
}

#[derive(Debug)]
pub struct BinReceiveDone {
    pub filename: Option<String>,
    pub data: Vec<u8>,
    pub crc_ok: bool,
    pub crc: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_extended_header() {
        let line = format_extended_offer(4, "TEST.BIN", 0x1234_5678, 0xABCD, true);
        let ctrl = parse_control_line(line.trim_end_matches('\r')).unwrap();
        match ctrl {
            BinControl::Offer(o) => {
                assert_eq!(o.length, 4);
                assert_eq!(o.filename.as_deref(), Some("TEST.BIN"));
                assert!(o.resumable);
                assert_eq!(o.crc, Some(0xABCD));
            }
            _ => panic!("expected offer"),
        }
    }

    #[test]
    fn auto_receive_with_crc() {
        let data = b"ABCD";
        let crc = crc_ccitt(data);
        let mut st = AutoBinState::Idle;
        let offer_line = format_extended_offer(4, "X.BIN", 0, crc, false);
        let replies = st.on_text_line(offer_line.trim_end_matches('\r'), true);
        assert!(replies[0].starts_with("#OK#"));
        let done = st.on_binary(data).unwrap();
        assert!(done.crc_ok);
        assert_eq!(done.data, data);
    }
}
