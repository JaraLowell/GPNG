//! AutoBin / `#BIN#` file transfer — interoperable with DOS GP, SP, THP.
//!
//! See `docs/TRANSFER.md` and DK4NB’s 1994 protocol bulletin.

use super::crc::{crc_ccitt, crc_gp};

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
    /// Receiving raw bytes. `store` keeps a copy in `sink`.
    Receiving {
        offer: BinOffer,
        got: u64,
        crc: u16,
        sink: Vec<u8>,
        store: bool,
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

/// GP 1.63 offer: `#BIN#len#|crc#$ftime#path`.
pub fn format_gp_offer(length: u64, crc: u16, ftime: u32, path: &str) -> String {
    format!("#BIN#{length}#|{crc}#${ftime:08X}#{path}\r")
}

/// Borland `struct ftime` packed the way GP prints it (`1B4B5BDA`).
pub fn pack_ftime(year: i32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> u32 {
    let year = (year - 1980).clamp(0, 127) as u32;
    let tsec = (sec / 2) & 31;
    tsec | ((min & 63) << 5)
        | ((hour & 31) << 11)
        | ((day & 31) << 16)
        | ((month & 15) << 21)
        | (year << 25)
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

/// An I-frame that is only the abort, not a piece of the file.
pub fn is_abort_payload(info: &[u8]) -> bool {
    let line: Vec<u8> = info
        .iter()
        .skip_while(|b| matches!(b, b'\r' | b'\n' | b' '))
        .take_while(|b| !matches!(b, b'\r' | b'\n'))
        .copied()
        .collect();
    line == b"#ABORT#"
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
        if let Some((mid, after_dollar)) = after.split_once("#$") {
            if mid.chars().all(|c| c.is_ascii_digit()) && !mid.is_empty() {
                // GP 1.63: #BIN#len#|crc#$ftime#path
                crc = mid.parse().ok();
                if let Some((hex_part, name)) = after_dollar.split_once('#') {
                    ftime_hex = Some(hex_part.to_string());
                    if !name.is_empty() {
                        filename = Some(name.to_string());
                    }
                } else if !after_dollar.is_empty() {
                    ftime_hex = Some(after_dollar.to_string());
                }
            } else {
                filename = Some(mid.to_string());
                let body = after_dollar;
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
                        store: true,
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

    /// Offer held until the operator hits SAVE.
    pub fn pending_offer(&self) -> Option<&BinOffer> {
        match self {
            Self::WaitLocalDecision { offer } => Some(offer),
            _ => None,
        }
    }

    /// SAVE accepted the offer. The next bytes are the file, kept in memory.
    pub fn begin_receive(&mut self) -> bool {
        self.begin_receive_keeping(true)
    }

    /// SAVE accepted the offer. Bytes are counted, not kept.
    /// The caller writes them to a part file and renames it when this finishes.
    pub fn begin_receive_stream(&mut self) -> bool {
        self.begin_receive_keeping(false)
    }

    fn begin_receive_keeping(&mut self, store: bool) -> bool {
        let Self::WaitLocalDecision { offer } = self else {
            return false;
        };
        let offer = offer.clone();
        *self = Self::Receiving {
            offer,
            got: 0,
            crc: 0,
            sink: Vec::new(),
            store,
        };
        true
    }

    /// Bytes still expected, and the file name from the offer.
    pub fn recv_progress(&self) -> Option<(u64, u64, String)> {
        let AutoBinState::Receiving { offer, got, .. } = self else {
            return None;
        };
        let name = offer
            .filename
            .clone()
            .unwrap_or_else(|| "autobin.bin".into());
        Some((*got, offer.length, name))
    }

    /// Feed raw RX bytes while in [`Receiving`].
    ///
    /// Returns how many leading bytes were file data, and the finished file
    /// when the offer length is reached. Bytes after that belong to the next
    /// text line.
    pub fn on_binary(&mut self, chunk: &[u8]) -> (usize, Option<BinReceiveDone>) {
        let AutoBinState::Receiving {
            offer,
            got,
            crc,
            sink,
            store,
        } = self
        else {
            return (0, None);
        };
        let mut i = 0;
        while i < chunk.len() {
            let b = chunk[i];
            *crc = super::crc::updcrc(b, *crc);
            if *store {
                sink.push(b);
            }
            *got += 1;
            i += 1;
            if *got >= offer.length {
                let expected = offer.crc;
                let actual = *crc;
                let filename = offer.filename.clone();
                let data = std::mem::take(sink);
                *self = AutoBinState::Idle;
                return (
                    i,
                    Some(BinReceiveDone {
                        filename,
                        data,
                        crc_ok: expected.map(|e| e == actual).unwrap_or(true),
                        crc: actual,
                    }),
                );
            }
        }
        (i, None)
    }

    pub fn tx_active(&self) -> bool {
        matches!(self, Self::WaitPeerAck { .. } | Self::Sending { .. })
    }

    pub fn tx_waiting(&self) -> bool {
        matches!(self, Self::WaitPeerAck { .. })
    }

    /// Bytes handed to the link, the file length, and the path.
    /// Waiting for `#OK#` reports zero bytes sent.
    pub fn tx_progress(&self) -> Option<(u64, u64, String, bool)> {
        match self {
            Self::WaitPeerAck { offer, .. } => Some((
                0,
                offer.length,
                offer.filename.clone().unwrap_or_else(|| "file".into()),
                true,
            )),
            Self::Sending { offer, offset, .. } => Some((
                *offset,
                offer.length,
                offer.filename.clone().unwrap_or_else(|| "file".into()),
                false,
            )),
            _ => None,
        }
    }

    /// `#OK#` starts the byte send. `#NO#` / `#ABORT#` stops it,
    /// including an abort that arrives after the file bytes have started.
    pub fn on_tx_reply(&mut self, line: &str) -> TxReply {
        if !self.tx_active() {
            return TxReply::Ignore;
        }
        let Some(ctrl) = parse_control_line(line) else {
            return TxReply::Ignore;
        };
        match ctrl {
            BinControl::Ok { .. } => {
                if !matches!(self, Self::WaitPeerAck { .. }) {
                    return TxReply::Ignore;
                }
                let Self::WaitPeerAck { offer, sent } = std::mem::replace(self, Self::Idle) else {
                    return TxReply::Ignore;
                };
                *self = Self::Sending {
                    offer,
                    offset: 0,
                    data: sent,
                };
                TxReply::Start
            }
            BinControl::No(reason) => {
                *self = Self::Idle;
                TxReply::Rejected(reason)
            }
            BinControl::Abort => {
                *self = Self::Idle;
                TxReply::Rejected(String::new())
            }
            BinControl::Offer(_) => TxReply::Ignore,
        }
    }

    /// Next slice of the file. `None` when every byte has been handed over.
    pub fn next_chunk(&mut self, max: usize) -> Option<Vec<u8>> {
        let Self::Sending { offset, data, .. } = self else {
            return None;
        };
        if *offset >= data.len() as u64 {
            return None;
        }
        let start = *offset as usize;
        let end = (start + max.max(1)).min(data.len());
        let chunk = data[start..end].to_vec();
        *offset = end as u64;
        Some(chunk)
    }

    pub fn tx_summary(&self) -> Option<(u64, u16)> {
        let (offer, got) = match self {
            Self::Sending { offer, offset, .. } => (offer, *offset),
            Self::WaitPeerAck { offer, .. } => (offer, 0),
            _ => return None,
        };
        Some((got, offer.crc.unwrap_or(0)))
    }

    /// Start sending `data` with the GP 1.63 header.
    pub fn begin_gp_send(&mut self, path: &str, data: Vec<u8>, ftime: u32) -> String {
        let crc = crc_gp(&data);
        let offer = BinOffer {
            length: data.len() as u64,
            filename: Some(path.to_string()),
            ftime_hex: Some(format!("{ftime:08X}")),
            crc: Some(crc),
            resumable: false,
            magic: BinMagic::Bin,
        };
        let header = format_gp_offer(offer.length, crc, ftime, path);
        *self = AutoBinState::WaitPeerAck { offer, sent: data };
        header
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxReply {
    Ignore,
    Start,
    Rejected(String),
}

#[derive(Debug)]
pub struct BinReceiveDone {
    pub filename: Option<String>,
    pub data: Vec<u8>,
    pub crc_ok: bool,
    pub crc: u16,
}
