//! `*.GPH` remote-help packs. `G.GPH` is English.
//!
//! A file is an overview (after the opening `@@`) and `@TAG,min` sections.
//! `//Help` sends the overview. `//Help READ` sends the `@READ` section.
//! `//Language` picks which pack is served. The files stay in `data/`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::ui::editor::decode_cp437;

/// One language a station can select. Several queries share one file.
struct Lang {
    /// Token accepted by `//Language`.
    query: &'static str,
    /// Value stored in `remote_language`.
    store: &'static str,
    file: &'static str,
    label: &'static str,
}

const LANGUAGES: &[Lang] = &[
    Lang {
        query: "EN",
        store: "EN",
        file: "G.GPH",
        label: "English",
    },
    Lang {
        query: "G",
        store: "EN",
        file: "G.GPH",
        label: "English",
    },
    Lang {
        query: "GB",
        store: "EN",
        file: "G.GPH",
        label: "English",
    },
    Lang {
        query: "NL",
        store: "NL",
        file: "NL.GPH",
        label: "Dutch",
    },
    Lang {
        query: "DL",
        store: "DL",
        file: "DL.GPH",
        label: "German",
    },
    Lang {
        query: "DE",
        store: "DL",
        file: "DL.GPH",
        label: "German",
    },
    Lang {
        query: "D",
        store: "DL",
        file: "DL.GPH",
        label: "German",
    },
    Lang {
        query: "F",
        store: "F",
        file: "F.GPH",
        label: "French",
    },
    Lang {
        query: "FR",
        store: "F",
        file: "F.GPH",
        label: "French",
    },
    Lang {
        query: "PL",
        store: "PL",
        file: "PL.GPH",
        label: "Polish",
    },
    Lang {
        query: "SLO",
        store: "SLO",
        file: "SLO.GPH",
        label: "Slovenian",
    },
    Lang {
        query: "SL",
        store: "SLO",
        file: "SLO.GPH",
        label: "Slovenian",
    },
    Lang {
        query: "OK",
        store: "OK",
        file: "OK.GPH",
        label: "Czech",
    },
    Lang {
        query: "CZ",
        store: "OK",
        file: "OK.GPH",
        label: "Czech",
    },
    Lang {
        query: "HA",
        store: "HA",
        file: "HA.GPH",
        label: "Hungarian",
    },
    Lang {
        query: "HU",
        store: "HA",
        file: "HA.GPH",
        label: "Hungarian",
    },
];

#[derive(Debug, Clone)]
struct Section {
    tag: String,
    min_prefix: usize,
    body: String,
}

#[derive(Debug, Clone)]
struct HelpPack {
    overview: String,
    sections: Vec<Section>,
}

/// Languages whose `*.GPH` is actually in `data/`, one line each.
pub fn language_list() -> String {
    let mut seen = Vec::new();
    let mut lines = Vec::new();
    for lang in LANGUAGES {
        if seen.iter().any(|s: &String| s == lang.store) {
            continue;
        }
        if find_file(lang.file).is_none() {
            continue;
        }
        seen.push(lang.store.to_string());
        lines.push(format!("{:<4} {}", lang.store, lang.label));
    }
    if lines.is_empty() {
        "No *.GPH language files in data/".into()
    } else {
        lines.join("\r")
    }
}

/// `//Language NL` → the code to store, or `None` when the token is unknown
/// or that pack is not on disk.
pub fn resolve_language(token: &str) -> Option<(&'static str, &'static str)> {
    let token = token.trim();
    let lang = LANGUAGES
        .iter()
        .find(|l| l.query.eq_ignore_ascii_case(token))?;
    if find_file(lang.file).is_none() {
        return None;
    }
    Some((lang.store, lang.label))
}

/// Help text for the station language. Empty `topic` is the command list.
pub fn help_for(language: &str, topic: &str) -> String {
    let file = file_for(language);
    let Some(path) = find_file(file) else {
        return format!("{file} not found");
    };
    let Some(text) = read_pack(&path) else {
        return format!("cannot read {file}");
    };
    let pack = parse(&text);
    let topic = help_topic(topic);
    if topic.is_empty() {
        let overview = trim_help_block(&pack.overview);
        if overview.is_empty() {
            format!("{file} has no overview")
        } else {
            overview.to_string()
        }
    } else if let Some(body) = find_section(&pack, topic) {
        trim_help_block(&body).to_string()
    } else {
        format!("No help for {topic}\r{}", trim_help_block(&pack.overview))
    }
}

/// Drop blank lines at the ends only. Keep the two-space indent on help lines.
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

fn file_for(language: &str) -> &'static str {
    LANGUAGES
        .iter()
        .find(|l| l.store.eq_ignore_ascii_case(language) || l.query.eq_ignore_ascii_case(language))
        .map(|l| l.file)
        .unwrap_or("G.GPH")
}

/// `//H H READ` asks for READ. `//H H` asks for the HELP section.
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

fn find_section(pack: &HelpPack, topic: &str) -> Option<String> {
    let topic = topic.to_ascii_uppercase();
    let mut best: Option<&Section> = None;
    for section in &pack.sections {
        if !tag_matches(&topic, &section.tag, section.min_prefix) {
            continue;
        }
        let better = best.is_none_or(|cur| section.tag.len() > cur.tag.len());
        if better {
            best = Some(section);
        }
    }
    best.map(|s| s.body.clone())
}

fn tag_matches(topic: &str, tag: &str, min_prefix: usize) -> bool {
    if topic.len() < min_prefix && topic != tag {
        return false;
    }
    tag.starts_with(topic) || topic.starts_with(tag)
}

fn parse(text: &str) -> HelpPack {
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0usize;
    while i < lines.len() && !lines[i].trim().starts_with("@@") {
        i += 1;
    }
    if i < lines.len() && lines[i].trim().starts_with("@@") {
        i += 1;
    }
    let mut overview = String::new();
    while i < lines.len() {
        if tag_header(lines[i]).is_some() {
            break;
        }
        overview.push_str(lines[i]);
        overview.push('\n');
        i += 1;
    }
    let mut sections = Vec::new();
    while i < lines.len() {
        let Some((tag, min_prefix)) = tag_header(lines[i]) else {
            break;
        };
        i += 1;
        let mut body = String::new();
        while i < lines.len()
            && tag_header(lines[i]).is_none()
            && !lines[i].trim().starts_with("@@")
        {
            body.push_str(lines[i]);
            body.push('\n');
            i += 1;
        }
        sections.push(Section {
            tag,
            min_prefix,
            body,
        });
    }
    HelpPack { overview, sections }
}

fn tag_header(line: &str) -> Option<(String, usize)> {
    let t = line.trim();
    if !t.starts_with('@') || t.starts_with("@@") {
        return None;
    }
    let rest = &t[1..];
    let (tag, n) = rest.split_once(',').unwrap_or((rest, "1"));
    let tag = tag.trim();
    if tag.is_empty() || !tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '+') {
        return None;
    }
    let min_prefix = n.trim().parse().unwrap_or(1usize).max(1);
    Some((tag.to_ascii_uppercase(), min_prefix))
}

fn find_file(file: &str) -> Option<PathBuf> {
    for dir in crate::paths::data_dirs() {
        let path = dir.join(file);
        if path.is_file() {
            return Some(path);
        }
        if let Some(found) = find_ci(&dir, file) {
            return Some(found);
        }
    }
    None
}

fn find_ci(dir: &Path, file: &str) -> Option<PathBuf> {
    let rd = fs::read_dir(dir).ok()?;
    for ent in rd.flatten() {
        let name = ent.file_name();
        if name.to_string_lossy().eq_ignore_ascii_case(file) && ent.path().is_file() {
            return Some(ent.path());
        }
    }
    None
}

fn read_pack(path: &Path) -> Option<String> {
    let raw = fs::read(path).ok()?;
    let text = if std::str::from_utf8(&raw).is_ok() {
        String::from_utf8(raw).ok()?
    } else {
        decode_cp437(&raw)
    };
    Some(crate::tnc::normalize_newlines(&text))
}
