//! WA8DED / TFPCX binary hostmode framing.
//!
//! Wire shape from `DOC/WA8DED Page1.txt`:
//! ```text
//!   <channel:u8> <code:u8> <len:u8> <payload[len]>
//! ```

use chrono::Local;
use tracing::debug;

use super::serial::SerialLink;
use super::TncError;

/// WA8DED hostmode response / request codes (subset used by GP).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostCode {
    /// Channel success / ACK (`0x00` in many traces).
    Ok,
    /// Success, message / data follows.
    Data,
    /// Link / channel status report.
    LinkStatus,
    /// Monitor header / monitored frame.
    Monitor,
    /// Catch-all for yet-unmapped codes.
    Other(u8),
}

impl HostCode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0x00 => Self::Ok,
            0x01 => Self::Data,
            0x02 => Self::LinkStatus,
            0x03 => Self::Monitor,
            other => Self::Other(other),
        }
    }

    pub fn as_u8(self) -> u8 {
        match self {
            Self::Ok => 0x00,
            Self::Data => 0x01,
            Self::LinkStatus => 0x02,
            Self::Monitor => 0x03,
            Self::Other(v) => v,
        }
    }
}

/// Encode and write one host→TNC hostmode frame (`code = 0x01` data/command).
pub fn write_host_frame(
    link: &mut SerialLink,
    channel: u8,
    payload: &[u8],
) -> Result<(), TncError> {
    if payload.len() > 255 {
        return Err(TncError::Protocol(
            "hostmode payload exceeds 255 bytes".into(),
        ));
    }
    let mut frame = Vec::with_capacity(3 + payload.len());
    frame.push(channel);
    frame.push(0x01); // host → tnc command/data
    frame.push(payload.len() as u8);
    frame.extend_from_slice(payload);
    link.write_all(&frame)?;
    debug!(channel, len = payload.len(), "tx host frame");
    Ok(())
}

/// Expand GP `DateTime` pseudo-command into `K dd/mm/yy` + `K hh:mm:ss`.
pub fn sync_datetime(link: &mut SerialLink) -> Result<(), TncError> {
    let now = Local::now();
    let date = now.format("K %d/%m/%y").to_string();
    let time = now.format("K %H:%M:%S").to_string();
    write_host_frame(link, 0, date.as_bytes())?;
    write_host_frame(link, 0, time.as_bytes())?;
    Ok(())
}

/// Apply a list of TNCINI/TNCDEI-style command strings as channel-0 frames.
pub fn apply_command_list(link: &mut SerialLink, cmds: &[String]) -> Result<(), TncError> {
    for cmd in cmds {
        if cmd.eq_ignore_ascii_case("DateTime") {
            sync_datetime(link)?;
        } else {
            write_host_frame(link, 0, cmd.as_bytes())?;
        }
    }
    Ok(())
}

/// Hostmode poll command. Drivers enqueue these bytes so the TNC reply is
/// the acknowledgement that releases the next frame.
pub fn poll_command() -> &'static [u8] {
    b"G0"
}

/// One TNC→host frame. Callers decide whether it is traffic or a status reply.
#[derive(Debug, Clone)]
pub struct InboundFrame {
    pub channel: u8,
    pub code: HostCode,
    pub payload: Vec<u8>,
}

/// WA8DED `L` answer: `a b c d e f` (channel) or `a b` (channel 0).
///
/// b is receive frames not yet displayed, c is frames not yet transmitted,
/// d is frames transmitted and not acknowledged, e is retries of the current
/// operation. Tries and unack are different counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkReport {
    pub not_disp: u32,
    pub not_sent: u32,
    pub unack: u32,
    pub tries: u32,
    /// Six numbers (a connected channel). Channel 0 answers with only a and b.
    pub full: bool,
}

pub fn parse_link_status(payload: &[u8]) -> Option<LinkReport> {
    let text = payload_text(payload);
    if text.is_empty() {
        return None;
    }
    let mut nums = Vec::new();
    for word in text.split_whitespace() {
        nums.push(word.parse::<u32>().ok()?);
    }
    match nums.len() {
        2 => Some(LinkReport {
            not_disp: nums[1],
            not_sent: 0,
            unack: 0,
            tries: 0,
            full: false,
        }),
        6 => Some(LinkReport {
            not_disp: nums[1],
            not_sent: nums[2],
            unack: nums[3],
            tries: nums[4],
            full: true,
        }),
        _ => None,
    }
}

/// WA8DED `@B`: free bytes left in the TNC buffer.
pub fn parse_free_buffer(payload: &[u8]) -> Option<u32> {
    let text = payload_text(payload);
    let digits: String = text
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

fn payload_text(payload: &[u8]) -> &str {
    let end = payload
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(payload.len());
    std::str::from_utf8(&payload[..end]).unwrap_or("")
}

/// Extract complete hostmode frames from an accumulating byte stream.
pub fn drain_frames(stream: &mut Vec<u8>) -> Vec<InboundFrame> {
    let mut seen = Vec::new();
    loop {
        if stream.len() < 3 {
            return seen;
        }
        let channel = stream[0];
        let code = HostCode::from_u8(stream[1]);
        let len = stream[2] as usize;
        if stream.len() < 3 + len {
            return seen;
        }
        let payload = stream[3..3 + len].to_vec();
        stream.drain(..3 + len);
        seen.push(InboundFrame {
            channel,
            code,
            payload,
        });
    }
}
