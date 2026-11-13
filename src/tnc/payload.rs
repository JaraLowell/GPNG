//! Operator text shared by every modem path.
//!
//! KISS, WA8DED, and TFPCX do not format typed text or files. The UI
//! normalizes newlines to `\n`, cuts that stream into
//! `[buffers].packet_payload` chunks (classic GP default 128), and hands
//! each chunk to the active driver as opaque bytes.

/// `\r\n`, `\n`, and `\r` all become a single `\n`.
pub fn normalize_newlines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// Fill packets up to `max_len` bytes. Newlines stay inside a packet; they
/// do not start a new one. `max_len` is counted in UTF-8 bytes.
pub fn pack_text_payloads(text: &str, max_len: usize) -> Vec<String> {
    let max_len = max_len.max(1);
    if text.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut buf = String::with_capacity(max_len.min(text.len()));
    let mut chars = text.chars().peekable();
    while let Some(mut ch) = chars.next() {
        if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            ch = '\n';
        }
        let n = ch.len_utf8();
        if !buf.is_empty() && buf.len() + n > max_len {
            out.push(std::mem::take(&mut buf));
        }
        buf.push(ch);
        if buf.len() >= max_len {
            out.push(std::mem::take(&mut buf));
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

/// Pull finished rows out of a stream that may be cut in the middle of a line.
///
/// A packet boundary is not a row break. Only `\n`, `\r`, and `\r\n` start
/// the next row. The unfinished tail stays in `pending` for the next packet.
/// A blank row (two breaks in a row) is kept so column art stays aligned.
///
/// DOS packet radio ends a row with CR. A CR as the last byte of a packet is
/// still a finished row — holding it forever left 7Plus `stop_7+.` stuck in
/// the sniffer after a `//Read` part. When that CR might be the first half of
/// CRLF, set `cr_hold` so the next chunk drops a leading LF.
pub fn take_rx_lines(pending: &mut String, chunk: &str, cr_hold: &mut bool) -> Vec<String> {
    let chunk = if *cr_hold {
        *cr_hold = false;
        chunk.strip_prefix('\n').unwrap_or(chunk)
    } else {
        chunk
    };
    pending.push_str(chunk);
    let mut lines = Vec::new();
    loop {
        let bytes = pending.as_bytes();
        let Some(pos) = bytes.iter().position(|&b| b == b'\n' || b == b'\r') else {
            break;
        };
        let line = pending[..pos].to_string();
        let mut next = pos + 1;
        if bytes[pos] == b'\r' {
            if bytes.get(pos + 1) == Some(&b'\n') {
                next = pos + 2;
            } else if pos + 1 == bytes.len() {
                *cr_hold = true;
            }
        }
        pending.drain(..next);
        lines.push(line);
    }
    lines
}

/// Same row rules as [`take_rx_lines`], but on raw octets so 7Plus check bytes
/// (`0xB0 0xB1`) survive as Latin-1 code points.
pub fn take_rx_lines_bytes(pending: &mut Vec<u8>, chunk: &[u8], cr_hold: &mut bool) -> Vec<String> {
    let mut offset = 0;
    if *cr_hold {
        *cr_hold = false;
        if chunk.first() == Some(&b'\n') {
            offset = 1;
        }
    }
    pending.extend_from_slice(&chunk[offset..]);
    let mut lines = Vec::new();
    loop {
        let Some(pos) = pending.iter().position(|&b| b == b'\n' || b == b'\r') else {
            break;
        };
        let line: String = pending[..pos]
            .iter()
            .map(|&b| char::from_u32(u32::from(b)).unwrap_or('?'))
            .collect();
        let mut next = pos + 1;
        if pending[pos] == b'\r' {
            if pending.get(pos + 1) == Some(&b'\n') {
                next = pos + 2;
            } else if pos + 1 == pending.len() {
                *cr_hold = true;
            }
        }
        pending.drain(..next);
        lines.push(line);
    }
    lines
}

/// Last row when the link closes and no newline followed it.
pub fn flush_rx_line(pending: &mut String) -> Option<String> {
    if pending.is_empty() {
        return None;
    }
    let text = std::mem::take(pending);
    let text = text.trim_end_matches(['\r', '\n']);
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// Lines to show for one received payload. A trailing break does not add a
/// blank row.
pub fn payload_lines(text: &str) -> Vec<String> {
    let normalized = normalize_newlines(text);
    let mut parts: Vec<String> = normalized.split('\n').map(str::to_string).collect();
    if parts.last().is_some_and(|s| s.is_empty()) {
        parts.pop();
    }
    if parts.is_empty() {
        parts.push(String::new());
    }
    parts
}
