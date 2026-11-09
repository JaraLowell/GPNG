//! Classic `//` remote-command table (abbreviation-aware).
//!
//! Built from the union of `*.GPH` help packs + `README.TXT` / EXE strings.
//! Matching is case-insensitive; significant-prefix length follows `@TAG,N`.

/// What a recognised remote line should do (semantics filled in over time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteAction {
    CrossChannelText,
    SevenPlusAutoSave,
    Activity,
    Bell,
    Catalog,
    ChangeDir,
    CutTransfer,
    Delete,
    Disconnect,
    Echo,
    EndWrite,
    Help,
    Info,
    Language,
    MHeard,
    Name,
    News,
    Parameters,
    Prompt,
    Quit,
    Read,
    ReadAutoBin,
    RoundTrip,
    Run,
    /// `//SEND <call>` then the next line is mail for that station.
    Send,
    /// `//MAIL` sends this station's mail, or says there is none.
    Mail,
    /// PK-88-style dump of every channel link state (`//CSTATUS`).
    ChannelStatus,
    Usage,
    Version,
    Write,
    WriteAutoBin,
    RemoteConnect,
    ChatMode,
}

/// One catalogue entry.
#[derive(Debug, Clone)]
pub struct RemoteCommand {
    pub canonical: &'static str,
    /// Match keys (uppercase, without `//`), longest first wins.
    pub keys: &'static [&'static str],
    /// Minimum characters the operator must type (from `@TAG,N` when known).
    pub min_prefix: usize,
    pub action: RemoteAction,
    pub summary: &'static str,
}

/// Authoritative list for the modern rewrite.
pub fn catalogue() -> Vec<RemoteCommand> {
    vec![
        cmd(
            "//<n>",
            &[],
            1,
            RemoteAction::CrossChannelText,
            "Send text to channel/port n",
        ),
        cmd(
            "//7+",
            &["7+"],
            2,
            RemoteAction::SevenPlusAutoSave,
            "7Plus AutoSave on/off",
        ),
        cmd(
            "//Activity",
            &["ACTIVITY"],
            2,
            RemoteAction::Activity,
            "Last sysop keyboard/mouse activity",
        ),
        cmd(
            "//Bell",
            &["BELL", "B"],
            1,
            RemoteAction::Bell,
            "Ring the sysop",
        ),
        // "RI" so //R stays //Read. //Ring and //RI still page the sysop.
        cmd("//RIng", &["RING"], 2, RemoteAction::Bell, "Ring the sysop"),
        cmd(
            "//CATalog",
            &["CATALOG", "CAT", "DIRECTORY", "DIR"],
            3,
            RemoteAction::Catalog,
            "List UserDir files",
        ),
        // After CATALOG so //CS / //CST hit CSTATUS, not CAT*. //C stays //C.
        cmd(
            "//CSTATUS",
            &["CSTATUS", "CS"],
            2,
            RemoteAction::ChannelStatus,
            "Status of all channels (PK-88 style)",
        ),
        cmd(
            "//CD",
            &["CD"],
            2,
            RemoteAction::ChangeDir,
            "Change/show UserDir (DL/SLO)",
        ),
        cmd(
            "//CUT",
            &["CUT"],
            3,
            RemoteAction::CutTransfer,
            "Abort BIN or 7+ transfer",
        ),
        cmd(
            "//DELete",
            &["DELETE", "DEL", "KILL"],
            3,
            RemoteAction::Delete,
            "Delete //Write file or clear .GPM",
        ),
        cmd(
            "//DISConnect",
            &["DISCONNECT", "DISC"],
            4,
            RemoteAction::Disconnect,
            "Disconnect without QRT",
        ),
        cmd(
            "//Echo",
            &["ECHO", "E"],
            1,
            RemoteAction::Echo,
            "Echo text (RTT helper)",
        ),
        cmd(
            "//END",
            &["END"],
            3,
            RemoteAction::EndWrite,
            "Finish //Write capture",
        ),
        cmd(
            "//Help",
            &["HELP", "H"],
            1,
            RemoteAction::Help,
            "Send *.GPH help",
        ),
        cmd(
            "//Info",
            &["INFO", "I"],
            1,
            RemoteAction::Info,
            "Station info GPI",
        ),
        cmd(
            "//Language",
            &["LANGUAGE", "L"],
            1,
            RemoteAction::Language,
            "Switch remote language",
        ),
        cmd(
            "//MHeard",
            &["MHEARD", "MH"],
            2,
            RemoteAction::MHeard,
            "MH list / last-heard",
        ),
        cmd(
            "//Name",
            &["NAME", "N"],
            1,
            RemoteAction::Name,
            "Store opposite station name on the MHeard disk list",
        ),
        cmd(
            "//NEws",
            &["NEWS", "NE"],
            2,
            RemoteAction::News,
            "Send NEWS.GPI",
        ),
        // "P" alone = Parameters. //PR / //PRO stay Prompt (shorter-key tie-break).
        cmd(
            "//PARameters",
            &["PARAMETERS", "PARAMETER", "PAR", "P"],
            1,
            RemoteAction::Parameters,
            "Show TNC timing parameters",
        ),
        cmd(
            "//PRompt",
            &["PROMPT", "PR"],
            2,
            RemoteAction::Prompt,
            "Remote prompt on/off",
        ),
        cmd(
            "//Quit",
            &["QUIT", "Q"],
            1,
            RemoteAction::Quit,
            "QRT then disconnect",
        ),
        cmd(
            "//Read",
            &["READ", "R"],
            1,
            RemoteAction::Read,
            "Send UserDir text file",
        ),
        cmd(
            "//RPrg",
            &["RPRG", "RP"],
            2,
            RemoteAction::ReadAutoBin,
            "AutoBin send",
        ),
        cmd(
            "//RTt",
            &["RTT", "RT"],
            2,
            RemoteAction::RoundTrip,
            "Round-trip time",
        ),
        cmd(
            "//RUN",
            &["RUN"],
            3,
            RemoteAction::Run,
            "EXTERNAL / GPRI program",
        ),
        cmd(
            "//SEND",
            &["SEND"],
            4,
            RemoteAction::Send,
            "Leave mail for a station",
        ),
        cmd(
            "//MAIL",
            &["MAIL", "MA"],
            2,
            RemoteAction::Mail,
            "Send this station's mail",
        ),
        cmd(
            "//Usage",
            &["USAGE", "U"],
            1,
            RemoteAction::Usage,
            "Connected stations",
        ),
        cmd(
            "//VERsion",
            &["VERSION", "VER"],
            3,
            RemoteAction::Version,
            "Report version",
        ),
        cmd(
            "//Write",
            &["WRITE", "WRIT", "W"],
            1,
            RemoteAction::Write,
            "Capture text to file",
        ),
        cmd(
            "//WPrg",
            &["WPRG", "WP"],
            2,
            RemoteAction::WriteAutoBin,
            "AutoBin store",
        ),
        cmd(
            "//C",
            &["C"],
            1,
            RemoteAction::RemoteConnect,
            "Remote-initiated connect",
        ),
        cmd(
            "#",
            &["#"],
            1,
            RemoteAction::ChatMode,
            "Chat-mode allow token",
        ),
    ]
}

fn cmd(
    canonical: &'static str,
    keys: &'static [&'static str],
    min_prefix: usize,
    action: RemoteAction,
    summary: &'static str,
) -> RemoteCommand {
    RemoteCommand {
        canonical,
        keys,
        min_prefix,
        action,
        summary,
    }
}

/// A `//` line typed into a live QSO belongs to the other station.
/// This station runs it only when the channel is not linked.
/// `//Bell` is always a page to the other station, never a local ring.
pub fn typed_runs_locally(linked: bool, action: RemoteAction) -> bool {
    !linked && action != RemoteAction::Bell
}

/// Parse a line that may be a remote command → `(action, args)`.
///
/// A leading space means plain text (` //Help` in a help file), not a command.
/// Trailing spaces are ignored. Runs of spaces inside the command collapse to one.
pub fn parse_line(line: &str) -> Option<(RemoteAction, String)> {
    let line = line.trim_end_matches(['\r', '\n']).trim_end();
    if line.is_empty() {
        return None;
    }
    // Indented ` //cmd` is prose, not remote control.
    if line.bytes().next().is_some_and(|b| b.is_ascii_whitespace()) {
        return None;
    }
    if line == "#" {
        return Some((RemoteAction::ChatMode, String::new()));
    }

    let rest = squeeze_spaces(line.strip_prefix("//")?);
    if rest.is_empty() {
        return None;
    }

    // //7+ ON  (must precede digit-channel check — '7' is also a digit)
    if rest.len() >= 2 && rest.as_bytes()[0] == b'7' && rest.as_bytes()[1] == b'+' {
        let args = rest[2..].trim().to_string();
        return Some((RemoteAction::SevenPlusAutoSave, args));
    }

    // //1 hello  — cross-channel text
    if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return Some((RemoteAction::CrossChannelText, rest));
    }

    let (verb, args) = split_verb(&rest);
    let verb_up = verb.to_ascii_uppercase();
    let action = match_verb(&verb_up)?;
    Some((action, args.to_string()))
}

/// Collapse runs of whitespace to a single space and trim ends.
fn squeeze_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut gap = false;
    for c in s.chars() {
        if c.is_whitespace() {
            gap = true;
            continue;
        }
        if gap && !out.is_empty() {
            out.push(' ');
        }
        gap = false;
        out.push(c);
    }
    out
}

fn match_verb(verb_up: &str) -> Option<RemoteAction> {
    // Prefer a longer matched prefix. On a tie (e.g. //PRO → PROMPT vs
    // PARAMETERS), prefer the shorter key so //PR* stays Prompt.
    let mut best: Option<(RemoteAction, usize, usize)> = None;
    for entry in catalogue() {
        for key in entry.keys {
            if verb_matches(verb_up, key, entry.min_prefix) {
                let score = key.len().min(verb_up.len());
                if best.is_none_or(|(_, s, klen)| score > s || (score == s && key.len() < klen))
                {
                    best = Some((entry.action, score, key.len()));
                }
            }
        }
    }
    best.map(|(a, _, _)| a)
}

/// Operator typed `verb`; catalogue key is `key`. Accept abbreviations.
fn verb_matches(verb: &str, key: &str, min_prefix: usize) -> bool {
    if verb.len() < min_prefix && verb != key {
        // Allow exact short aliases listed explicitly (e.g. "B", "Q") even if
        // min_prefix for the canonical English name is longer.
        if key.len() == verb.len() && key == verb {
            return true;
        }
        if verb.len() < min_prefix {
            return false;
        }
    }
    key.starts_with(verb) || verb.starts_with(key)
}

fn split_verb(rest: &str) -> (&str, &str) {
    let mut split = rest.splitn(2, ' ');
    let verb = split.next().unwrap_or("");
    let args = split.next().unwrap_or("").trim_end();
    (verb, args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_abbreviations() {
        assert_eq!(parse_line("//VER").unwrap().0, RemoteAction::Version);
        assert_eq!(
            parse_line("//echo hi").unwrap(),
            (RemoteAction::Echo, "hi".into())
        );
        assert_eq!(
            parse_line("//3 cq").unwrap().0,
            RemoteAction::CrossChannelText
        );
        assert_eq!(parse_line("//DIR *.txt").unwrap().0, RemoteAction::Catalog);
        assert_eq!(parse_line("//KILL").unwrap().0, RemoteAction::Delete);
        assert_eq!(
            parse_line("//CD ..").unwrap(),
            (RemoteAction::ChangeDir, "..".into())
        );
        assert_eq!(parse_line("//PAR").unwrap().0, RemoteAction::Parameters);
        assert_eq!(parse_line("//P").unwrap().0, RemoteAction::Parameters);
        assert_eq!(parse_line("//PR").unwrap().0, RemoteAction::Prompt);
        assert_eq!(parse_line("//PRO").unwrap().0, RemoteAction::Prompt);
        assert_eq!(parse_line("//PROMPT ON").unwrap().0, RemoteAction::Prompt);
        assert_eq!(parse_line("//CSTATUS").unwrap().0, RemoteAction::ChannelStatus);
        assert_eq!(parse_line("//CS").unwrap().0, RemoteAction::ChannelStatus);
        assert_eq!(parse_line("//CST").unwrap().0, RemoteAction::ChannelStatus);
        assert_eq!(parse_line("//C").unwrap().0, RemoteAction::RemoteConnect);
        assert_eq!(
            parse_line("//7+ ON").unwrap(),
            (RemoteAction::SevenPlusAutoSave, "ON".into())
        );
        assert_eq!(
            parse_line("//SEND NL1STK-2").unwrap(),
            (RemoteAction::Send, "NL1STK-2".into())
        );
        assert!(parse_line("//SEN NL1STK").is_none());
        assert_eq!(parse_line("//MAIL").unwrap().0, RemoteAction::Mail);
        assert_eq!(parse_line("//MA").unwrap().0, RemoteAction::Mail);
        assert_eq!(parse_line("//MH").unwrap().0, RemoteAction::MHeard);
        assert_eq!(parse_line("//Q").unwrap().0, RemoteAction::Quit);
        assert_eq!(parse_line("//DISC").unwrap().0, RemoteAction::Disconnect);
        assert_eq!(
            parse_line("//Name Jara").unwrap(),
            (RemoteAction::Name, "Jara".into())
        );
        assert_eq!(parse_line("//N Jara").unwrap().0, RemoteAction::Name);
        assert_eq!(parse_line("//bell").unwrap().0, RemoteAction::Bell);
        assert_eq!(parse_line("//B").unwrap().0, RemoteAction::Bell);
        assert_eq!(parse_line("//ring").unwrap().0, RemoteAction::Bell);
        assert_eq!(parse_line("//RI").unwrap().0, RemoteAction::Bell);
        assert_eq!(parse_line("//R").unwrap().0, RemoteAction::Read);
        assert_eq!(
            parse_line("//Name   Jara  ").unwrap(),
            (RemoteAction::Name, "Jara".into())
        );
        assert_eq!(
            parse_line("//mh  50").unwrap(),
            (RemoteAction::MHeard, "50".into())
        );
    }

    #[test]
    fn leading_space_is_not_a_command() {
        assert_eq!(parse_line(" //mh"), None);
        assert_eq!(parse_line("  //Help"), None);
        assert_eq!(parse_line("\t//VER"), None);
        assert_eq!(parse_line("//VER").unwrap().0, RemoteAction::Version);
    }

    #[test]
    fn typed_command_on_a_link_is_for_the_other_station() {
        assert!(!typed_runs_locally(true, RemoteAction::MHeard));
        assert!(!typed_runs_locally(true, RemoteAction::Bell));
        assert!(typed_runs_locally(false, RemoteAction::MHeard));
        assert!(!typed_runs_locally(false, RemoteAction::Bell));
    }
}
