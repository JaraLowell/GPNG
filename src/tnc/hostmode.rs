//! TNC2/TheFirmware hostmode: typed requests and response-specific framing.

use chrono::Local;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::debug;

use super::serial::SerialLink;
use super::{TncError, TncEvent};

pub trait HostIo {
    fn write_all(&mut self, data: &[u8]) -> Result<(), TncError>;
    fn try_read(&mut self, data: &mut [u8]) -> Result<usize, TncError>;
}

impl HostIo for SerialLink {
    fn write_all(&mut self, data: &[u8]) -> Result<(), TncError> {
        SerialLink::write_all(self, data)
    }

    fn try_read(&mut self, data: &mut [u8]) -> Result<usize, TncError> {
        SerialLink::try_read(self, data)
    }
}

/// WA8DED hostmode response / request codes (subset used by GP).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostCode {
    /// Channel success / ACK (`0x00` in many traces).
    Ok,
    CommandReply,
    Error,
    /// Link / channel status report.
    LinkStatus,
    /// Monitor header / monitored frame.
    Monitor,
    MonitorHeader,
    MonitorData,
    /// Connected information, not a command response.
    Data,
    /// Catch-all for yet-unmapped codes.
    Other(u8),
}

impl HostCode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0x00 => Self::Ok,
            0x01 => Self::CommandReply,
            0x02 => Self::Error,
            0x03 => Self::LinkStatus,
            0x04 => Self::Monitor,
            0x05 => Self::MonitorHeader,
            0x06 => Self::MonitorData,
            0x07 => Self::Data,
            other => Self::Other(other),
        }
    }

    pub fn as_u8(self) -> u8 {
        match self {
            Self::Ok => 0x00,
            Self::CommandReply => 0x01,
            Self::Error => 0x02,
            Self::LinkStatus => 0x03,
            Self::Monitor => 0x04,
            Self::MonitorHeader => 0x05,
            Self::MonitorData => 0x06,
            Self::Data => 0x07,
            Self::Other(v) => v,
        }
    }
}

pub fn write_host_frame(
    link: &mut impl HostIo,
    channel: u8,
    payload: &[u8],
) -> Result<(), TncError> {
    write_request(link, channel, true, payload)
}

pub fn write_host_data(
    link: &mut impl HostIo,
    channel: u8,
    payload: &[u8],
) -> Result<(), TncError> {
    write_request(link, channel, false, payload)
}

fn write_request(
    link: &mut impl HostIo,
    channel: u8,
    command: bool,
    payload: &[u8],
) -> Result<(), TncError> {
    if payload.is_empty() || payload.len() > 256 {
        return Err(TncError::Protocol(
            "hostmode payload must contain 1..=256 bytes".into(),
        ));
    }
    let mut frame = Vec::with_capacity(3 + payload.len());
    frame.push(channel);
    frame.push(u8::from(command));
    frame.push((payload.len() - 1) as u8);
    frame.extend_from_slice(payload);
    link.write_all(&frame)?;
    debug!(channel, len = payload.len(), "tx host frame");
    Ok(())
}

pub fn expand_command(command: &str) -> Vec<String> {
    if command.eq_ignore_ascii_case("DateTime") {
        let now = Local::now();
        vec![
            now.format("K %m/%d/%y").to_string(),
            now.format("K %H:%M:%S").to_string(),
        ]
    } else {
        vec![command.to_string()]
    }
}

/// Apply a list of TNCINI/TNCDEI-style command strings as channel-0 frames.
pub fn apply_command_list(link: &mut SerialLink, cmds: &[String]) -> Result<(), TncError> {
    let mut stream = Vec::new();
    for cmd in cmds {
        for cmd in expand_command(cmd) {
            let reply = exchange(link, &mut stream, 0, true, cmd.as_bytes(), None)?;
            if reply.code == HostCode::Error {
                return Err(TncError::Protocol(format!(
                    "{cmd}: {}",
                    String::from_utf8_lossy(&reply.payload)
                )));
            }
        }
    }
    Ok(())
}

/// Hostmode poll command. Drivers enqueue these bytes so the TNC reply is
/// the acknowledgement that releases the next frame.
pub fn poll_command() -> &'static [u8] {
    b"G"
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

/// Firmware-native `@B` count; TF2.7 reports twice its internal free-buffer count.
pub fn parse_free_buffer(payload: &[u8]) -> Option<u32> {
    payload_text(payload).trim().parse().ok()
}

fn payload_text(payload: &[u8]) -> &str {
    let end = payload
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(payload.len());
    std::str::from_utf8(&payload[..end]).unwrap_or("")
}

/// Extract complete hostmode frames from an accumulating byte stream.
pub fn drain_frames(stream: &mut Vec<u8>) -> Result<Vec<InboundFrame>, TncError> {
    let mut seen = Vec::new();
    while let Some(frame) = take_frame(stream)? {
        seen.push(frame);
    }
    Ok(seen)
}

fn take_frame(stream: &mut Vec<u8>) -> Result<Option<InboundFrame>, TncError> {
    if stream.len() < 2 {
        return Ok(None);
    }
    let code = HostCode::from_u8(stream[1]);
    let (start, end, consumed) = match code {
        HostCode::Ok => (2, 2, 2),
        HostCode::MonitorData | HostCode::Data => {
            let Some(&len) = stream.get(2) else {
                return Ok(None);
            };
            let end = 3 + usize::from(len) + 1;
            if stream.len() < end {
                return Ok(None);
            }
            (3, end, end)
        }
        HostCode::Other(code) => {
            return Err(TncError::Protocol(format!(
                "unknown hostmode response code {code:#04x}"
            )));
        }
        _ => {
            let Some(end) = stream[2..].iter().position(|&b| b == 0) else {
                if stream.len() > 4096 {
                    return Err(TncError::Protocol("unterminated hostmode response".into()));
                }
                return Ok(None);
            };
            (2, end + 2, end + 3)
        }
    };
    let frame = InboundFrame {
        channel: stream[0],
        code,
        payload: stream[start..end].to_vec(),
    };
    stream.drain(..consumed);
    Ok(Some(frame))
}

/// Never replay an ambiguously accepted data block after a timeout.
pub fn exchange(
    link: &mut impl HostIo,
    stream: &mut Vec<u8>,
    channel: u8,
    command: bool,
    payload: &[u8],
    events: Option<&mpsc::Sender<TncEvent>>,
) -> Result<InboundFrame, TncError> {
    if !stream.is_empty() {
        return Err(TncError::Protocol(
            "unexpected bytes before hostmode request".into(),
        ));
    }
    write_request(link, channel, command, payload)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut buf = [0; 512];
    loop {
        if let Some(frame) = take_frame(stream)? {
            if frame.channel != channel {
                return Err(TncError::Protocol(format!(
                    "hostmode reply on channel {}, expected {channel}",
                    frame.channel
                )));
            }
            return Ok(frame);
        }
        if Instant::now() >= deadline {
            return Err(TncError::Protocol(format!(
                "hostmode ch{channel} reply timed out; acceptance unknown, block not replayed"
            )));
        }
        let n = link.try_read(&mut buf)?;
        if n == 0 {
            std::thread::sleep(Duration::from_millis(5));
        } else {
            if let Some(events) = events {
                let _ = events.blocking_send(TncEvent::RawRx(buf[..n].to_vec()));
            }
            stream.extend_from_slice(&buf[..n]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Wire {
        writes: Vec<Vec<u8>>,
        reply: std::collections::VecDeque<u8>,
    }

    impl HostIo for Wire {
        fn write_all(&mut self, data: &[u8]) -> Result<(), TncError> {
            self.writes.push(data.to_vec());
            Ok(())
        }
        fn try_read(&mut self, data: &mut [u8]) -> Result<usize, TncError> {
            if let Some(byte) = self.reply.pop_front() {
                data[0] = byte;
                Ok(1)
            } else {
                Ok(0)
            }
        }
    }

    #[test]
    fn hostmode_requests_match_captures_and_support_binary_channel_ten() {
        let mut wire = Wire::default();
        write_host_frame(&mut wire, 0, b"G0").unwrap();
        write_host_frame(&mut wire, 1, b"C NL1AS NL0PEL").unwrap();
        write_host_data(&mut wire, 10, b"hello\r").unwrap();
        assert_eq!(
            wire.writes,
            [
                b"\x00\x01\x01G0".to_vec(),
                b"\x01\x01\x0dC NL1AS NL0PEL".to_vec(),
                b"\x0a\x00\x05hello\r".to_vec(),
            ]
        );
        let binary: Vec<_> = (0..=255).collect();
        write_host_data(&mut wire, 10, &binary).unwrap();
        assert_eq!(&wire.writes[3][..3], &[10, 0, 255]);
        assert_eq!(&wire.writes[3][3..], binary);
        assert!(write_host_data(&mut wire, 1, &[]).is_err());
        assert!(write_host_data(&mut wire, 1, &[0; 257]).is_err());
    }

    #[test]
    fn hostmode_stream_decodes_captured_text_and_all_binary_response_types() {
        let mut wire = b"\x00\x00\x00\x01IUSC\0\x01\x02INVALID CALL\0\
                \x01\x03*** CONNECTED to N1TEST\0\x00\x04fm N1TEST to CQ ctl RR0v\0\
                \x00\x05fm N1TEST to CQ ctl UI^ pid F0\0"
            .to_vec();
        wire.extend_from_slice(&[0, 6, 3, 0, 0xdb, 0xc0, b'\r']);
        wire.extend_from_slice(&[10, 7, 255]);
        wire.extend(0..=255);
        let mut pending = Vec::new();
        let mut decoded = Vec::new();
        for byte in wire {
            pending.push(byte);
            decoded.extend(drain_frames(&mut pending).unwrap());
        }
        assert!(pending.is_empty());
        assert_eq!(decoded.len(), 8);
        assert_eq!(
            decoded.iter().map(|f| f.code).collect::<Vec<_>>(),
            [
                HostCode::Ok,
                HostCode::CommandReply,
                HostCode::Error,
                HostCode::LinkStatus,
                HostCode::Monitor,
                HostCode::MonitorHeader,
                HostCode::MonitorData,
                HostCode::Data,
            ]
        );
        assert_eq!(decoded[1].payload, b"IUSC");
        assert_eq!(decoded[6].payload, [0, 0xdb, 0xc0, b'\r']);
        assert_eq!(decoded[7].channel, 10);
        assert_eq!(decoded[7].payload, (0..=255).collect::<Vec<u8>>());
        let mut joined = b"\x00\x00\x01\x01OK\0\x01\x07\x00\0".to_vec();
        assert_eq!(drain_frames(&mut joined).unwrap().len(), 3);
        assert!(joined.is_empty());
        assert!(drain_frames(&mut vec![1, 0x80]).is_err());
    }

    #[test]
    fn hostmode_exchange_correlates_channels_and_does_not_replay_ambiguous_timeouts() {
        let mut wire = Wire {
            reply: [2, 0].into(),
            ..Wire::default()
        };
        assert!(exchange(&mut wire, &mut Vec::new(), 1, false, b"data", None).is_err());
        assert_eq!(wire.writes.len(), 1);
        let mut silent = Wire::default();
        let error = exchange(&mut silent, &mut Vec::new(), 1, false, b"data", None).unwrap_err();
        assert!(error.to_string().contains("acceptance unknown"));
        assert_eq!(silent.writes.len(), 1);
    }
}
