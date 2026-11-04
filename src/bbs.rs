//! DieBox `CHECK` and `LIST` lines for the BBS menu and the Box-List.
//!
//! The samples are the ones in the DieBox mailbox handbook (DB0HB):
//! `1 DL8FAZ > ALLE......53 12.05.90 ALLE 611 10 Pr-Programme`
//! and `1 DF8AY 03.04.90 17.33 845 Terminalprogramm`.
//! F6FBB, DK5SG, and DB0IE are not recognised here.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub number: u32,
    /// Rubric from a CHECK line. Empty on a LIST line.
    pub rubric: String,
    pub from: String,
    pub date: String,
    pub time: String,
    pub bytes: u32,
    pub lifetime: u32,
    pub dist: String,
    pub title: String,
    pub marked: bool,
}

impl Entry {
    /// DieBox read. CHECK is `R ALLE 53`. LIST is `R 1`, or `R IBM 1` when a
    /// file drawer is set.
    pub fn read_command(&self, list: bool, drawer: &str) -> String {
        if list {
            let drawer = drawer.trim();
            if drawer.is_empty() {
                format!("R {}", self.number)
            } else {
                format!("R {drawer} {}", self.number)
            }
        } else {
            format!("R {} {}", self.rubric, self.number)
        }
    }

    pub fn display(&self) -> String {
        let mark = if self.marked { '*' } else { ' ' };
        if self.rubric.is_empty() {
            format!(
                "{mark} {:>5} {:<9} {} {}  {}",
                self.number, self.from, self.date, self.time, self.title
            )
        } else {
            format!(
                "{mark} {:>5} {:<8} {:<9} {}  {}",
                self.number, self.rubric, self.from, self.date, self.title
            )
        }
    }

    pub fn matches(&self, term: &str) -> bool {
        let term = term.trim();
        if term.is_empty() {
            return false;
        }
        let blob = format!(
            "{} {} {} {} {} {}",
            self.number, self.rubric, self.from, self.dist, self.title, self.display()
        );
        blob.to_ascii_lowercase().contains(&term.to_ascii_lowercase())
    }
}

/// `(All) DL1BHO de DK0MAV>` — the list is complete.
pub fn is_diebox_prompt(line: &str) -> bool {
    let t = line.trim();
    t.ends_with('>') && t.to_ascii_lowercase().contains(" de ")
}

pub fn parse_check(line: &str) -> Option<Entry> {
    let line = line.trim();
    let (left, right) = line.split_once('>')?;
    let mut left_it = left.split_whitespace();
    let _index = left_it.next()?;
    let from = left_it.next()?;
    if left_it.next().is_some() || !looks_like_call(from) {
        return None;
    }
    let mut it = right.split_whitespace();
    let file = it.next()?;
    let date = it.next()?;
    let dist = it.next()?;
    let bytes: u32 = it.next()?.parse().ok()?;
    let lifetime: u32 = it.next()?.parse().ok()?;
    if !is_date(date) {
        return None;
    }
    let (rubric, number) = split_board_number(file)?;
    let title = it.collect::<Vec<_>>().join(" ");
    Some(Entry {
        number,
        rubric,
        from: from.to_string(),
        date: date.to_string(),
        time: String::new(),
        bytes,
        lifetime,
        dist: dist.to_string(),
        title,
        marked: false,
    })
}

pub fn parse_list(line: &str) -> Option<Entry> {
    let line = line.trim();
    if line.contains('>') {
        return None;
    }
    let mut it = line.split_whitespace();
    let number: u32 = it.next()?.parse().ok()?;
    let from = it.next()?;
    let date = it.next()?;
    let time = it.next()?;
    let bytes: u32 = it.next()?.parse().ok()?;
    if !looks_like_call(from) || !is_date(date) || !is_time(time) {
        return None;
    }
    let title = it.collect::<Vec<_>>().join(" ");
    Some(Entry {
        number,
        rubric: String::new(),
        from: from.to_string(),
        date: date.to_string(),
        time: time.to_string(),
        bytes,
        lifetime: 0,
        dist: String::new(),
        title,
        marked: false,
    })
}

fn split_board_number(file: &str) -> Option<(String, u32)> {
    let cut = file.rfind(|c: char| !c.is_ascii_digit())?;
    let number: u32 = file[cut + 1..].parse().ok()?;
    let rubric = file[..=cut].trim_end_matches('.').trim();
    if rubric.is_empty() {
        return None;
    }
    Some((rubric.to_string(), number))
}

fn is_date(s: &str) -> bool {
    let p: Vec<&str> = s.split('.').collect();
    p.len() == 3
        && p[0].len() == 2
        && p[1].len() == 2
        && (p[2].len() == 2 || p[2].len() == 4)
        && p.iter().all(|x| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit()))
}

fn is_time(s: &str) -> bool {
    let p: Vec<&str> = s.split(['.', ':']).collect();
    p.len() == 2
        && p[0].len() == 2
        && p[1].len() == 2
        && p.iter().all(|x| x.chars().all(|c| c.is_ascii_digit()))
}

fn looks_like_call(s: &str) -> bool {
    let (base, ssid) = s.split_once('-').unwrap_or((s, ""));
    if !ssid.is_empty() && ssid.parse::<u8>().ok().is_none_or(|n| !(0..=15).contains(&n)) {
        return false;
    }
    let n = base.chars().count();
    (3..=6).contains(&n)
        && base.chars().any(|c| c.is_ascii_alphabetic())
        && base.chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_line_keeps_the_rubric_number() {
        let e = parse_check("1 DL8FAZ > ALLE......53 12.05.90 ALLE 611 10 Pr-Programme").unwrap();
        assert_eq!(e.number, 53);
        assert_eq!(e.rubric, "ALLE");
        assert_eq!(e.from, "DL8FAZ");
        assert_eq!(e.bytes, 611);
        assert_eq!(e.lifetime, 10);
        assert_eq!(e.title, "Pr-Programme");
        assert_eq!(e.read_command(false, ""), "R ALLE 53");
        let e = parse_check("2 FP5CJ > ALL......154 12.05.90 EU 1667 10 INFO ON IC2KL").unwrap();
        assert_eq!(e.number, 154);
        assert_eq!(e.rubric, "ALL");
        assert_eq!(e.title, "INFO ON IC2KL");
        let e = parse_check("4 OM3ABC > TNC2......15 01.11.26 EU 100 30 Hallo").unwrap();
        assert_eq!((e.rubric.as_str(), e.number), ("TNC2", 15));
    }

    #[test]
    fn list_line_is_the_rubric_index() {
        let e = parse_list("1 DF8AY 03.04.90 17.33 845 Terminalprogramm").unwrap();
        assert_eq!(e.number, 1);
        assert!(e.rubric.is_empty());
        assert_eq!(e.time, "17.33");
        assert_eq!(e.read_command(true, ""), "R 1");
        assert_eq!(e.read_command(true, "IBM"), "R IBM 1");
        let e = parse_list("3 DC2AJ 07.04.90 15:32 765 Hallo Lutz").unwrap();
        assert_eq!(e.title, "Hallo Lutz");
        assert!(parse_list("1 DL8FAZ > ALLE......53 12.05.90 ALLE 611 10 Pr-Programme").is_none());
        assert!(parse_check("Nr Call Datum Zeit Bytes Titel").is_none());
    }

    #[test]
    fn prompt_closes_the_list() {
        assert!(is_diebox_prompt("(All) DL1BHO de DK0MAV>"));
        assert!(is_diebox_prompt("() DF8AY de DB0BQ>"));
        assert!(!is_diebox_prompt("1 DF8AY 03.04.90 17.33 845 Terminalprogramm"));
    }
}
