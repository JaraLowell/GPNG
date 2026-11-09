//! Personal mail, `data/MAIL/{CALL}.GPM`.
//!
//! The folder is not a `//CATalog` directory. A linked station can read and
//! delete only the file for its own call. Another call's mail stays put.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::normalize_mycall;
use crate::ui::editor::decode_cp437;

const README: &str = "\
Graphic Packet mail. One {CALL}.GPM per station.\r\n\
\r\n\
On connect the station is told that mail is waiting, then the greeting\r\n\
follows. //MAIL sends the mail. //Read with no file name does too.\r\n\
//SEND <call> stores the next line as mail for that station.\r\n\
//DEL with no file name deletes only the linked station's mail.\r\n\
The file name is the callsign with a numeric SSID removed.\r\n\
This folder is not listed by //CATalog or //Read of USER.\r\n";

/// `data/MAIL`.
pub fn mail_dir() -> PathBuf {
    crate::paths::data_dir().join("MAIL")
}

pub fn ensure_mail_dir(dir: &Path) -> std::io::Result<()> {
    crate::paths::ensure_dir(dir)?;
    let note = dir.join("README.txt");
    if !note.is_file() {
        fs::write(note, README)?;
    }
    Ok(())
}

/// How a `//Read` / `//DEL` argument relates to the station on this channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailAsk {
    /// Empty, or exactly this station's call / `{CALL}.GPM`.
    Own,
    /// Some other `*.GPM`.
    Foreign,
    /// A normal USER file name.
    NotMail,
}

pub fn classify(peer: Option<&str>, args: &str) -> MailAsk {
    let args = args.trim();
    let own = peer.and_then(|call| filename(call).ok());
    if args.is_empty() {
        return if own.is_some() {
            MailAsk::Own
        } else {
            MailAsk::NotMail
        };
    }
    let upper = args.to_ascii_uppercase();
    if let Some(own) = own {
        let stem = own.trim_end_matches(".GPM");
        let asked = upper.strip_suffix(".GPM").unwrap_or(upper.as_str());
        let asked = crate::cipher::base_call(asked);
        if upper == own || upper == stem || asked == stem {
            return MailAsk::Own;
        }
    }
    if upper.ends_with(".GPM") {
        MailAsk::Foreign
    } else {
        MailAsk::NotMail
    }
}

pub fn filename(call: &str) -> Result<String, String> {
    let call = normalize_mycall(call)?;
    // `NL1STK-2` and `NL1STK` are the same mailbox. A non-numeric suffix stays.
    let base = crate::cipher::base_call(&call);
    if base.is_empty() || !base.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!("invalid call '{call}'"));
    }
    Ok(format!("{base}.GPM"))
}

/// True when this call has a mail file. The text itself is not read.
pub fn waiting(dir: &Path, call: &str) -> bool {
    let Ok(path) = file_path(dir, call) else {
        return false;
    };
    path.is_file() && inside(dir, &path)
}

/// Mail text for `call`, macros still unexpanded. `None` when there is no file.
pub fn read_text(dir: &Path, call: &str) -> Option<String> {
    let path = file_path(dir, call).ok()?;
    if !path.is_file() || !inside(dir, &path) {
        return None;
    }
    let raw = fs::read(&path).ok()?;
    let text = if std::str::from_utf8(&raw).is_ok() {
        String::from_utf8(raw).ok()?
    } else {
        decode_cp437(&raw)
    };
    Some(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// Add one message. An existing file gets two blank lines, then the new block.
///
/// ```text
/// FROM to TO at DATE TIME
/// message
/// ```
pub fn append_message(dir: &Path, to: &str, from: &str, at: &str, msg: &str) -> Result<(), String> {
    ensure_mail_dir(dir).map_err(|e| format!("MAIL: {e}"))?;
    let path = file_path(dir, to)?;
    let to_name = filename(to)?.trim_end_matches(".GPM").to_string();
    let from_name = crate::cipher::base_call(from);
    if from_name.is_empty() || !from_name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("mail needs a from call".into());
    }
    let block = format!("{from_name} to {to_name} at {at}\n{msg}\n");
    let existed = path.is_file() && path.metadata().map(|m| m.len() > 0).unwrap_or(false);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("mail: {e}"))?;
    if existed {
        file.write_all(b"\n\n").map_err(|e| format!("mail: {e}"))?;
    }
    file.write_all(block.as_bytes())
        .map_err(|e| format!("mail: {e}"))?;
    Ok(())
}

/// Remove this station's mail. `Ok(false)` means there was no file.
pub fn delete_own(dir: &Path, call: &str) -> Result<bool, String> {
    let path = file_path(dir, call)?;
    if !path.is_file() {
        return Ok(false);
    }
    if !inside(dir, &path) {
        return Err("mail stays in data/MAIL".into());
    }
    fs::remove_file(&path).map_err(|e| format!("delete mail: {e}"))?;
    Ok(true)
}

fn file_path(dir: &Path, call: &str) -> Result<PathBuf, String> {
    Ok(dir.join(filename(call)?))
}

fn inside(dir: &Path, path: &Path) -> bool {
    let Ok(dir) = dir.canonicalize() else {
        return false;
    };
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    path.starts_with(&dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_linked_call_matches() {
        assert_eq!(classify(Some("nl0msk"), ""), MailAsk::Own);
        assert_eq!(classify(Some("NL0MSK"), "nl0msk.gpm"), MailAsk::Own);
        assert_eq!(classify(Some("NL0MSK"), "NL0MSK"), MailAsk::Own);
        assert_eq!(classify(Some("NL0MSK"), "DL1ABC.GPM"), MailAsk::Foreign);
        assert_eq!(classify(Some("NL0MSK"), "notes.txt"), MailAsk::NotMail);
        assert_eq!(classify(None, ""), MailAsk::NotMail);
        assert_eq!(classify(None, "NL0MSK.GPM"), MailAsk::Foreign);
        assert_eq!(classify(Some("NL1STK-2"), ""), MailAsk::Own);
        assert_eq!(classify(Some("NL1STK-2"), "NL1STK"), MailAsk::Own);
        assert_eq!(classify(Some("NL1STK-2"), "nl1stk-2.gpm"), MailAsk::Own);
        assert_eq!(filename("NL1STK-2").unwrap(), "NL1STK.GPM");
        assert_eq!(filename("nl1stk").unwrap(), "NL1STK.GPM");
    }

    #[test]
    fn delete_removes_only_that_call() {
        let dir = std::env::temp_dir().join(format!("gpng-mail-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        ensure_mail_dir(&dir).unwrap();
        fs::write(dir.join("NL0MSK.GPM"), b"hello Jara\r\n").unwrap();
        fs::write(dir.join("DL1ABC.GPM"), b"other\r\n").unwrap();
        assert!(read_text(&dir, "nl0msk").unwrap().contains("Jara"));
        assert!(read_text(&dir, "nl0msk-2").unwrap().contains("Jara"));
        assert!(waiting(&dir, "NL0MSK-4"));
        assert!(delete_own(&dir, "NL0MSK").unwrap());
        assert!(read_text(&dir, "NL0MSK").is_none());
        assert!(dir.join("DL1ABC.GPM").is_file());
        assert!(!delete_own(&dir, "NL0MSK").unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_appends_a_second_message_after_two_enters() {
        let dir = std::env::temp_dir().join(format!("gpng-mail-send-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        append_message(&dir, "NL1STK-2", "pd4mv-9", "09-11-2026 13:59:00", "hello").unwrap();
        append_message(&dir, "nl1stk", "PD4MV", "09-11-2026 14:01:00", "again").unwrap();
        let text = fs::read_to_string(dir.join("NL1STK.GPM")).unwrap();
        assert_eq!(
            text,
            "PD4MV to NL1STK at 09-11-2026 13:59:00\nhello\n\n\nPD4MV to NL1STK at 09-11-2026 14:01:00\nagain\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
