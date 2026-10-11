use std::collections::HashMap;
use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::changed_files::FileStatus;
use crate::file_diff::{FileDiffSpec, Rev, Version, decode, display};
use crate::oid::Oid;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlameSpec {
    pub rev: Rev,
    pub path: String,
}

impl BlameSpec {
    pub fn reads_working_tree(&self) -> bool {
        self.rev == Rev::WorkingTree
    }
}

/// Move and copy detection; off by default, as in TortoiseGit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Moves {
    #[default]
    Off,
    WithinFile,
    AcrossFiles,
}

impl Moves {
    pub const ALL: [Moves; 3] = [Moves::Off, Moves::WithinFile, Moves::AcrossFiles];

    pub fn label(self) -> &'static str {
        match self {
            Moves::Off => "Off",
            Moves::WithinFile => "In file",
            Moves::AcrossFiles => "Across files",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Moves::Off => {
                "A line moved or copied, within the file or from another, counts as changed \
                 where it landed: it belongs to the commit that moved it."
            }
            Moves::WithinFile => {
                "Detect lines moved or copied within the file: they keep the commit that \
                 wrote them. As git blame -M."
            }
            Moves::AcrossFiles => {
                "Detect lines moved or copied within the file, and from other files the same \
                 commit changed: they keep the commit that wrote them. As git blame -M -C."
            }
        }
    }

    pub(crate) fn args(self) -> &'static [&'static str] {
        match self {
            Moves::Off => &[],
            Moves::WithinFile => &["-M"],
            Moves::AcrossFiles => &["-M", "-C"],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BlameOptions {
    pub ignore_whitespace: bool,
    pub moves: Moves,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    /// `None` for uncommitted lines.
    pub commit: Option<Oid>,
    pub path: String,
    /// `None` for a new file, or where the history stops.
    pub previous: Option<Version>,
    pub author: String,
    pub author_email: String,
    pub author_time: i64,
    /// `+hhmm` or `-hhmm`.
    pub author_tz: String,
    /// For uncommitted lines, when git blamed them.
    pub committer_time: i64,
    pub summary: String,
    /// The history stops here: a shallow clone's oldest commit, or a graft.
    pub boundary: bool,
}

impl Origin {
    pub fn changes(&self) -> Option<FileDiffSpec> {
        if self.boundary {
            return None;
        }
        let status = match &self.previous {
            None => FileStatus::Added,
            Some(v) if v.path != self.path => FileStatus::Renamed,
            Some(_) => FileStatus::Modified,
        };
        // Blamed files are regular text files; their exact modes don't change the diff.
        const REGULAR_FILE_MODE: u32 = 0o100644;
        let old_mode = if self.previous.is_some() {
            REGULAR_FILE_MODE
        } else {
            0
        };
        Some(FileDiffSpec {
            modes: [old_mode, REGULAR_FILE_MODE],
            old: self.previous.clone(),
            new: Some(Version {
                rev: self.commit.map_or(Rev::WorkingTree, Rev::Commit),
                path: self.path.clone(),
            }),
            status,
            binary: false,
        })
    }

    pub fn previous_blame(&self) -> Option<BlameSpec> {
        self.previous.as_ref().map(|v| BlameSpec {
            rev: v.rev,
            path: v.path.clone(),
        })
    }

    /// `YYYY-MM-DD HH:MM` in the author's own zone, for commits whose local date the snapshot
    /// lacks.
    pub fn author_date(&self) -> String {
        let tz = &self.author_tz;
        let sign = if tz.starts_with('-') { -1 } else { 1 };
        let digits = tz.trim_start_matches(['+', '-']);
        let number = |at: Range<usize>| {
            digits
                .get(at)
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0)
        };
        let (hours, minutes) = (number(0..2), number(2..4));
        format_time(self.author_time + sign * (hours * 3600 + minutes * 60))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    /// Index into [`Blame::origins`].
    pub origin: usize,
    /// 0-based, in the origin's version of the file.
    pub orig_line: u32,
    /// Without its newline; a carriage return stays.
    pub raw: String,
    /// As shown: tabs expanded, no line ending.
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blame {
    pub origins: Vec<Origin>,
    pub lines: Vec<BlameLine>,
    pub invalid_bytes: usize,
    /// In characters, tabs expanded.
    pub widest: usize,
}

impl Blame {
    /// Parses `git blame --line-porcelain`.
    pub fn parse(out: &[u8]) -> Result<Blame, String> {
        let mut blame = Blame::default();
        let mut by_key: HashMap<(Option<Oid>, String, Option<Version>), usize> = HashMap::new();
        // An entry is its header, its fields, then the line's text.
        let mut header: Option<(Option<Oid>, u32)> = None;
        let mut fields = Fields::default();
        for line in out.split_inclusive(|&b| b == b'\n') {
            let line = line.strip_suffix(b"\n").unwrap_or(line);
            if let Some(content) = line.strip_prefix(b"\t") {
                let Some((commit, orig_line)) = header.take() else {
                    return Err("a line of the file before its commit".into());
                };
                let f = std::mem::take(&mut fields);
                let path = f.filename.ok_or("a line without its file name")?;
                let origins = &mut blame.origins;
                let origin = *by_key.entry((commit, path, f.previous)).or_insert_with_key(
                    |(commit, path, previous)| {
                        origins.push(Origin {
                            commit: *commit,
                            path: path.clone(),
                            previous: previous.clone(),
                            author: f.author,
                            author_email: f.author_email,
                            author_time: f.author_time,
                            author_tz: f.author_tz,
                            committer_time: f.committer_time,
                            summary: f.summary,
                            boundary: f.boundary,
                        });
                        origins.len() - 1
                    },
                );
                let (raw, invalid) = decode(content);
                blame.invalid_bytes += invalid;
                let (text, _) = display(&raw, &[]);
                blame.widest = blame.widest.max(text.chars().count());
                blame.lines.push(BlameLine {
                    origin,
                    orig_line,
                    raw,
                    text,
                });
                continue;
            }
            let line = String::from_utf8_lossy(line);
            if header.is_none() {
                header = Some(parse_header(&line, blame.lines.len())?);
                continue;
            }
            let (key, value) = line.split_once(' ').unwrap_or((&line, ""));
            match key {
                "author" => fields.author = value.to_owned(),
                "author-mail" => {
                    let mail = value.strip_prefix('<').unwrap_or(value);
                    fields.author_email = mail.strip_suffix('>').unwrap_or(mail).to_owned();
                }
                "author-time" => {
                    fields.author_time = value
                        .parse()
                        .map_err(|_| format!("author time {value:?}"))?;
                }
                "author-tz" => fields.author_tz = value.to_owned(),
                "committer-time" => {
                    fields.committer_time = value
                        .parse()
                        .map_err(|_| format!("committer time {value:?}"))?;
                }
                "summary" => fields.summary = value.to_owned(),
                "boundary" => fields.boundary = true,
                "previous" => {
                    let (hex, path) = value
                        .split_once(' ')
                        .ok_or_else(|| format!("previous {value:?}"))?;
                    let oid = Oid::from_hex(hex).ok_or_else(|| format!("previous {value:?}"))?;
                    fields.previous = Some(Version {
                        rev: Rev::Commit(oid),
                        path: unquote(path)?,
                    });
                }
                "filename" => fields.filename = Some(unquote(value)?),
                _ => {}
            }
        }
        if header.is_some() {
            return Err("the output ends inside an entry".into());
        }
        Ok(blame)
    }

    /// Each origin's age from 0 (oldest) to 1 (newest, and uncommitted lines), by rank of
    /// author time, so that one ancient commit doesn't wash out the rest.
    pub fn ages(&self) -> Vec<f32> {
        let mut times: Vec<i64> = self
            .origins
            .iter()
            .filter(|o| o.commit.is_some())
            .map(|o| o.author_time)
            .collect();
        times.sort_unstable();
        times.dedup();
        let last = times.len().saturating_sub(1).max(1) as f32;
        self.origins
            .iter()
            .map(|o| {
                if o.commit.is_none() || times.len() <= 1 {
                    return 1.0;
                }
                let rank = times.partition_point(|&t| t < o.author_time);
                rank as f32 / last
            })
            .collect()
    }

    pub fn starts_run(&self, i: usize) -> bool {
        i == 0 || self.lines[i].origin != self.lines[i - 1].origin
    }

    /// Uncommitted lines don't count.
    pub fn commit_count(&self) -> usize {
        let mut commits: Vec<Oid> = self.origins.iter().filter_map(|o| o.commit).collect();
        commits.sort_unstable();
        commits.dedup();
        commits.len()
    }
}

#[derive(Default)]
struct Fields {
    author: String,
    author_email: String,
    author_time: i64,
    author_tz: String,
    committer_time: i64,
    summary: String,
    boundary: bool,
    previous: Option<Version>,
    filename: Option<String>,
}

/// `<hash> <orig line> <final line> [<group size>]`, lines from 1. The lines must come in
/// order.
fn parse_header(line: &str, lines_read: usize) -> Result<(Option<Oid>, u32), String> {
    let bad = || format!("blame entry {line:?}");
    let mut words = line.split(' ');
    let oid = words.next().and_then(Oid::from_hex).ok_or_else(bad)?;
    let orig_line: u32 = words.next().and_then(|w| w.parse().ok()).ok_or_else(bad)?;
    let final_line: usize = words.next().and_then(|w| w.parse().ok()).ok_or_else(bad)?;
    if final_line != lines_read + 1 || orig_line == 0 {
        return Err(bad());
    }
    let uncommitted = oid.as_bytes().iter().all(|&b| b == 0);
    Ok(((!uncommitted).then_some(oid), orig_line - 1))
}

/// Undoes git's C-style quoting of a path with special characters.
fn unquote(s: &str) -> Result<String, String> {
    let Some(inner) = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return Ok(s.to_owned());
    };
    let bad = || format!("quoted path {s:?}");
    let mut bytes = Vec::with_capacity(inner.len());
    let mut quoted = inner.bytes();
    while let Some(b) = quoted.next() {
        if b != b'\\' {
            bytes.push(b);
            continue;
        }
        let escaped = quoted.next().ok_or_else(bad)?;
        bytes.push(match escaped {
            b'a' => 7,
            b'b' => 8,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 11,
            b'f' => 12,
            b'r' => b'\r',
            b'"' | b'\\' => escaped,
            b'0'..=b'3' => {
                let mut n = u32::from(escaped - b'0');
                for _ in 0..2 {
                    let d = quoted.next().filter(u8::is_ascii_digit).ok_or_else(bad)?;
                    n = n * 8 + u32::from(d - b'0');
                }
                u8::try_from(n).map_err(|_| bad())?
            }
            _ => return Err(bad()),
        });
    }
    String::from_utf8(bytes).map_err(|_| bad())
}

/// `YYYY-MM-DD HH:MM` of seconds since the epoch, in UTC or the zone they were shifted to.
fn format_time(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "c4275f7dbe7e820e3bb9a21c7d1cc1317657f2d4";
    const B: &str = "8e09b4551acb469f9df4ce58895b77e2e7e4190e";
    const ZERO: &str = "0000000000000000000000000000000000000000";

    fn entry(hash: &str, orig: u32, fin: u32, time: i64, extra: &str, line: &str) -> String {
        format!(
            "{hash} {orig} {fin} 1\nauthor A B\nauthor-mail <a@b>\nauthor-time {time}\n\
             author-tz +0200\ncommitter A B\ncommitter-mail <a@b>\ncommitter-time {}\n\
             committer-tz +0200\nsummary s{time}\n{extra}\t{line}\n",
            time + 5
        )
    }

    fn sample() -> String {
        [
            entry(A, 1, 1, 100, "filename a.txt\n", "one"),
            entry(
                B,
                2,
                2,
                200,
                &format!("previous {A} a.txt\nfilename \"b \\\"q\\\".txt\"\n"),
                "2\tx",
            ),
            entry(A, 3, 3, 100, "filename a.txt\n", "three"),
            entry(
                ZERO,
                4,
                4,
                300,
                &format!("previous {B} \"b \\\"q\\\".txt\"\nfilename \"b \\\"q\\\".txt\"\n"),
                "four\r",
            ),
        ]
        .concat()
    }

    #[test]
    fn lines_share_their_origins() {
        let blame = Blame::parse(sample().as_bytes()).unwrap();
        let origins: Vec<usize> = blame.lines.iter().map(|l| l.origin).collect();
        assert_eq!(origins, [0, 1, 0, 2]);
        assert_eq!(blame.origins.len(), 3);
        let b = &blame.origins[1];
        assert_eq!(b.path, "b \"q\".txt");
        assert_eq!(b.author_email, "a@b");
        assert_eq!(b.summary, "s200");
        assert_eq!((b.author_time, b.committer_time), (200, 205));
        assert_eq!(
            b.previous,
            Some(Version {
                rev: Rev::Commit(Oid::from_hex(A).unwrap()),
                path: "a.txt".into()
            })
        );
        assert_eq!(blame.lines[1].text, "2   x");
        assert_eq!(blame.lines[1].raw, "2\tx");
        assert_eq!(blame.lines[3].raw, "four\r");
        assert_eq!(blame.lines[3].text, "four");
        assert_eq!(blame.origins[2].commit, None);
        assert_eq!(blame.commit_count(), 2);
        assert!(blame.starts_run(1) && blame.starts_run(2));
    }

    #[test]
    fn ages_rank_commits_and_uncommitted_lines_are_newest() {
        let blame = Blame::parse(sample().as_bytes()).unwrap();
        assert_eq!(blame.ages(), [0.0, 1.0, 1.0]);
    }

    #[test]
    fn a_line_leads_to_its_change_and_the_blame_before_it() {
        let blame = Blame::parse(sample().as_bytes()).unwrap();
        let [a, b, wt] = [0, 1, 2].map(|i| &blame.origins[i]);
        let added = a.changes().unwrap();
        assert_eq!((added.old, added.status), (None, FileStatus::Added));
        assert_eq!(a.previous_blame(), None);
        let renamed = b.changes().unwrap();
        assert_eq!(renamed.status, FileStatus::Renamed);
        assert_eq!(renamed.old.unwrap().path, "a.txt");
        assert_eq!(
            b.previous_blame(),
            Some(BlameSpec {
                rev: Rev::Commit(Oid::from_hex(A).unwrap()),
                path: "a.txt".into()
            })
        );
        let wt = wt.changes().unwrap();
        assert_eq!(wt.new.unwrap().rev, Rev::WorkingTree);
        assert_eq!(wt.status, FileStatus::Modified);
    }

    #[test]
    fn where_the_history_stops_there_is_no_change_to_show() {
        let out = entry(A, 1, 1, 100, "boundary\nfilename a.txt\n", "one");
        let blame = Blame::parse(out.as_bytes()).unwrap();
        assert!(blame.origins[0].boundary);
        assert_eq!(blame.origins[0].changes(), None);
    }

    #[test]
    fn lines_out_of_order_or_cut_short_are_errors() {
        let out = entry(A, 1, 2, 100, "filename a.txt\n", "one");
        assert!(Blame::parse(out.as_bytes()).is_err());
        let out = format!("{A} 1 1 1\nauthor A\n");
        assert!(Blame::parse(out.as_bytes()).is_err());
    }

    #[test]
    fn quoted_paths_are_unquoted() {
        assert_eq!(unquote("plain name").unwrap(), "plain name");
        assert_eq!(unquote(r#""a\tb\\c\"d""#).unwrap(), "a\tb\\c\"d");
        // Octal escapes are bytes of UTF-8.
        assert_eq!(unquote(r#""\303\251t\303\251""#).unwrap(), "été");
        assert!(unquote(r#""\q""#).is_err());
    }

    #[test]
    fn times_are_formatted_in_the_authors_zone() {
        let o = |time, tz: &str| Origin {
            commit: None,
            path: String::new(),
            previous: None,
            author: String::new(),
            author_email: String::new(),
            author_time: time,
            author_tz: tz.into(),
            committer_time: time,
            summary: String::new(),
            boundary: false,
        };
        assert_eq!(o(0, "+0000").author_date(), "1970-01-01 00:00");
        assert_eq!(o(1_700_000_000, "+0000").author_date(), "2023-11-14 22:13");
        assert_eq!(o(1_700_000_000, "+0530").author_date(), "2023-11-15 03:43");
        assert_eq!(o(0, "-0100").author_date(), "1969-12-31 23:00");
        // A leap day.
        assert_eq!(o(951_782_400, "+0000").author_date(), "2000-02-29 00:00");
    }
}
