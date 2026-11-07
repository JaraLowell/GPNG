//! Classic `*.GPI` text files — served on `//` remotes and sampled by `%O`.
//!
//! `ORIGIN.GPI` is special: `%O` picks one random non-empty line.
//! Every other `*.GPI` under `data/` can be requested with
//! `//{stem}` or a unique prefix / first letter (case-insensitive).

use std::fs;
use std::path::{Path, PathBuf};

use rand::Rng;

use super::macros::{expand, MacroContext};

/// Directories searched for `*.GPI` (first hit wins per file name).
pub fn gpi_search_dirs() -> Vec<PathBuf> {
    crate::paths::data_dirs()
        .into_iter()
        .filter(|d| d.is_dir())
        .collect()
}

fn resolve_named(file_name: &str) -> Option<PathBuf> {
    let want = file_name.to_ascii_lowercase();
    for dir in gpi_search_dirs() {
        let path = dir.join(file_name);
        if path.is_file() {
            return Some(path);
        }
        // Case-insensitive scan (Windows is CI; Linux needs this).
        if let Ok(rd) = fs::read_dir(&dir) {
            for ent in rd.flatten() {
                let name = ent.file_name();
                let s = name.to_string_lossy();
                if s.eq_ignore_ascii_case(&want) && ent.path().is_file() {
                    return Some(ent.path());
                }
            }
        }
    }
    None
}

/// Load raw file text (CRLF normalized to `\n`).
pub fn read_file(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    Some(raw.replace("\r\n", "\n").replace('\r', "\n"))
}

/// Random non-empty line from `ORIGIN.GPI` for `%O`.
pub fn random_origin_line() -> String {
    let Some(path) = resolve_named("ORIGIN.GPI") else {
        return String::new();
    };
    let Some(text) = read_file(&path) else {
        return String::new();
    };
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    let i = rand::thread_rng().gen_range(0..lines.len());
    lines[i].to_string()
}

/// Whether `NEWS.GPI` exists (for `%I` hint).
pub fn news_available() -> bool {
    resolve_named("NEWS.GPI").is_some()
}

/// Default connect-text path (`CTEXT.GPI`). Classic GP uses **C**onnect text.
///
/// Falls back to a leftover `GTEXT.GPI` so older data folders still work.
pub fn default_ctext_path() -> PathBuf {
    resolve_named("CTEXT.GPI")
        .or_else(|| resolve_named("GTEXT.GPI"))
        .unwrap_or_else(|| crate::paths::data_file("CTEXT.GPI"))
}

/// Old name — prefer [`default_ctext_path`].
pub fn default_gtext_path() -> PathBuf {
    default_ctext_path()
}

#[derive(Debug, Clone)]
pub struct GpiStem {
    pub stem: String,
    pub path: PathBuf,
}

/// All servable GPI files (excludes `ORIGIN.GPI`).
pub fn list_servable() -> Vec<GpiStem> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for dir in gpi_search_dirs() {
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for ent in rd.flatten() {
            let path = ent.path();
            if !path.is_file() {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.to_ascii_lowercase().ends_with(".gpi") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if stem.is_empty() || stem == "origin" {
                continue;
            }
            if seen.insert(stem.clone()) {
                out.push(GpiStem { stem, path });
            }
        }
    }
    out
}

/// Map a remote verb (`INFO`, `NE`, `Q`, `FREQ`, …) onto a GPI stem.
///
/// Match rules (case-insensitive, no extension):
/// - full stem equality, or
/// - verb is a prefix of the stem (incl. single first letter).
/// Longest matching stem wins.
pub fn match_verb_to_gpi(verb: &str) -> Option<GpiStem> {
    let v = verb.trim().to_ascii_lowercase();
    if v.is_empty() || v == "origin" {
        return None;
    }
    let mut best: Option<GpiStem> = None;
    for g in list_servable() {
        if g.stem == v || g.stem.starts_with(&v) {
            let better = match &best {
                None => true,
                Some(b) => g.stem.len() > b.stem.len(),
            };
            if better {
                best = Some(g);
            }
        }
    }
    best
}

/// Canonical stems for known [`super::commands::RemoteAction`] values.
pub fn stem_for_action(action: super::commands::RemoteAction) -> Option<&'static str> {
    use super::commands::RemoteAction::*;
    match action {
        Info => Some("info"),
        News => Some("news"),
        Quit => Some("qrt"),
        _ => None,
    }
}

/// Load + macro-expand a GPI by stem (`info`, `news`, …).
pub fn load_expanded(stem: &str, ctx: &MacroContext) -> Option<String> {
    let want = stem.to_ascii_lowercase();
    let path = list_servable()
        .into_iter()
        .find(|g| g.stem == want)
        .map(|g| g.path)
        .or_else(|| resolve_named(&format!("{stem}.GPI")))?;
    let text = read_file(&path)?;
    Some(expand_gpi_text(&text, ctx))
}

/// Expand macros; resolve `%O` live from ORIGIN.GPI (fresh line per text line).
pub fn expand_gpi_text(text: &str, ctx: &MacroContext) -> String {
    let mut ctx = ctx.clone();
    if ctx.news_hint.is_empty() && news_available() {
        ctx.news_hint = "News available — //NEws".into();
    }
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        if line.to_ascii_lowercase().contains("%o") {
            ctx.origin_line = random_origin_line();
        }
        out.push_str(&expand(line, &ctx));
    }
    out
}

/// Serve GPI for a remote verb, or `None` if no file matches.
pub fn serve_verb(verb: &str, ctx: &MacroContext) -> Option<(String, String)> {
    let g = match_verb_to_gpi(verb)?;
    let text = read_file(&g.path)?;
    Some((g.stem, expand_gpi_text(&text, ctx)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_data_gpi_if_present() {
        let list = list_servable();
        if resolve_named("NEWS.GPI").is_none() {
            return;
        }
        assert!(list.iter().any(|g| g.stem == "news"));
        assert!(!list.iter().any(|g| g.stem == "origin"));
        assert!(match_verb_to_gpi("NE").is_some());
        assert!(match_verb_to_gpi("news").is_some());
        assert!(
            match_verb_to_gpi("i").map(|g| g.stem) == Some("info".into())
                || match_verb_to_gpi("info").is_some()
        );
    }
}
