//! WA8DED / TFPCX binary hostmode framing.
//!
//! Wire shape from `DOC/WA8DED Page1.txt`:
//! ```text
//!   <channel:u8> <code:u8> <len:u8> <payload[len]>
//! ```

use chrono::Local;
use tokio::sync::mpsc;
use tracing::debug;

use super::serial::SerialLink;
use super::{TncError, TncEvent};

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
pub fn write_host_frame(link: &mut SerialLink, channel: u8, payload: &[u8]) -> Result<(), TncError> {
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

/// Classic timed poll: host → `G0` on channel 0 (see DOC Page1 “Get 0”).
pub fn poll_g0(link: &mut SerialLink) -> Result<(), TncError> {
    write_host_frame(link, 0, b"G0")
}

/// Extract complete hostmode frames from an accumulating byte stream.
pub fn drain_frames(stream: &mut Vec<u8>, events: &mpsc::Sender<TncEvent>) {
    loop {
        if stream.len() < 3 {
            return;
        }
        let channel = stream[0];
        let code = HostCode::from_u8(stream[1]);
        let len = stream[2] as usize;
        if stream.len() < 3 + len {
            return;
        }
        let payload = stream[3..3 + len].to_vec();
        stream.drain(..3 + len);
        let _ = events.blocking_send(TncEvent::HostFrame {
            channel,
            code,
            payload,
        });
    }
}
