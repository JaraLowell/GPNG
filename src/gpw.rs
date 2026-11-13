//! Sysop password files (`*.GPW`).
//!
//! The file name is the call plus SSID with the hyphen removed
//! (`DB0IZ-9` → `DB0IZ9.GPW`). The station's `NAMES.GP` type picks the
//! method:
//!
//! - `N` (TheNet / BayCom), `D`, and `W`: 1-based character positions.
//! - `F` (FlexNet): five-digit secret and a five-digit seed use the
//!   published product-sum. A longer secret uses character positions
//!   (PC/FlexNet from 3.3g).
//! - `B` (DieBox / FALCon): a 60×27 grid is recognised and not calculated.
//!   A single password string uses character positions.

use std::path::{Path, PathBuf};

use crate::config::normalize_mycall;
use crate::names::StationKind;
use crate::paths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Answer(String),
    /// DieBox / FALCon grid. The mix is not calculated.
    GridNotCalculated,
    Ignore,
}

pub fn file_names(call: &str) -> Vec<String> {
    let Ok(norm) = normalize_mycall(call) else {
        return Vec::new();
    };
    let flat = norm.replace('-', "");
    let mut names = vec![format!("{flat}.GPW")];
    if flat != norm {
        names.push(format!("{norm}.GPW"));
    }
    names
}

pub fn find_file(call: &str) -> Option<PathBuf> {
    for name in file_names(call) {
        let path = paths::data_file(&name);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

pub fn load(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn reply(kind: Option<StationKind>, secret: &str, line: &str) -> Reply {
    let body = challenge_body(line);
    let runs = digit_runs(body);
    let nums: Vec<u32> = runs.iter().filter_map(|r| r.parse().ok()).collect();
    if !looks_like_challenge(body, &runs) {
        return Reply::Ignore;
    }
    if is_diebox_grid(secret) {
        return Reply::GridNotCalculated;
    }
    let flat = flatten(secret);
    if matches!(kind, Some(StationKind::Flex)) && is_five_digits(&flat) {
        if runs.len() == 1 && runs[0].len() == 5 {
            if let Some(ans) = flex_sum(&flat, &runs[0]) {
                return Reply::Answer(ans);
            }
        }
    }
    if nums.len() >= 5 {
        if let Some(ans) = pick(&flat, &nums) {
            return Reply::Answer(ans);
        }
    }
    Reply::Ignore
}

fn flatten(secret: &str) -> String {
    secret
        .chars()
        .filter(|c| *c != '\n' && *c != '\r')
        .collect::<String>()
        .trim()
        .to_string()
}

fn is_five_digits(text: &str) -> bool {
    text.len() == 5 && text.chars().all(|c| c.is_ascii_digit())
}

fn is_diebox_grid(secret: &str) -> bool {
    let lines: Vec<&str> = secret
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines.len() >= 50 && lines.iter().filter(|l| l.chars().count() >= 20).count() >= 50
}

fn digit_runs(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in line.chars() {
        if c.is_ascii_digit() {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn challenge_body(line: &str) -> &str {
    let line = line.trim();
    let mut parts = line.split_whitespace();
    let Some(first) = parts.next() else {
        return line;
    };
    let rest = line[first.len()..].trim_start();
    let first_has_letters = first.chars().any(|c| c.is_ascii_alphabetic());
    let rest_has_letters = rest.chars().any(|c| c.is_ascii_alphabetic());
    if first_has_letters && !rest_has_letters && !rest.is_empty() {
        rest
    } else {
        line
    }
}
fn looks_like_challenge(body: &str, runs: &[String]) -> bool {
    if runs.len() == 1 && runs[0].len() == 5 {
        return true;
    }
    runs.len() >= 5 && !body.chars().any(|c| c.is_ascii_alphabetic())
}

/// 1-based indexes into the secret. Every index must land on a character.
fn pick(secret: &str, indexes: &[u32]) -> Option<String> {
    let chars: Vec<char> = secret.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let mut out = String::new();
    for n in indexes {
        let i = n.checked_sub(1)?;
        out.push(*chars.get(i as usize)?);
    }
    Some(out)
}

/// Old FlexNet: multiply matching digits, then add the products.
fn flex_sum(secret5: &str, seed5: &str) -> Option<String> {
    let secret: Vec<u32> = secret5
        .chars()
        .map(|c| c.to_digit(10))
        .collect::<Option<_>>()?;
    let seed: Vec<u32> = seed5
        .chars()
        .map(|c| c.to_digit(10))
        .collect::<Option<_>>()?;
    if secret.len() != 5 || seed.len() != 5 {
        return None;
    }
    let sum: u32 = secret.iter().zip(seed).map(|(a, b)| a * b).sum();
    Some(sum.to_string())
}
