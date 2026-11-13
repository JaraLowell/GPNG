//! Classic `//` remote-command table (abbreviation-aware).
//!
//! Built from the union of `*.GPH` help packs + `README.TXT` / EXE strings.
//! Matching is case-insensitive; significant-prefix length follows `@TAG,N`.

/// What a recognised remote line should do (semantics filled in over time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteAction {
    CrossChannelText,
    SevenPlusAutoSave,
    /// `//7DECODE <file>` — join 7Plus parts in USER into the original file.
    SevenPlusDecode,
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
    /// GP DOS channel state: linked channels only (`//CSTATUS`).
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
            "Chat mode / send text to channel n (//0 leaves)",
        ),
        cmd(
            "//7+",
            &["7+"],
            2,
            RemoteAction::SevenPlusAutoSave,
            "7Plus AutoSave on/off",
        ),
        cmd(
            "//7DECODE",
            &["7DECODE"],
            7,
            RemoteAction::SevenPlusDecode,
            "Join 7Plus parts in USER into the original file",
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
            "Wake the sysop (BELL.WAV)",
        ),
        // "RI" so //R stays //Read. //Ring and //RI still page the sysop.
        cmd(
            "//RIng",
            &["RING"],
            2,
            RemoteAction::Bell,
            "Wake the sysop (BELL.WAV)",
        ),
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
            &["QUIT", "Q", "BYE"],
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
/// Unlinked `//Bell` still runs here so the operator can test the page.
pub fn typed_runs_locally(linked: bool, _action: RemoteAction) -> bool {
    !linked
}

/// Sent when Remote is off and the other station tries `//RUN`.
pub const REMOTE_OFF: &str = "<GPNG>: Remote commands are turned off by the sysop.\n";

/// A `//` word matched no internal command, no `data/*.GPI`, and no EXTERNAL program.
pub const NOT_RECOGNIZED: &str = "<GPNG>: Command not recognized\n";

/// Prefix on every GPNG status reply that goes on the air.
pub const REPORT_PREFIX: &str = "<GPNG>:";

/// Tag a remote status reply. Text that already starts with `<GPNG>:` is left alone.
///
/// GPI files, USER file bodies, mail text, help packs, and EXTERNAL program
/// output are not status replies and are sent without this tag.
pub fn report(info: &str) -> String {
    let info = info.trim_start_matches(['\r', '\n']);
    if info.starts_with(REPORT_PREFIX) {
        info.to_string()
    } else {
        format!("{REPORT_PREFIX} {info}")
    }
}

/// Status title on the first line, body on the lines after it.
///
/// `//HELP` → `<GPNG>: Available Help commands` then the list.
/// `//CSTATUS` → `<GPNG>: Channel state %D %T %Z` then one line per link.
pub fn report_block(title: &str, body: &str) -> String {
    let title = title.trim();
    let body = body
        .trim_start_matches(['\r', '\n'])
        .trim_end_matches(['\r', '\n']);
    if body.is_empty() {
        report(title)
    } else {
        format!("{}\r{body}", report(title))
    }
}

/// Strip trailing CR/LF, refuse leading whitespace, return the text after `//`.
fn slash_rest(line: &str) -> Option<String> {
    let line = line.trim_end_matches(['\r', '\n']).trim_end();
    if line.is_empty() || line.bytes().next().is_some_and(|b| b.is_ascii_whitespace()) {
        return None;
    }
    let rest = squeeze_spaces(line.strip_prefix("//")?);
    if rest.is_empty() {
        None
    } else {
        Some(rest)
    }
}

/// The command word of a `//` line, in uppercase. Arguments are not included.
///
/// `None` means the line is chat: `CAT` and `7+` are chat, `//CAT` and `//7+`
/// are commands. A leading space is prose. `//` with no word is not a command.
pub fn slash_verb(line: &str) -> Option<String> {
    let rest = slash_rest(line)?;
    let (verb, _) = split_verb(&rest);
    if verb.is_empty() {
        None
    } else {
        Some(verb.to_ascii_uppercase())
    }
}

/// Parse a line that may be a remote command → `(action, args)`.
///
/// A leading space means plain text (` //Help` in a help file), not a command.
/// Trailing spaces are ignored. Runs of spaces inside the command collapse to one.
/// The command word is uppercased before it is compared. A line with no `//`
/// is chat and returns `None`.
pub fn parse_line(line: &str) -> Option<(RemoteAction, String)> {
    let trimmed = line.trim_end_matches(['\r', '\n']).trim_end();
    if trimmed == "#" {
        return Some((RemoteAction::ChatMode, String::new()));
    }

    let rest = slash_rest(line)?;

    // //7+ ON  (must precede digit-channel check — '7' is also a digit)
    if rest.len() >= 2 && rest.as_bytes()[0] == b'7' && rest.as_bytes()[1] == b'+' {
        let args = rest[2..].trim().to_string();
        return Some((RemoteAction::SevenPlusAutoSave, args));
    }

    // //7DECODE file — also starts with '7', so it must beat //n chat mode
    {
        let (verb, args) = split_verb(&rest);
        let verb_up = verb.to_ascii_uppercase();
        if verb_up.len() >= 7 && "7DECODE".starts_with(verb_up.as_str()) {
            return Some((RemoteAction::SevenPlusDecode, args.to_string()));
        }
    }

    // //1 hello  — cross-channel text / chat mode; //0 leaves
    if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return Some((RemoteAction::CrossChannelText, rest));
    }

    let (verb, args) = split_verb(&rest);
    let verb_up = verb.to_ascii_uppercase();
    let action = match_verb(&verb_up)?;
    Some((action, args.to_string()))
}

/// Split `//n` / `//n text` args into `(channel, optional text)`.
///
/// Leading digits form the channel. `//0` is leave-chat (`0`, empty text).
/// `//2 hello` → `(2, "hello")`. `//02` → `(2, "")`.
pub fn split_cross_channel(args: &str) -> Option<(u8, &str)> {
    let args = args.trim();
    if args.is_empty() {
        return None;
    }
    let digit_len = args
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .map(char::len_utf8)
        .sum::<usize>();
    if digit_len == 0 {
        return None;
    }
    let n: u8 = args[..digit_len].parse().ok()?;
    let rest = args[digit_len..].trim_start();
    Some((n, rest))
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
                if best.is_none_or(|(_, s, klen)| score > s || (score == s && key.len() < klen)) {
                    best = Some((entry.action, score, key.len()));
                }
            }
        }
    }
    best.map(|(a, _, _)| a)
}

/// Operator typed `verb`; catalogue key is `key`. Accept abbreviations.
///
/// `//Q` matches `QUIT`. A longer word does not: `//QSLINFO` is not Quit,
/// so that name can be a program in EXTERNAL.
fn verb_matches(verb: &str, key: &str, min_prefix: usize) -> bool {
    // An exact short alias (`Q`, `B`) matches even when the canonical
    // name's minimum is longer. Anything shorter than that minimum does not.
    if verb != key && verb.len() < min_prefix {
        return false;
    }
    key.starts_with(verb)
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
    fn remote_off_answer_names_the_sysop() {
        assert_eq!(
            REMOTE_OFF,
            "<GPNG>: Remote commands are turned off by the sysop.\n"
        );
    }

    #[test]
    fn parses_common_abbreviations() {
        assert_eq!(parse_line("//VER").unwrap().0, RemoteAction::Version);
        assert_eq!(
            parse_line("//echo hi").unwrap(),
            (RemoteAction::Echo, "hi".into())
        );
        assert_eq!(
            parse_line("//3 cq").unwrap(),
            (RemoteAction::CrossChannelText, "3 cq".into())
        );
        assert_eq!(
            parse_line("//0").unwrap(),
            (RemoteAction::CrossChannelText, "0".into())
        );
        assert_eq!(split_cross_channel("3 cq"), Some((3, "cq")));
        assert_eq!(split_cross_channel("0"), Some((0, "")));
        assert_eq!(split_cross_channel("02"), Some((2, "")));
        assert_eq!(split_cross_channel("2hello"), Some((2, "hello")));
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
        assert_eq!(
            parse_line("//CSTATUS").unwrap().0,
            RemoteAction::ChannelStatus
        );
        assert_eq!(parse_line("//CS").unwrap().0, RemoteAction::ChannelStatus);
        assert_eq!(parse_line("//CST").unwrap().0, RemoteAction::ChannelStatus);
        assert_eq!(parse_line("//C").unwrap().0, RemoteAction::RemoteConnect);
        assert_eq!(
            parse_line("//7+ ON").unwrap(),
            (RemoteAction::SevenPlusAutoSave, "ON".into())
        );
        assert_eq!(
            parse_line("//7DECODE README.P01").unwrap(),
            (RemoteAction::SevenPlusDecode, "README.P01".into())
        );
        assert_eq!(
            parse_line("//7decode readme.txt").unwrap(),
            (RemoteAction::SevenPlusDecode, "readme.txt".into())
        );
        // Too short for 7DECODE; leading '7' is channel chat, not a command stub.
        assert_eq!(
            parse_line("//7DEC").unwrap().0,
            RemoteAction::CrossChannelText
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
        assert_eq!(parse_line("//QUIT").unwrap().0, RemoteAction::Quit);
        assert_eq!(parse_line("//BYE").unwrap().0, RemoteAction::Quit);
        assert!(parse_line("//QSLINFO").is_none());
        assert!(parse_line("//QSLINFO N1UIR").is_none());
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
    fn a_word_without_slashes_is_chat() {
        assert!(parse_line("CAT").is_none());
        assert!(parse_line("cat").is_none());
        assert!(parse_line("7+").is_none());
        assert!(parse_line("7+ ON").is_none());
        assert!(parse_line("DOS").is_none());
        assert!(slash_verb("CAT").is_none());
        assert!(slash_verb("7+").is_none());
        assert_eq!(parse_line("//CAT").unwrap().0, RemoteAction::Catalog);
        assert_eq!(parse_line("//cat").unwrap().0, RemoteAction::Catalog);
        assert_eq!(
            parse_line("//7+").unwrap().0,
            RemoteAction::SevenPlusAutoSave
        );
        assert_eq!(slash_verb("//d"), Some("D".into()));
        assert_eq!(slash_verb("//D"), Some("D".into()));
        assert_eq!(slash_verb("//dos extra"), Some("DOS".into()));
        assert_eq!(slash_verb("//7+ ON"), Some("7+".into()));
        assert!(parse_line("//D").is_none());
        assert!(parse_line("//DO").is_none());
        assert!(slash_verb("//").is_none());
        assert_eq!(NOT_RECOGNIZED, "<GPNG>: Command not recognized\n");
        assert_eq!(report("7+ ON"), "<GPNG>: 7+ ON");
        assert_eq!(report("<GPNG>: USER CONTENT"), "<GPNG>: USER CONTENT");
        assert_eq!(
            report("USER CONTENT\r     682  README.txt"),
            "<GPNG>: USER CONTENT\r     682  README.txt"
        );
        assert_eq!(
            report_block("Channel status", "Ch. 1 - IO DISCONNECTED"),
            "<GPNG>: Channel status\rCh. 1 - IO DISCONNECTED"
        );
        assert_eq!(
            report_block("Available Help commands", "//Read  : send a file"),
            "<GPNG>: Available Help commands\r//Read  : send a file"
        );
        assert_eq!(report_block("Version", ""), "<GPNG>: Version");
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
        assert!(typed_runs_locally(false, RemoteAction::Bell));
    }
}
