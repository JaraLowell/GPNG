//! Runtime files live in one `data/` directory.
//!
//! Release folder next to the executable (or the crate, while developing):
//!
//! ```text
//! Config.toml
//! data/           ICONS.GP, *.GPI, MHEARD.json, KISS.INI, WA8DED.INI, sounds
//! data/LOG/       gpng.log (shell), `<base call>.GPL` (journal), PADS/
//! ```
//!
//! Lookups try the current directory, the executable directory, then the
//! crate root, so `cargo run` and a copied release both find the same tree.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

fn push_unique(roots: &mut Vec<PathBuf>, path: PathBuf) {
    if !roots.iter().any(|p| p == &path) {
        roots.push(path);
    }
}

/// Directories that may contain `Config.toml` and `data/`.
pub fn install_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        push_unique(&mut roots, cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            push_unique(&mut roots, dir.to_path_buf());
        }
    }
    push_unique(&mut roots, PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    roots
}

/// Existing `data/` directories, install root first.
pub fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for root in install_roots() {
        let dir = root.join("data");
        if dir.is_dir() {
            push_unique(&mut dirs, dir);
        }
    }
    if dirs.is_empty() {
        dirs.push(PathBuf::from("data"));
    }
    dirs
}

/// Preferred `data/` directory (created files should go here).
pub fn data_dir() -> PathBuf {
    data_dirs()
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from("data"))
}

/// First existing `data/<name>`, or `data/<name>` under [`data_dir`].
///
/// The match ignores ASCII case (`bell.wav` finds `BELL.WAV`). The returned
/// path uses the name stored on disk, so opening it works on a case-sensitive
/// volume too.
pub fn data_file(name: &str) -> PathBuf {
    for dir in data_dirs() {
        if let Some(path) = find_named_file(&dir, name) {
            return path;
        }
    }
    data_dir().join(name)
}

/// File in `dir` whose name matches `name`, ignoring ASCII case.
/// An exact match wins. The path keeps the stored spelling.
pub fn find_named_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let want = OsStr::new(name);
    let mut folded = None;
    for ent in std::fs::read_dir(dir).ok()?.flatten() {
        let path = ent.path();
        if !path.is_file() {
            continue;
        }
        let got = ent.file_name();
        if got == want {
            return Some(path);
        }
        if folded.is_none() && got.eq_ignore_ascii_case(want) {
            folded = Some(path);
        }
    }
    folded
}

/// `relative` as written, or the same path under an install root.
pub fn find_file(relative: &str) -> Option<PathBuf> {
    let rel = PathBuf::from(relative);
    if rel.is_file() {
        return Some(rel);
    }
    for root in install_roots() {
        let path = root.join(&rel);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Create `dir`, and on a case-insensitive disk rename an existing folder so
/// the stored name matches (`log` becomes `LOG`). Classic GP folders are capitals.
pub fn ensure_dir(dir: &Path) -> io::Result<()> {
    if let Some(parent) = dir.parent() {
        if parent.components().next().is_some() && parent != dir {
            if !parent.exists() {
                ensure_dir(parent)?;
            }
        }
    }
    let Some(want) = dir.file_name() else {
        return std::fs::create_dir_all(dir);
    };
    if let Some(parent) = dir.parent().filter(|p| p.exists()) {
        if let Some(actual) = dir_name_on_disk(parent, want) {
            if actual != want {
                let tmp = parent.join(format!(".case-{}", std::process::id()));
                std::fs::rename(parent.join(&actual), &tmp)?;
                std::fs::rename(&tmp, parent.join(want))?;
            }
            return Ok(());
        }
    }
    std::fs::create_dir_all(dir)
}

/// Directory entry whose name matches `want` ignoring case.
fn dir_name_on_disk(parent: &Path, want: &OsStr) -> Option<std::ffi::OsString> {
    let want_s = want.to_string_lossy();
    let mut folded = None;
    for ent in std::fs::read_dir(parent).ok()?.flatten() {
        if !ent.path().is_dir() {
            continue;
        }
        let name = ent.file_name();
        if name == want {
            return Some(name);
        }
        if name.eq_ignore_ascii_case(&*want_s) {
            folded = Some(name);
        }
    }
    folded
}

/// Configured path (`data/KISS.INI` or a bare file name) resolved against
/// the install roots and, by file name, inside `data/`.
pub fn resolve_configured(configured: &str) -> PathBuf {
    if let Some(path) = find_file(configured) {
        return path;
    }
    if let Some(name) = Path::new(configured).file_name() {
        let in_data = data_file(&name.to_string_lossy());
        if in_data.is_file() {
            return in_data;
        }
    }
    PathBuf::from(configured)
}

#[cfg(test)]
mod tests {
    use super::ensure_dir;

    #[test]
    fn a_lowercase_folder_is_renamed_to_the_requested_capitals() {
        let root = std::env::temp_dir().join(format!("gpng-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("log").join("pads")).unwrap();
        ensure_dir(&root.join("LOG")).unwrap();
        ensure_dir(&root.join("LOG").join("PADS")).unwrap();
        let names: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().any(|n| n == "LOG"), "{names:?}");
        let inner: Vec<String> = std::fs::read_dir(root.join("LOG"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(inner.iter().any(|n| n == "PADS"), "{inner:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_lowercase_wav_is_found_by_the_capital_name() {
        let root = std::env::temp_dir().join(format!("gpng-wav-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("bell.wav"), b"RIFF").unwrap();
        let found = super::find_named_file(&root, "BELL.WAV").unwrap();
        assert!(found.is_file());
        assert!(found.file_name().unwrap().eq_ignore_ascii_case("BELL.WAV"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
