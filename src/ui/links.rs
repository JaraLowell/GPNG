//! `http://` and `https://` spans in a text pane.
//!
//! A click opens the system browser. eframe 0.29 only honours
//! [`egui::Context::open_url`] in the web build, so each desktop target
//! launches the browser itself.

use std::io::{self, ErrorKind};
#[cfg(not(windows))]
use std::process::{Command, Stdio};

/// One clickable span. `start`/`end` are character indexes into the laid-out
/// pane text (the same indexes a galley cursor uses), not bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextLink {
    pub start: usize,
    pub end: usize,
    pub url: String,
}

/// `http://` and `https://` addresses in `text`. Indexes are characters.
///
/// The scheme is recognised in any case. A trailing `. , ; : ! ?` is left
/// off, and a closing `)` or `]` is left off when it has no partner inside
/// the address. The span covers only the characters that are opened.
pub fn find_urls(text: &str) -> Vec<(usize, usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if let Some(scheme) = scheme_at(&chars, i) {
            let start = i;
            i += scheme;
            while i < chars.len() && is_url_char(chars[i]) {
                i += 1;
            }
            let mut end = i;
            end = trim_tail(&chars[start..end], end);
            if end > start + scheme {
                let url: String = chars[start..end].iter().collect();
                out.push((start, end, url));
            }
            continue;
        }
        i += 1;
    }
    out
}

/// Character index of a link under a galley cursor, including a click on the
/// right half of the last glyph (the cursor then sits on `end`).
pub fn hit_link(links: &[TextLink], index: usize) -> Option<&TextLink> {
    if let Some(link) = links
        .iter()
        .find(|link| index >= link.start && index < link.end)
    {
        return Some(link);
    }
    links
        .iter()
        .find(|link| link.end > link.start && index == link.end)
}

/// Open `url` in the default browser. Only `http://` and `https://`.
pub fn open_in_browser(url: &str) -> io::Result<()> {
    if !is_browser_url(url) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "only http and https links are opened",
        ));
    }
    #[cfg(windows)]
    {
        open_windows(url)
    }
    #[cfg(target_os = "macos")]
    {
        spawn_detached(Command::new("open").arg(url))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        spawn_detached(Command::new("xdg-open").arg(url))
    }
}

fn is_browser_url(url: &str) -> bool {
    let rest = url
        .get(..8)
        .filter(|head| head.eq_ignore_ascii_case("https://"))
        .map(|_| &url[8..])
        .or_else(|| {
            url.get(..7)
                .filter(|head| head.eq_ignore_ascii_case("http://"))
                .map(|_| &url[7..])
        });
    let Some(rest) = rest else {
        return false;
    };
    !rest.is_empty()
        && url.len() <= 2048
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

fn scheme_at(chars: &[char], i: usize) -> Option<usize> {
    if i > 0 && chars[i - 1].is_ascii_alphanumeric() {
        return None;
    }
    for scheme in ["https://", "http://"] {
        let n = scheme.chars().count();
        if chars.len() >= i + n
            && chars[i..i + n]
                .iter()
                .zip(scheme.chars())
                .all(|(got, want)| got.eq_ignore_ascii_case(&want))
        {
            return Some(n);
        }
    }
    None
}

fn is_url_char(c: char) -> bool {
    matches!(
        c,
        'A'..='Z'
            | 'a'..='z'
            | '0'..='9'
            | '-'
            | '.'
            | '_'
            | '~'
            | ':'
            | '/'
            | '?'
            | '#'
            | '['
            | ']'
            | '@'
            | '!'
            | '$'
            | '&'
            | '\''
            | '('
            | ')'
            | '*'
            | '+'
            | ','
            | ';'
            | '='
            | '%'
    ) || (!c.is_ascii() && !c.is_whitespace() && !c.is_control())
}

/// Drop punctuation that is more likely to be the sentence than the address.
fn trim_tail(body: &[char], end: usize) -> usize {
    let start = end - body.len();
    let mut keep = body.len();
    while keep > 0 {
        let c = body[keep - 1];
        let view = &body[..keep];
        let drop = matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"')
            || (c == ')'
                && view.iter().filter(|ch| **ch == '(').count()
                    < view.iter().filter(|ch| **ch == ')').count())
            || (c == ']'
                && view.iter().filter(|ch| **ch == '[').count()
                    < view.iter().filter(|ch| **ch == ']').count());
        if !drop {
            break;
        }
        keep -= 1;
    }
    start + keep
}

#[cfg(windows)]
fn open_windows(url: &str) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let op: Vec<u16> = std::ffi::OsStr::new("open")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let file: Vec<u16> = std::ffi::OsStr::new(url)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let rc = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        )
    };
    if rc > 32 {
        Ok(())
    } else {
        Err(io::Error::new(
            ErrorKind::Other,
            format!("the browser did not open ({rc})"),
        ))
    }
}

#[cfg(windows)]
#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: *mut std::ffi::c_void,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show_cmd: i32,
    ) -> isize;
}

#[cfg(not(windows))]
fn spawn_detached(command: &mut Command) -> io::Result<()> {
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // `open` / `xdg-open` exit as soon as the browser is started. Wait on
    // another thread so the short-lived process is not left as a zombie.
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_sentence_is_one_link() {
        let line = "een versie ouder staat op https://github.com/JaraLowell/GPNG/releases";
        let found = find_urls(line);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].2, "https://github.com/JaraLowell/GPNG/releases");
        let (start, end, url) = &found[0];
        let chars: Vec<char> = line.chars().collect();
        let sliced: String = chars[*start..*end].iter().collect();
        assert_eq!(sliced, *url);
    }

    #[test]
    fn trailers_and_balanced_parens() {
        let line = "see (https://example.com/a). and https://en.wikipedia.org/wiki/Rust_(lang)";
        let found = find_urls(line);
        assert_eq!(
            found
                .iter()
                .map(|(_, _, url)| url.as_str())
                .collect::<Vec<_>>(),
            vec![
                "https://example.com/a",
                "https://en.wikipedia.org/wiki/Rust_(lang)",
            ]
        );
    }

    #[test]
    fn scheme_must_stand_alone() {
        assert!(find_urls("nothttps://example.com").is_empty());
        assert!(find_urls("//github.com/JaraLowell/GPNG").is_empty());
        assert_eq!(find_urls("HTTP://example.com/x").len(), 1);
    }

    #[test]
    fn refuses_other_schemes() {
        assert!(open_in_browser("file:///C:/Windows").is_err());
        assert!(open_in_browser("https://").is_err());
        assert!(open_in_browser("https://bad host").is_err());
    }
}
