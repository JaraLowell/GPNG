//! Remote file commands, jailed to the USER folder.
//!
//! `//CATalog`, `//Read`, `//Write`, and `//DELete` only see files whose
//! real path is inside that directory. `..`, a drive, or a slash is refused.
//! `//CD` cannot point this at the rest of the disk.

use std::fs;
use std::path::{Path, PathBuf};

/// One path segment the remote is allowed to name. No folders.
pub fn safe_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains(':')
        || name.contains('*')
        || name.contains('?')
        || name.contains('"')
        || name.chars().any(|c| c.is_control())
    {
        return Err("use a file name in USER".into());
    }
    let base = Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .ok_or_else(|| "use a file name in USER".to_string())?;
    if base.len() > 64 {
        return Err("file name too long".into());
    }
    Ok(base.to_string())
}

/// The folder note shipped with USER. Remotes do not replace or delete it.
pub fn is_protected(name: &str) -> bool {
    name.eq_ignore_ascii_case("README.txt")
}

/// `{CALL}.GPM` lives in `data/mail`, never in the USER listing.
pub fn is_mail_name(name: &str) -> bool {
    name.trim().to_ascii_uppercase().ends_with(".GPM")
}

fn refuse_mail_name(name: &str) -> Result<(), String> {
    if is_mail_name(name) {
        Err("mail is not in USER".into())
    } else {
        Ok(())
    }
}

/// Bytes of one file in USER.
pub fn read_file(dir: &Path, name: &str) -> Result<Vec<u8>, String> {
    refuse_mail_name(name)?;
    let path = existing_file(dir, name)?;
    fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))
}

/// Replace a USER file. The caller decides whether this remote may write it.
pub fn write_file(dir: &Path, name: &str, text: &str) -> Result<(), String> {
    let name = safe_name(name)?;
    if is_mail_name(&name) {
        return Err("mail is not in USER".into());
    }
    if is_protected(&name) {
        return Err("README.txt stays".into());
    }
    fs::create_dir_all(dir).map_err(|e| format!("USER: {e}"))?;
    let path = dir.join(&name);
    if path.parent() != Some(dir) {
        return Err("USER only".into());
    }
    let mut bytes = Vec::new();
    if !text.is_empty() {
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                bytes.extend_from_slice(b"\r\n");
            }
            bytes.extend_from_slice(line.as_bytes());
        }
        bytes.extend_from_slice(b"\r\n");
    }
    fs::write(&path, bytes).map_err(|e| format!("write {name}: {e}"))?;
    if !inside(dir, &path) {
        let _ = fs::remove_file(&path);
        return Err("USER only".into());
    }
    Ok(())
}

pub fn delete_file(dir: &Path, name: &str) -> Result<(), String> {
    refuse_mail_name(name)?;
    let path = existing_file(dir, name)?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(name);
    if is_protected(name) {
        return Err("README.txt stays".into());
    }
    fs::remove_file(&path).map_err(|e| format!("delete {name}: {e}"))
}

/// `//CATalog` / `//DIR`. `spec` is DOS wildcards split by `;`.
/// The reply names the folder `USER` and never a disk path.
pub fn catalog(dir: &Path, spec: &str) -> String {
    let patterns = patterns(spec);
    if patterns.is_empty() {
        return "USER only".into();
    }
    let mut rows: Vec<(String, u64)> = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for ent in rd.flatten() {
            let path = ent.path();
            if !path.is_file() || !inside(dir, &path) {
                continue;
            }
            let Some(name) = ent.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if is_mail_name(&name) || !patterns.iter().any(|pat| dos_match(&name, pat)) {
                continue;
            }
            let len = ent.metadata().map(|m| m.len()).unwrap_or(0);
            rows.push((name, len));
        }
    }
    rows.sort_by(|a, b| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()));
    let mut out = String::from("USER");
    for (name, len) in &rows {
        out.push('\r');
        out.push_str(&format!("{len:>8}  {name}"));
    }
    out.push('\r');
    out.push_str(&format!("{} file(s)", rows.len()));
    out
}

fn existing_file(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = safe_name(name)?;
    let path = dir.join(&name);
    if !path.is_file() || !inside(dir, &path) {
        return Err(format!("not in USER: {name}"));
    }
    Ok(path)
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

fn patterns(spec: &str) -> Vec<String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return vec!["*".into()];
    }
    spec.split(';')
        .map(str::trim)
        .filter(|s| {
            !s.is_empty()
                && !s.contains("..")
                && !s.contains('/')
                && !s.contains('\\')
                && !s.contains(':')
        })
        .map(str::to_string)
        .collect()
}

fn dos_match(name: &str, pat: &str) -> bool {
    fn rec(name: &[u8], pat: &[u8]) -> bool {
        if pat.is_empty() {
            return name.is_empty();
        }
        if pat[0] == b'*' {
            let mut rest = pat;
            while rest.first() == Some(&b'*') {
                rest = &rest[1..];
            }
            if rest.is_empty() {
                return true;
            }
            for i in 0..=name.len() {
                if rec(&name[i..], rest) {
                    return true;
                }
            }
            return false;
        }
        if name.is_empty() {
            return false;
        }
        let ok = pat[0] == b'?' || pat[0].eq_ignore_ascii_case(&name[0]);
        ok && rec(&name[1..], &pat[1..])
    }
    rec(name.as_bytes(), pat.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cannot_leave_user() {
        assert!(safe_name("notes.txt").is_ok());
        assert!(safe_name(r"..\secret").is_err());
        assert!(safe_name("a/b").is_err());
        assert!(safe_name(r"C:\GP\x").is_err());
        assert!(safe_name("*.txt").is_err());
    }

    #[test]
    fn wildcards_are_dos() {
        assert!(dos_match("Notes.TXT", "*.txt"));
        assert!(dos_match("a.doc", "*.txt;*.doc".split(';').nth(1).unwrap()));
        assert!(!dos_match("a.doc", "*.txt"));
        assert!(dos_match("ab", "a?"));
    }

    #[test]
    fn catalog_stays_in_the_folder() {
        let dir = std::env::temp_dir().join(format!("gpng-user-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("keep.txt"), b"hi").unwrap();
        fs::write(dir.join("skip.doc"), b"no").unwrap();
        fs::write(dir.join("NL0MSK.GPM"), b"secret").unwrap();
        let listed = catalog(&dir, "*");
        assert!(listed.starts_with("USER\r"), "{listed}");
        assert!(listed.contains("keep.txt"), "{listed}");
        assert!(listed.contains("skip.doc"), "{listed}");
        assert!(!listed.contains("GPM"), "{listed}");
        let listed = catalog(&dir, "*.txt");
        assert!(!listed.contains(':'), "{listed}");
        write_file(&dir, "mine.txt", "one\ntwo").unwrap();
        let body = read_file(&dir, "mine.txt").unwrap();
        assert_eq!(body, b"one\r\ntwo\r\n");
        delete_file(&dir, "mine.txt").unwrap();
        assert!(read_file(&dir, "mine.txt").is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
