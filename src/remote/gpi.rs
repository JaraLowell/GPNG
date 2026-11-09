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

/// `data/NL1STK.GPC` for `NL1STK` or `NL1STK-2`. The numeric SSID is dropped.
///
/// `None` when that file is not in `data/`. The caller then uses the connect
/// text selected on the channel.
pub fn personal_connect_text(call: &str) -> Option<String> {
    let base = crate::cipher::base_call(call);
    if base.is_empty() || !base.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let name = format!("{base}.GPC");
    for dir in crate::paths::data_dirs() {
        if let Some(path) = crate::paths::find_named_file(&dir, &name) {
            return read_file(&path);
        }
    }
    None
}

/// Read `<call>.GPC` from one folder. Tests use this; the program searches `data/`.
pub fn read_personal_connect(dir: &Path, call: &str) -> Option<String> {
    let base = crate::cipher::base_call(call);
    if base.is_empty() || !base.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let path = crate::paths::find_named_file(dir, &format!("{base}.GPC"))?;
    read_file(&path)
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
/// The verb is uppercased, then compared to the stem with no extension:
/// - full stem equality, or
/// - the verb is a prefix of the stem (including a single first letter).
/// Longest matching stem wins. This does not see EXTERNAL programs, so
/// `//D` is not `DOS`.
pub fn match_verb_to_gpi(verb: &str) -> Option<GpiStem> {
    let v = verb.trim().to_ascii_uppercase();
    if v.is_empty() || v == "ORIGIN" {
        return None;
    }
    let mut best: Option<GpiStem> = None;
    for g in list_servable() {
        let stem = g.stem.to_ascii_uppercase();
        if stem == v || stem.starts_with(&v) {
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

/// `//Info`: prefer `<mycall>.GPI` (our station call), else `INFO.GPI`.
///
/// Releases ship `INFO.GPI` as the example — the operator’s call is unknown
/// at pack time. A personal `<call>.GPI` overrides it when present.
///
/// This is not connect text. Connect uses `{peer}.GPC` then `CTEXT.GPI`.
pub fn load_info(ctx: &MacroContext) -> Option<String> {
    let base = crate::cipher::base_call(&ctx.my_call);
    if !base.is_empty() && base.chars().all(|c| c.is_ascii_alphanumeric()) {
        if let Some(text) = resolve_named(&format!("{base}.GPI")).and_then(|p| read_file(&p)) {
            return Some(expand_gpi_text(&text, ctx));
        }
    }
    load_expanded("info", ctx)
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

    #[test]
    fn a_call_file_drops_the_ssid_and_skips_a_missing_one() {
        let dir = std::env::temp_dir().join(format!(
            "gpng-gpc-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("NL1STK.GPC"), "hello %C\n").unwrap();
        let text = read_personal_connect(&dir, "nl1stk-2").unwrap();
        assert!(text.contains("hello"), "{text}");
        assert!(read_personal_connect(&dir, "DL1ABC").is_none());
        assert!(read_personal_connect(&dir, "../NL1STK").is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
