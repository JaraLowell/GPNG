//! Personal mail, `data/mail/{CALL}.GPM`.
//!
//! The folder is not a `//CATalog` directory. A linked station can read and
//! delete only the file for its own call. Another call's mail stays put.

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::normalize_mycall;
use crate::ui::editor::decode_cp437;

const README: &str = "\
Graphic Packet mail. One {CALL}.GPM per station.\r\n\
\r\n\
On connect, and on //Read with no file name, that station receives its mail.\r\n\
//DEL with no file name deletes only the linked station's mail.\r\n\
This folder is not listed by //CATalog or //Read of USER.\r\n";

/// `data/mail`.
pub fn mail_dir() -> PathBuf {
    crate::paths::data_dir().join("mail")
}

pub fn ensure_mail_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
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
        if upper == own || upper == stem {
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
    Ok(format!("{call}.GPM"))
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

/// Remove this station's mail. `Ok(false)` means there was no file.
pub fn delete_own(dir: &Path, call: &str) -> Result<bool, String> {
    let path = file_path(dir, call)?;
    if !path.is_file() {
        return Ok(false);
    }
    if !inside(dir, &path) {
        return Err("mail stays in data/mail".into());
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
    }

    #[test]
    fn delete_removes_only_that_call() {
        let dir = std::env::temp_dir().join(format!("gpng-mail-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        ensure_mail_dir(&dir).unwrap();
        fs::write(dir.join("NL0MSK.GPM"), b"hello Jara\r\n").unwrap();
        fs::write(dir.join("DL1ABC.GPM"), b"other\r\n").unwrap();
        assert!(read_text(&dir, "nl0msk").unwrap().contains("Jara"));
        assert!(delete_own(&dir, "NL0MSK").unwrap());
        assert!(read_text(&dir, "NL0MSK").is_none());
        assert!(dir.join("DL1ABC.GPM").is_file());
        assert!(!delete_own(&dir, "NL0MSK").unwrap());
        let _ = fs::remove_dir_all(&dir);
    }
}
