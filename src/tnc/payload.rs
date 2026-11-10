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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newlines_collapse_to_lf() {
        assert_eq!(normalize_newlines("a\r\nb\nc\rd"), "a\nb\nc\nd");
    }

    #[test]
    fn packs_across_lines_up_to_max() {
        let text = "hello\r\nworld\n!";
        assert_eq!(
            pack_text_payloads(text, 128),
            vec!["hello\nworld\n!".to_string()]
        );
        assert_eq!(
            pack_text_payloads(text, 6),
            vec![
                "hello\n".to_string(),
                "world\n".to_string(),
                "!".to_string()
            ]
        );
    }

    #[test]
    fn long_line_splits_without_waiting_for_newline() {
        let text = "abcdefghij";
        assert_eq!(
            pack_text_payloads(text, 4),
            vec!["abcd".to_string(), "efgh".to_string(), "ij".to_string()]
        );
    }

    #[test]
    fn receiver_joins_a_line_cut_across_packets() {
        let text = "---.ooO.(_).Ooo.------------------  NL NULL MSK  ----------------------------\n    .oO                        Musselkanaal Terminal\nHallo ,\n\nOm volledig van de server faciliteiten van dit station gebruik te maken,\n";
        let chunks = pack_text_payloads(text, 40);
        assert!(chunks.len() > 1);
        let mut pending = String::new();
        let mut cr_hold = false;
        let mut lines = Vec::new();
        for chunk in chunks {
            lines.extend(take_rx_lines(&mut pending, &chunk, &mut cr_hold));
        }
        if let Some(rest) = flush_rx_line(&mut pending) {
            lines.push(rest);
        }
        assert_eq!(
            lines,
            vec![
                "---.ooO.(_).Ooo.------------------  NL NULL MSK  ----------------------------",
                "    .oO                        Musselkanaal Terminal",
                "Hallo ,",
                "",
                "Om volledig van de server faciliteiten van dit station gebruik te maken,",
            ]
        );
    }

    #[test]
    fn receiver_consumes_many_rows_in_one_pass() {
        let mut pending = String::new();
        let mut cr_hold = false;
        let rows = take_rx_lines(&mut pending, "one\r\ntwo\nthree\rfour", &mut cr_hold);
        assert_eq!(rows, ["one", "two", "three"]);
        assert_eq!(pending, "four");
        assert!(!cr_hold);
    }

    #[test]
    fn receiver_trailing_cr_completes_the_row() {
        let mut pending = String::new();
        let mut cr_hold = false;
        assert_eq!(
            take_rx_lines(&mut pending, "first\nsecond\r", &mut cr_hold),
            ["first", "second"]
        );
        assert!(pending.is_empty());
        assert!(cr_hold);
        // CRLF split across packets: leading LF after a held CR is filler.
        assert_eq!(
            take_rx_lines(&mut pending, "\nthird\n", &mut cr_hold),
            ["third"]
        );
        assert!(pending.is_empty());
        assert!(!cr_hold);
    }

    #[test]
    fn receiver_bytes_trailing_cr_closes_stop_7plus() {
        let mut pending = Vec::new();
        let mut cr_hold = false;
        let go = b" go_7+. 001 of 001 README.TXT\r";
        let body = b"payload\r";
        let stop = b" stop_7+. (README.7PL) [0]\r";
        assert!(take_rx_lines_bytes(&mut pending, go, &mut cr_hold)
            .iter()
            .any(|l| l.contains("go_7+")));
        assert!(cr_hold);
        let _ = take_rx_lines_bytes(&mut pending, body, &mut cr_hold);
        let last = take_rx_lines_bytes(&mut pending, stop, &mut cr_hold);
        assert!(
            last.iter().any(|l| l.trim_start().starts_with("stop_7+.")),
            "stop marker must not stick behind a trailing CR: {last:?}"
        );
        assert!(pending.is_empty());
        assert!(cr_hold);
    }

    #[test]
    fn packing_normalizes_newlines_without_splitting_utf8() {
        assert_eq!(pack_text_payloads("é\r\n🙂x", 4), ["é\n", "🙂", "x"]);
    }
}
