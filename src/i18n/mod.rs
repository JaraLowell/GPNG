//! TOML locale packs for UI strings and remote `//Help`.
//!
//! Files live in `data/LOCALES/<iso>.toml` (`en`, `nl`, `de`, …). Missing keys
//! fall back to English. ISO 639-1 codes only.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use toml::Value;

/// Languages offered in Setup and `//Language`.
pub const CODES: &[&str] = &[
    "en", "nl", "de", "fr", "pl", "sl", "cs", "hu", "es", "no", "bg", "pt", "ru",
];

struct LangMeta {
    code: &'static str,
    name: &'static str,
}

const META: &[LangMeta] = &[
    LangMeta {
        code: "en",
        name: "English",
    },
    LangMeta {
        code: "nl",
        name: "Dutch",
    },
    LangMeta {
        code: "de",
        name: "German",
    },
    LangMeta {
        code: "fr",
        name: "French",
    },
    LangMeta {
        code: "pl",
        name: "Polish",
    },
    LangMeta {
        code: "sl",
        name: "Slovenian",
    },
    LangMeta {
        code: "cs",
        name: "Czech",
    },
    LangMeta {
        code: "hu",
        name: "Hungarian",
    },
    LangMeta {
        code: "es",
        name: "Spanish",
    },
    LangMeta {
        code: "no",
        name: "Norwegian",
    },
    LangMeta {
        code: "bg",
        name: "Bulgarian",
    },
    LangMeta {
        code: "pt",
        name: "Portuguese",
    },
    LangMeta {
        code: "ru",
        name: "Russian",
    },
];

#[derive(Default, Clone)]
struct Catalog {
    /// Flat keys: `setup.language`, `help.overview`, `help.commands.READ`.
    strings: HashMap<String, String>,
    /// Min significant letters for help topics (`help.prefix` in `en.toml`).
    prefixes: HashMap<String, usize>,
}

struct State {
    lang: String,
    catalogs: HashMap<String, Catalog>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(State {
            lang: "en".into(),
            catalogs: HashMap::new(),
        })
    })
}

/// Map any legacy DOS token or ISO code to a stored ISO code. Unknown → `en`.
pub fn migrate_language(token: &str) -> String {
    let t = token.trim();
    if t.is_empty() {
        return "en".into();
    }
    if let Some(code) = normalize_language(t) {
        return code.to_string();
    }
    match t.to_ascii_uppercase().as_str() {
        "EN" | "G" | "GB" | "ENG" => "en".into(),
        "NL" | "NED" | "PA" => "nl".into(),
        "DL" | "DE" | "D" | "GER" => "de".into(),
        "F" | "FR" | "FRA" => "fr".into(),
        "PL" | "POL" | "SP" => "pl".into(),
        "SLO" | "SL" | "SVN" => "sl".into(),
        "OK" | "CZ" | "CES" | "CZE" => "cs".into(),
        "HA" | "HG" | "HU" | "HUN" => "hu".into(),
        "E" | "EA" | "ES" | "SPA" => "es".into(),
        "LA" | "NO" | "NOR" => "no".into(),
        "BG" | "LZ" | "BUL" => "bg".into(),
        "PY" | "PT" | "POR" => "pt".into(),
        "RUS" | "RU" | "RA" => "ru".into(),
        _ => "en".into(),
    }
}

/// Accept only modern ISO codes (case-insensitive).
pub fn normalize_language(token: &str) -> Option<&'static str> {
    let t = token.trim();
    META.iter()
        .find(|m| m.code.eq_ignore_ascii_case(t))
        .map(|m| m.code)
}

/// Display name for an ISO code.
pub fn language_name(code: &str) -> &'static str {
    META.iter()
        .find(|m| m.code.eq_ignore_ascii_case(code))
        .map(|m| m.name)
        .unwrap_or("English")
}

/// Set the active UI / help language (ISO). Reloads are lazy.
pub fn set_language(code: &str) {
    let code = migrate_language(code);
    if let Ok(mut st) = state().lock() {
        st.lang = code;
    }
}

pub fn language() -> String {
    state()
        .lock()
        .map(|st| st.lang.clone())
        .unwrap_or_else(|_| "en".into())
}

/// Translate `dotted.key`. Falls back to English, then the key itself.
pub fn t(key: &str) -> String {
    let lang = language();
    lookup(&lang, key).unwrap_or_else(|| key.to_string())
}

fn lookup(lang: &str, key: &str) -> Option<String> {
    let mut st = state().lock().ok()?;
    ensure_loaded(&mut st, lang);
    if let Some(v) = st.catalogs.get(lang).and_then(|c| c.strings.get(key)) {
        if !v.is_empty() {
            return Some(v.clone());
        }
    }
    if !lang.eq_ignore_ascii_case("en") {
        ensure_loaded(&mut st, "en");
        if let Some(v) = st.catalogs.get("en").and_then(|c| c.strings.get(key)) {
            if !v.is_empty() {
                return Some(v.clone());
            }
        }
    }
    None
}

/// Languages whose locale file is on disk (ISO + name).
pub fn language_list() -> String {
    let mut lines = Vec::new();
    for meta in META {
        if find_locale_file(meta.code).is_some() {
            lines.push(format!("{:<4} {}", meta.code, meta.name));
        }
    }
    if lines.is_empty() {
        "No locale files in data/LOCALES/".into()
    } else {
        lines.join("\r")
    }
}

/// `//Language de` → `(code, label)` when the locale exists.
pub fn resolve_language(token: &str) -> Option<(&'static str, &'static str)> {
    let code = normalize_language(token)?;
    if find_locale_file(code).is_none() {
        return None;
    }
    Some((code, language_name(code)))
}

/// Help text for `language`. Empty `topic` is the command overview.
pub fn help_for(language: &str, topic: &str) -> String {
    let lang = migrate_language(language);
    let mut st = match state().lock() {
        Ok(s) => s,
        Err(_) => return "locale state unavailable".into(),
    };
    ensure_loaded(&mut st, &lang);
    ensure_loaded(&mut st, "en");

    let topic = help_topic(topic);
    let overview = help_string(&st, &lang, "help.overview");
    if topic.is_empty() {
        let overview = trim_help_block(&overview);
        if overview.is_empty() {
            "help overview missing".into()
        } else {
            overview.to_string()
        }
    } else if let Some(body) = find_help_section(&st, &lang, topic) {
        trim_help_block(&body).to_string()
    } else {
        format!("No help for {topic}\r{}", trim_help_block(&overview))
    }
}

fn help_string(st: &State, lang: &str, key: &str) -> String {
    if let Some(v) = st.catalogs.get(lang).and_then(|c| c.strings.get(key)) {
        if !v.is_empty() {
            return v.clone();
        }
    }
    st.catalogs
        .get("en")
        .and_then(|c| c.strings.get(key))
        .cloned()
        .unwrap_or_default()
}

fn find_help_section(st: &State, lang: &str, topic: &str) -> Option<String> {
    let topic = topic.to_ascii_uppercase();
    let mut best_tag: Option<String> = None;
    let mut best_len = 0usize;

    let prefixes = st
        .catalogs
        .get("en")
        .map(|c| &c.prefixes)
        .cloned()
        .unwrap_or_default();

    let mut tags: Vec<String> = Vec::new();
    for code in [lang, "en"] {
        if let Some(cat) = st.catalogs.get(code) {
            for key in cat.strings.keys() {
                if let Some(tag) = key.strip_prefix("help.commands.") {
                    if !tags.iter().any(|t| t == tag) {
                        tags.push(tag.to_string());
                    }
                }
            }
        }
    }

    for tag in &tags {
        let min_prefix = prefixes.get(tag).copied().unwrap_or(1).max(1);
        if !tag_matches(&topic, tag, min_prefix) {
            continue;
        }
        if tag.len() >= best_len {
            best_len = tag.len();
            best_tag = Some(tag.clone());
        }
    }

    let tag = best_tag?;
    let key = format!("help.commands.{tag}");
    let body = help_string(st, lang, &key);
    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}

fn tag_matches(topic: &str, tag: &str, min_prefix: usize) -> bool {
    let tag_u = tag.to_ascii_uppercase();
    if topic.len() < min_prefix && topic != tag_u {
        return false;
    }
    tag_u.starts_with(topic) || topic.starts_with(&tag_u)
}

fn help_topic(args: &str) -> &str {
    let mut parts = args.split_whitespace();
    let Some(first) = parts.next() else {
        return "";
    };
    if first.eq_ignore_ascii_case("h") || first.eq_ignore_ascii_case("help") {
        parts.last().unwrap_or(if first.eq_ignore_ascii_case("h") {
            "h"
        } else {
            ""
        })
    } else {
        first
    }
}

fn trim_help_block(s: &str) -> &str {
    let s = s.trim_end_matches(['\r', '\n']);
    let mut start = 0;
    for line in s.split_inclusive(['\n', '\r']) {
        if line
            .chars()
            .all(|c| c == '\n' || c == '\r' || c.is_whitespace())
        {
            start += line.len();
        } else {
            break;
        }
    }
    &s[start..]
}

fn ensure_loaded(st: &mut State, code: &str) {
    let code = migrate_language(code);
    if st.catalogs.contains_key(&code) {
        return;
    }
    let cat = load_catalog(&code).unwrap_or_default();
    st.catalogs.insert(code, cat);
}

fn load_catalog(code: &str) -> Option<Catalog> {
    let path = find_locale_file(code)?;
    let text = fs::read_to_string(&path).ok()?;
    let value: Value = toml::from_str(&text).ok()?;
    let mut cat = Catalog::default();
    flatten(&value, "", &mut cat);
    Some(cat)
}

fn flatten(value: &Value, prefix: &str, cat: &mut Catalog) {
    match value {
        Value::Table(table) => {
            for (k, v) in table {
                let next = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                if next == "help.prefix" {
                    if let Value::Table(prefs) = v {
                        for (tag, n) in prefs {
                            if let Some(n) = n.as_integer() {
                                cat.prefixes.insert(tag.clone(), n.max(1) as usize);
                            }
                        }
                    }
                    continue;
                }
                if next == "meta" {
                    if let Value::Table(meta) = v {
                        if let Some(name) = meta.get("name").and_then(|x| x.as_str()) {
                            cat.strings.insert("meta.name".into(), name.to_string());
                        }
                    }
                    continue;
                }
                flatten(v, &next, cat);
            }
        }
        Value::String(s) => {
            cat.strings.insert(prefix.to_string(), s.clone());
        }
        Value::Integer(n) => {
            cat.strings.insert(prefix.to_string(), n.to_string());
        }
        Value::Boolean(b) => {
            cat.strings.insert(prefix.to_string(), b.to_string());
        }
        Value::Float(f) => {
            cat.strings.insert(prefix.to_string(), f.to_string());
        }
        _ => {}
    }
}

fn find_locale_file(code: &str) -> Option<PathBuf> {
    let file = format!("{code}.toml");
    for dir in locale_dirs() {
        let path = dir.join(&file);
        if path.is_file() {
            return Some(path);
        }
        if let Some(found) = find_ci(&dir, &file) {
            return Some(found);
        }
    }
    None
}

fn locale_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for data in crate::paths::data_dirs() {
        let dir = data.join("LOCALES");
        if dir.is_dir() {
            dirs.push(dir);
            continue;
        }
        // Case-insensitive disk: accept `locales` / `Locales` and keep looking.
        if let Some(found) = find_ci_dir(&data, "LOCALES") {
            dirs.push(found);
        }
    }
    if dirs.is_empty() {
        dirs.push(crate::paths::data_dir().join("LOCALES"));
    }
    dirs
}

fn find_ci_dir(parent: &std::path::Path, name: &str) -> Option<PathBuf> {
    let rd = fs::read_dir(parent).ok()?;
    for ent in rd.flatten() {
        let got = ent.file_name();
        if got.to_string_lossy().eq_ignore_ascii_case(name) && ent.path().is_dir() {
            return Some(ent.path());
        }
    }
    None
}

fn find_ci(dir: &std::path::Path, file: &str) -> Option<PathBuf> {
    let rd = fs::read_dir(dir).ok()?;
    for ent in rd.flatten() {
        let name = ent.file_name();
        if name.to_string_lossy().eq_ignore_ascii_case(file) && ent.path().is_file() {
            return Some(ent.path());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_dos_and_iso() {
        assert_eq!(migrate_language("EN"), "en");
        assert_eq!(migrate_language("G"), "en");
        assert_eq!(migrate_language("DL"), "de");
        assert_eq!(migrate_language("de"), "de");
        assert_eq!(migrate_language("E"), "es");
        assert_eq!(migrate_language("EA"), "es");
        assert_eq!(migrate_language("es"), "es");
        assert_eq!(migrate_language("LA"), "no");
        assert_eq!(migrate_language("BG"), "bg");
        assert_eq!(migrate_language("PY"), "pt");
        assert_eq!(migrate_language("RUS"), "ru");
        assert_eq!(migrate_language("HG"), "hu");
        assert_eq!(migrate_language("nope"), "en");
    }

    #[test]
    fn help_loads_english_and_hash_topic() {
        let overview = help_for("en", "");
        assert!(
            overview.contains("Available Commands"),
            "overview missing: {overview}"
        );
        let hash = help_for("en", "#");
        assert!(hash.contains("//#"), "hash help missing: {hash}");
        let mail = help_for("de", "MAIL");
        assert!(
            mail.contains("//MAIL") || mail.contains("mail"),
            "de should fall back to en MAIL: {mail}"
        );
    }
}
