//! Blame: which commit last changed each line of a file, as `git blame` finds it.
//!
//! A blame is of a file at a revision (a commit, or the working tree, whose changed lines
//! belong to no commit yet). Git does the work
//! ([`crate::git::Git::blame`], `git blame --line-porcelain`, through the file's textconv
//! filter as a diff reads it); this module parses what it prints into lines and the
//! [`Origin`]s they come from, and says where to go from a line: the change that brought it
//! in ([`Origin::changes`]) and the blame of the version before ([`Origin::previous_blame`]),
//! as TortoiseGitBlame does.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::changed_files::FileStatus;
use crate::file_diff::{FileDiffSpec, Rev, Version, decode, display};
use crate::oid::Oid;

/// What to blame: a file at a revision.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlameSpec {
    pub rev: Rev,
    pub path: String,
}

impl BlameSpec {
    /// True if the file is read from the working tree, and so can change after loading.
    pub fn reads_working_tree(&self) -> bool {
        self.rev == Rev::WorkingTree
    }
}

/// Whether lines moved or copied from elsewhere keep the commit that wrote them, rather than
/// the one that moved them (`git blame -M`, `-C`). Off by default, as in TortoiseGit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Moves {
    #[default]
    Off,
    /// Lines moved or copied within the file (`-M`).
    WithinFile,
    /// Also lines moved or copied from other files the same commit changed (`-M -C`).
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

    /// What the choice does, in plain words, for its tooltip.
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

/// How git blames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BlameOptions {
    /// Changes in whitespace alone don't make a line new (`git blame -w`).
    pub ignore_whitespace: bool,
    pub moves: Moves,
}

/// Where a line comes from: a commit and the file's path in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    /// `None` for lines of the working tree that no commit has yet.
    pub commit: Option<Oid>,
    pub path: String,
    /// The file as it was before the commit, if git says (a new file has none, and neither
    /// has a commit where the history stops).
    pub previous: Option<Version>,
    pub author: String,
    pub author_email: String,
    /// Seconds since the epoch.
    pub author_time: i64,
    /// `+hhmm` or `-hhmm`.
    pub author_tz: String,
    /// Seconds since the epoch. For lines no commit has yet, when git blamed them.
    pub committer_time: i64,
    /// The commit's subject.
    pub summary: String,
    /// The history stops here: a shallow clone's oldest commit, or a graft. What was before
    /// isn't known.
    pub boundary: bool,
}

impl Origin {
    /// The change that brought the line in: this origin's file against its previous version.
    /// For uncommitted lines, the working tree against HEAD. `None` where the history stops
    /// here, since what came before isn't known.
    pub fn changes(&self) -> Option<FileDiffSpec> {
        if self.boundary {
            return None;
        }
        let rev = match self.commit {
            Some(oid) => Rev::Commit(oid),
            None => Rev::WorkingTree,
        };
        let status = match &self.previous {
            None => FileStatus::Added,
            Some(v) if v.path != self.path => FileStatus::Renamed,
            Some(_) => FileStatus::Modified,
        };
        // Blamed files are regular text files; their exact modes don't change the diff.
        const FILE: u32 = 0o100644;
        Some(FileDiffSpec {
            modes: [if self.previous.is_some() { FILE } else { 0 }, FILE],
            old: self.previous.clone(),
            new: Some(Version {
                rev,
                path: self.path.clone(),
            }),
            status,
            binary: false,
        })
    }

    /// The blame of the file as it was before this origin's commit, where the line was not
    /// yet what it is ("Blame previous revision").
    pub fn previous_blame(&self) -> Option<BlameSpec> {
        let v = self.previous.as_ref()?;
        Some(BlameSpec {
            rev: v.rev,
            path: v.path.clone(),
        })
    }

    /// The author's date and time in the author's own time zone, `YYYY-MM-DD HH:MM`. (The
    /// log shows commits in local time; this is for commits it doesn't know.)
    pub fn author_date(&self) -> String {
        let tz = &self.author_tz;
        let sign = if tz.starts_with('-') { -1 } else { 1 };
        let digits = tz.trim_start_matches(['+', '-']);
        let (h, m) = (
            digits.get(..2).and_then(|s| s.parse::<i64>().ok()),
            digits.get(2..4).and_then(|s| s.parse::<i64>().ok()),
        );
        let offset = sign * (h.unwrap_or(0) * 3600 + m.unwrap_or(0) * 60);
        format_time(self.author_time + offset)
    }
}

/// One line of the blamed file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    /// Index into [`Blame::origins`].
    pub origin: usize,
    /// The line's number (from 0) in the origin's version of the file.
    pub orig_line: u32,
    /// The line as in the file, without its newline (a carriage return stays).
    pub raw: String,
    /// The line as shown: tabs expanded, no line ending.
    pub text: String,
}

/// A blamed file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blame {
    pub origins: Vec<Origin>,
    pub lines: Vec<BlameLine>,
    /// Bytes that were not valid UTF-8, now shown as `\xNN`.
    pub invalid_bytes: usize,
    /// Characters in the longest shown line.
    pub widest: usize,
}

impl Blame {
    /// Parses `git blame --line-porcelain`.
    pub fn parse(out: &[u8]) -> Result<Blame, String> {
        let mut blame = Blame::default();
        let mut by_key: HashMap<(Option<Oid>, String, Option<Version>), usize> = HashMap::new();
        // The entry being read: its header, then its fields up to the line's text.
        let mut header: Option<(Option<Oid>, u32)> = None;
        let mut fields = Fields::default();
        let mut rest = out;
        while !rest.is_empty() {
            let end = rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
            let line = &rest[..end];
            rest = rest.get(end + 1..).unwrap_or(&[]);
            if let Some(content) = line.strip_prefix(b"\t") {
                let Some((commit, orig_line)) = header.take() else {
                    return Err("a line of the file before its commit".into());
                };
                let path = fields
                    .filename
                    .take()
                    .ok_or("a line without its file name")?;
                let key = (commit, path, fields.previous.take());
                let origin = match by_key.get(&key) {
                    Some(&i) => i,
                    None => {
                        let i = blame.origins.len();
                        let f = std::mem::take(&mut fields);
                        blame.origins.push(Origin {
                            commit: key.0,
                            path: key.1.clone(),
                            previous: key.2.clone(),
                            author: f.author,
                            author_email: f.author_email,
                            author_time: f.author_time,
                            author_tz: f.author_tz,
                            committer_time: f.committer_time,
                            summary: f.summary,
                            boundary: f.boundary,
                        });
                        by_key.insert(key, i);
                        i
                    }
                };
                fields = Fields::default();
                let (raw, bad) = decode(content);
                blame.invalid_bytes += bad;
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
                // The committer's name and zone, and any fields git adds later.
                _ => {}
            }
        }
        if header.is_some() {
            return Err("the output ends inside an entry".into());
        }
        Ok(blame)
    }

    /// Each origin's age among the file's commits: 0 for the oldest, 1 for the newest, by
    /// rank of author time (so that one ancient commit doesn't wash out the rest). Lines not
    /// committed yet are the newest.
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

    /// True if line `i` starts a run of lines from one origin.
    pub fn starts_run(&self, i: usize) -> bool {
        i == 0 || self.lines[i].origin != self.lines[i - 1].origin
    }

    /// How many commits the lines come from (uncommitted lines not counted).
    pub fn commit_count(&self) -> usize {
        let mut commits: Vec<Oid> = self.origins.iter().filter_map(|o| o.commit).collect();
        commits.sort_unstable();
        commits.dedup();
        commits.len()
    }
}

/// An entry's fields, as they come before its line.
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

/// `<hash> <line in origin> <line in file> [<lines in group>]`, with lines from 1; `at` is
/// how many lines were read, to check that they come in order.
fn parse_header(line: &str, at: usize) -> Result<(Option<Oid>, u32), String> {
    let bad = || format!("blame entry {line:?}");
    let mut words = line.split(' ');
    let oid = words.next().and_then(Oid::from_hex).ok_or_else(bad)?;
    let orig: u32 = words.next().and_then(|w| w.parse().ok()).ok_or_else(bad)?;
    let fin: usize = words.next().and_then(|w| w.parse().ok()).ok_or_else(bad)?;
    if fin != at + 1 || orig == 0 {
        return Err(bad());
    }
    let commit = oid.as_bytes().iter().any(|&b| b != 0).then_some(oid);
    Ok((commit, orig - 1))
}

/// A path as git writes it: as is, or in double quotes with C escapes when it holds quotes,
/// backslashes or control characters.
fn unquote(s: &str) -> Result<String, String> {
    let Some(inner) = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return Ok(s.to_owned());
    };
    let bad = || format!("quoted path {s:?}");
    let mut bytes = Vec::with_capacity(inner.len());
    let mut it = inner.bytes();
    while let Some(b) = it.next() {
        if b != b'\\' {
            bytes.push(b);
            continue;
        }
        let e = it.next().ok_or_else(bad)?;
        bytes.push(match e {
            b'a' => 7,
            b'b' => 8,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 11,
            b'f' => 12,
            b'r' => b'\r',
            b'"' | b'\\' => e,
            b'0'..=b'3' => {
                let mut n = u32::from(e - b'0');
                for _ in 0..2 {
                    let d = it.next().filter(u8::is_ascii_digit).ok_or_else(bad)?;
                    n = n * 8 + u32::from(d - b'0');
                }
                u8::try_from(n).map_err(|_| bad())?
            }
            _ => return Err(bad()),
        });
    }
    String::from_utf8(bytes).map_err(|_| bad())
}

/// Seconds since the epoch as `YYYY-MM-DD HH:MM` (UTC, or whatever zone they were moved to).
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
        // Tabs are expanded for showing, kept for copying; a carriage return is kept too.
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
        // The first commit added the file.
        let added = a.changes().unwrap();
        assert_eq!((added.old, added.status), (None, FileStatus::Added));
        assert_eq!(a.previous_blame(), None);
        // The second renamed it.
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
        // Uncommitted lines: the working tree against the commit before.
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
