//! Runtime files live in one `data/` directory.
//!
//! Release folder next to the executable (or the crate, while developing):
//!
//! ```text
//! Config.toml
//! data/           ICONS.GP, *.GPI, MHEARD.json, KISS.INI, WA8DED.INI, sounds
//! data/log/       gpng.log (shell), `<base call>.GPL` (journal), pads/
//! ```
//!
//! Lookups try the current directory, the executable directory, then the
//! crate root, so `cargo run` and a copied release both find the same tree.

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
pub fn data_file(name: &str) -> PathBuf {
    for dir in data_dirs() {
        let path = dir.join(name);
        if path.is_file() {
            return path;
        }
    }
    data_dir().join(name)
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
