//! Channel lock. A line wrapped in `Æ` … `¿` is sealed with the two base calls.
//!
//! SSIDs are dropped (`OK1ABC-2` and `OK1ABC` are the same party). The two
//! bases are sorted, so each station derives the same key. Anyone who knows
//! both calls and this layout can read the line; the calls are already on
//! the AX.25 header.
//!
//! The body is the same length as the text. Each printable ASCII byte is
//! rotated in place with a SHA-256 stream. Nothing is added for a nonce or a
//! check, so a short line does not roughly double.
//!
//! ```text
//! wire = "Æ" + shifted(text) + "¿"
//! key  = SHA-256( "GPNG-LOCK-2|" + lo + "|" + hi )
//! ```
//!
//! The markers are the only non-ASCII bytes (UTF-8 `C3 86` and `C2 BF`).
//! A link in extended-ASCII mode shows those bytes as Latin-1 characters;
//! [`open`] accepts that form too.

use sha2::{Digest, Sha256};

const MARK_START: char = 'Æ';
const MARK_END: char = '¿';
/// Extended-ASCII reading of the UTF-8 bytes of `Æ` (`C3 86`).
const MARK_START_EXT: &str = "\u{00C3}\u{0086}";
/// Extended-ASCII reading of the UTF-8 bytes of `¿` (`C2 BF`).
const MARK_END_EXT: &str = "\u{00C2}\u{00BF}";
const KEY_LABEL: &[u8] = b"GPNG-LOCK-2|";

/// Printable ASCII, space through `~`. CR and LF stay out of the body.
const ALPHA_LO: u8 = 0x20;
const ALPHA_N: u8 = 95;

/// `OK1ABC-12` → `OK1ABC`. A suffix that is not a numeric SSID stays put.
pub fn base_call(call: &str) -> String {
    let call = call.trim().to_ascii_uppercase();
    match call.split_once('-') {
        Some((base, ssid))
            if !base.is_empty() && !ssid.is_empty() && ssid.chars().all(|c| c.is_ascii_digit()) =>
        {
            base.to_string()
        }
        _ => call,
    }
}

/// Seal `plain` for the pair of calls. The result starts with `Æ` and ends with `¿`.
pub fn seal(call_a: &str, call_b: &str, plain: &str) -> String {
    let key = pair_key(call_a, call_b);
    let body = shift(&key, plain.as_bytes(), false);
    format!("{MARK_START}{}{MARK_END}", String::from_utf8_lossy(&body))
}

/// Recover the text when `line` is a sealed pair for these two calls.
pub fn open(call_a: &str, call_b: &str, line: &str) -> Option<String> {
    let body = sealed_body(line)?;
    let key = pair_key(call_a, call_b);
    let plain = shift(&key, body.as_bytes(), true);
    String::from_utf8(plain).ok()
}

fn pair_key(call_a: &str, call_b: &str) -> [u8; 32] {
    let a = base_call(call_a);
    let b = base_call(call_b);
    let (lo, hi) = if a <= b { (&a, &b) } else { (&b, &a) };
    let mut hasher = Sha256::new();
    hasher.update(KEY_LABEL);
    hasher.update(lo.as_bytes());
    hasher.update(b"|");
    hasher.update(hi.as_bytes());
    let dig = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&dig);
    out
}

fn shift(key: &[u8; 32], data: &[u8], decrypt: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut block_n = 0u32;
    let mut stream = Vec::new();
    for (i, &b) in data.iter().enumerate() {
        if stream.len() == i {
            let mut hasher = Sha256::new();
            hasher.update(key);
            hasher.update(block_n.to_be_bytes());
            stream.extend_from_slice(&hasher.finalize());
            block_n = block_n.wrapping_add(1);
        }
        out.push(rotate(b, stream[i], decrypt));
    }
    out
}

fn rotate(b: u8, stream: u8, decrypt: bool) -> u8 {
    if !(ALPHA_LO..ALPHA_LO + ALPHA_N).contains(&b) {
        return b;
    }
    let pos = b - ALPHA_LO;
    let k = stream % ALPHA_N;
    let npos = if decrypt {
        (pos + ALPHA_N - k) % ALPHA_N
    } else {
        (pos + k) % ALPHA_N
    };
    ALPHA_LO + npos
}

/// Text between the markers, or the same text when the markers were read as
/// separate Latin-1 bytes.
fn sealed_body(line: &str) -> Option<&str> {
    let line = line.trim();
    let body = if let Some(rest) = line.strip_prefix(MARK_START) {
        rest.strip_suffix(MARK_END)
    } else if let Some(rest) = line.strip_prefix(MARK_START_EXT) {
        rest.strip_suffix(MARK_END_EXT)
    } else {
        None
    }?;
    if body.is_empty() {
        return None;
    }
    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssid_and_order_do_not_change_the_key() {
        let plain = "Hello lock";
        let sealed = seal("ok1abc-2", "DL0XYZ-15", plain);
        assert!(sealed.starts_with('Æ') && sealed.ends_with('¿'));
        let body = sealed_body(&sealed).unwrap();
        assert!(body.bytes().all(|b| b.is_ascii()));
        assert_eq!(body.len(), plain.len());
        assert_eq!(open("DL0XYZ", "OK1ABC", &sealed).as_deref(), Some(plain));
        let extended = format!("{MARK_START_EXT}{body}{MARK_END_EXT}");
        assert_eq!(
            open("OK1ABC-4", "dl0xyz-1", &extended).as_deref(),
            Some(plain)
        );
    }

    #[test]
    fn a_short_line_stays_about_the_same_length() {
        let plain = "goed emorgen";
        let sealed = seal("NL1STK", "NL0MSK-1", plain);
        assert_eq!(sealed.chars().count(), plain.chars().count() + 2);
        assert_eq!(open("NL0MSK", "NL1STK", &sealed).as_deref(), Some(plain));
        assert_ne!(sealed_body(&sealed).unwrap(), plain);
    }

    #[test]
    fn a_changed_byte_does_not_return_the_text() {
        let sealed = seal("OK1AAA", "OK1ZZZ", "secret");
        let mut chars: Vec<char> = sealed.chars().collect();
        let mid = chars.len() / 2;
        chars[mid] = if chars[mid] == 'A' { 'B' } else { 'A' };
        let broken: String = chars.into_iter().collect();
        assert_ne!(open("OK1AAA", "OK1ZZZ", &broken).as_deref(), Some("secret"));
    }

    #[test]
    fn plain_chat_is_not_sealed() {
        assert!(open("OK1AAA", "OK1ZZZ", "hello").is_none());
    }

    #[test]
    fn base_call_drops_numeric_ssid_only() {
        assert_eq!(base_call("nl0msk-2"), "NL0MSK");
        assert_eq!(base_call("NL0MSK"), "NL0MSK");
        assert_eq!(base_call("OK1ABC-10"), "OK1ABC");
        assert_eq!(base_call("NODE-X"), "NODE-X");
    }
}
