//! The history pane of a blame window: the commits that changed the blamed file, one per row,
//! up to the blamed revision.
//!
//! Decided in #111 from the research on branch `research/git-file-history`. The rows are
//! git's plain history of the file ([`crate::git::Git::file_log`], `git log --no-follow
//! --topo-order`, git's default history simplification), and to it [`FileHistory::new`] adds:
//!
//! - a row for every commit the blame names that the log didn't list: commits from before a
//!   rename, where the log stops, and with moves across files, commits of other files. They
//!   come from the blame's own data, are put in by committer time and carry the path the
//!   file had there. They stand alone in the graph column;
//! - on top, a "Working tree changes" row when the blamed revision is the working tree and
//!   the file differs from `HEAD`, or the blame has lines no commit has yet.
//!
//! Rows own their data. The [`Repo`] snapshot only says which commits it has; a commit it
//! doesn't have (history rewritten since it was loaded, a merge in progress) is still listed.
//! So is every commit the blame names, so choosing works both ways: every line's commit has a
//! row ([`FileHistory::row_of`]), and every row says whether it owns lines.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use crate::blame::Blame;
use crate::log_graph::LogGraph;
use crate::oid::Oid;
use crate::repo::{CommitIx, Repo};

/// `git log -z` format for the listing: NUL-separated fields, as the snapshot's log reads them
/// (with [`DATE_FORMAT`] for `%ad`).
pub(crate) const LOG_FORMAT: &str = "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%ad%x00%ct%x00%s";
/// The author date as the snapshot's log shows it: local time, to the minute.
pub(crate) const DATE_FORMAT: &str = "--date=format-local:%Y-%m-%d %H:%M";
const LOG_FIELDS: usize = 8;

/// What git lists for a blamed file ([`crate::git::Git::file_log`]), before the blame's own
/// commits are added.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileLog {
    /// The path the log ran on: the blamed path, or for the working tree the file's path in
    /// `HEAD`, which differs after a staged rename.
    pub path: String,
    /// The blamed revision is the working tree, and the file there differs from `HEAD`.
    pub working_tree_changed: bool,
    /// The commits in git's order, newest first.
    pub commits: Vec<LogCommit>,
}

/// A commit as the listing prints it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogCommit {
    pub oid: Oid,
    /// Its parents in the file's history (git rewrites them to the listed commits), first
    /// parent first.
    pub parents: Vec<Oid>,
    pub author_name: String,
    pub author_email: String,
    /// Seconds since the epoch.
    pub author_time: i64,
    /// `YYYY-MM-DD HH:MM` in local time, as the log shows it.
    pub author_date: String,
    /// Seconds since the epoch.
    pub commit_time: i64,
    pub subject: String,
}

/// Where a row of the history pane comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// "Working tree changes": the file differs from `HEAD`.
    WorkingTree,
    /// The log listed it: it changed the file under [`FileHistory::path`].
    Log,
    /// Only the blame names it: the log didn't list it, because the file had another path
    /// there (before a rename) or the lines were moved from another file. The paths its lines
    /// had in it, in the order of their first lines; more than one only with moves across
    /// files.
    Blame { paths: Vec<String> },
}

/// One row of the history pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRow {
    /// `None` for the working tree changes, as in [`crate::blame::Origin::commit`].
    pub commit: Option<Oid>,
    pub source: Source,
    /// The parents git printed, for the graph column: rows of the log only.
    pub parents: Vec<Oid>,
    pub author_name: String,
    pub author_email: String,
    /// Seconds since the epoch; 0 for the working tree.
    pub author_time: i64,
    /// As the log shows it (local time), or in the author's zone for a commit only the blame
    /// names that the snapshot doesn't have; empty for the working tree.
    pub author_date: String,
    /// Seconds since the epoch; 0 for the working tree.
    pub commit_time: i64,
    pub subject: String,
    /// The commit in the snapshot, if it is there.
    pub snapshot: Option<CommitIx>,
    /// Some line of the blame belongs to this row.
    pub owns_lines: bool,
}

/// The rows of the history pane.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileHistory {
    /// The path the log ran on ([`FileLog::path`]).
    pub path: String,
    /// Working tree changes first, then the log's order, with the blame's own commits among
    /// them by committer time.
    pub rows: Vec<HistoryRow>,
    by_commit: HashMap<Option<Oid>, usize>,
}

impl FileHistory {
    /// The rows for `log` and the blame of the same file at the same revision.
    pub fn new(log: FileLog, blame: &Blame, repo: &Repo) -> FileHistory {
        // The commits owning lines, each with the paths its lines had, in line order.
        let mut owners: Vec<Option<Oid>> = Vec::new();
        let mut paths: HashMap<Option<Oid>, Vec<String>> = HashMap::new();
        let mut first_origin: HashMap<Option<Oid>, usize> = HashMap::new();
        for line in &blame.lines {
            let origin = &blame.origins[line.origin];
            let commit = origin.commit;
            let known = paths.entry(commit).or_insert_with(|| {
                owners.push(commit);
                first_origin.insert(commit, line.origin);
                Vec::new()
            });
            if !known.contains(&origin.path) {
                known.push(origin.path.clone());
            }
        }

        let listed: HashSet<Oid> = log.commits.iter().map(|c| c.oid).collect();
        let log_rows: Vec<HistoryRow> = log
            .commits
            .into_iter()
            .map(|c| HistoryRow {
                commit: Some(c.oid),
                source: Source::Log,
                parents: c.parents,
                author_name: c.author_name,
                author_email: c.author_email,
                author_time: c.author_time,
                author_date: c.author_date,
                commit_time: c.commit_time,
                subject: c.subject,
                snapshot: repo.lookup(&c.oid),
                owns_lines: paths.contains_key(&Some(c.oid)),
            })
            .collect();
        let blame_rows: Vec<HistoryRow> = owners
            .iter()
            .filter_map(|&commit| {
                let oid = commit?;
                if listed.contains(&oid) {
                    return None;
                }
                let o = &blame.origins[first_origin[&commit]];
                let snapshot = repo.lookup(&oid);
                Some(HistoryRow {
                    commit,
                    source: Source::Blame {
                        paths: paths[&commit].clone(),
                    },
                    parents: Vec::new(),
                    author_name: o.author.clone(),
                    author_email: o.author_email.clone(),
                    author_time: o.author_time,
                    author_date: snapshot
                        .map_or_else(|| o.author_date(), |ix| repo.commit(ix).author_date.clone()),
                    commit_time: o.committer_time,
                    subject: o.summary.clone(),
                    snapshot,
                    owns_lines: true,
                })
            })
            .collect();

        let uncommitted = paths.contains_key(&None);
        let mut rows = Vec::with_capacity(log_rows.len() + blame_rows.len() + 1);
        if log.working_tree_changed || uncommitted {
            rows.push(HistoryRow {
                commit: None,
                source: Source::WorkingTree,
                parents: Vec::new(),
                author_name: String::new(),
                author_email: String::new(),
                author_time: 0,
                author_date: String::new(),
                commit_time: 0,
                subject: String::new(),
                snapshot: None,
                owns_lines: uncommitted,
            });
        }
        rows.extend(interleave(log_rows, blame_rows));
        let by_commit = rows
            .iter()
            .enumerate()
            .map(|(i, r)| (r.commit, i))
            .collect();
        FileHistory {
            path: log.path,
            rows,
            by_commit,
        }
    }

    /// The row of a commit (`None`: the working tree changes), such as a blame origin's
    /// [`commit`](crate::blame::Origin::commit). Every commit of the blame the history was
    /// made with has one.
    pub fn row_of(&self, commit: Option<Oid>) -> Option<usize> {
        self.by_commit.get(&commit).copied()
    }

    /// The graph column's lanes, one per row. Rows of the log join as git's parents say; the
    /// working tree changes lead to the newest rows of the log (those no other row of it has
    /// for a parent: one, or one from each side of an unfinished merge). Rows only the blame
    /// names stand alone.
    pub fn graph(&self) -> LogGraph {
        let row_of = |oid: &Oid| self.by_commit.get(&Some(*oid)).copied();
        let has_child: HashSet<Oid> = self
            .rows
            .iter()
            .filter(|r| r.source == Source::Log)
            .flat_map(|r| r.parents.iter().copied())
            .collect();
        let mut parents = Vec::with_capacity(self.rows.len());
        let mut outside = Vec::with_capacity(self.rows.len());
        for (i, row) in self.rows.iter().enumerate() {
            let (ps, out): (Vec<u32>, bool) = match row.source {
                Source::WorkingTree => {
                    let tips = self.rows.iter().enumerate().filter(|(_, r)| {
                        r.source == Source::Log && r.commit.is_some_and(|c| !has_child.contains(&c))
                    });
                    (tips.map(|(k, _)| k as u32).collect(), false)
                }
                Source::Log => {
                    let found: Vec<u32> = row
                        .parents
                        .iter()
                        .filter_map(|p| row_of(p).filter(|&k| k > i).map(|k| k as u32))
                        .collect();
                    let out = found.len() < row.parents.len();
                    (found, out)
                }
                Source::Blame { .. } => (Vec::new(), false),
            };
            parents.push(ps);
            outside.push(out);
        }
        LogGraph::from_parents(parents, outside)
    }
}

/// Puts the blame's own rows among the log's: each goes before the first row of the log that
/// is older by committer time, and the log keeps its order. Among themselves, newest first;
/// equal times keep their order.
fn interleave(log: Vec<HistoryRow>, mut extra: Vec<HistoryRow>) -> Vec<HistoryRow> {
    extra.sort_by_key(|r| Reverse(r.commit_time));
    let mut out = Vec::with_capacity(log.len() + extra.len());
    let mut extra = extra.into_iter().peekable();
    for row in log {
        while let Some(e) = extra.next_if(|e| e.commit_time > row.commit_time) {
            out.push(e);
        }
        out.push(row);
    }
    out.extend(extra);
    out
}

/// Parses the listing: `git log -z` in [`LOG_FORMAT`].
pub(crate) fn parse_log(out: &[u8]) -> Result<Vec<LogCommit>, String> {
    let out = String::from_utf8_lossy(out);
    let mut tokens: Vec<&str> = out.split('\0').collect();
    // `-z` ends the last record with a NUL too.
    if tokens.last() == Some(&"") && tokens.len() % LOG_FIELDS == 1 {
        tokens.pop();
    }
    if !tokens.len().is_multiple_of(LOG_FIELDS) {
        return Err(format!(
            "file log has {} fields, not a multiple of {LOG_FIELDS}",
            tokens.len()
        ));
    }
    let (records, _) = tokens.as_chunks::<LOG_FIELDS>();
    records
        .iter()
        .map(|[hash, parents, an, ae, at, ad, ct, subject]| {
            let hash = hash.trim_start_matches('\n');
            let oid = |hex: &str| Oid::from_hex(hex).ok_or_else(|| format!("bad hash {hex:?}"));
            let time = |t: &str| t.parse().map_err(|_| format!("bad time {t:?}"));
            Ok(LogCommit {
                oid: oid(hash)?,
                parents: parents
                    .split_ascii_whitespace()
                    .map(oid)
                    .collect::<Result<_, _>>()?,
                author_name: (*an).to_owned(),
                author_email: (*ae).to_owned(),
                author_time: time(at)?,
                author_date: (*ad).to_owned(),
                commit_time: time(ct)?,
                subject: (*subject).to_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blame::{BlameLine, Origin};
    use crate::repo::{Commit, Head};

    fn oid(n: u8) -> Oid {
        Oid::from_hex(&format!("{n:040x}")).unwrap()
    }

    fn logged(n: u8, time: i64, parents: &[u8]) -> LogCommit {
        LogCommit {
            oid: oid(n),
            parents: parents.iter().map(|&p| oid(p)).collect(),
            author_name: "A".into(),
            author_email: "a@x".into(),
            author_time: time,
            author_date: String::new(),
            commit_time: time,
            subject: format!("c{n}"),
        }
    }

    fn origin(commit: Option<u8>, path: &str, time: i64) -> Origin {
        Origin {
            commit: commit.map(oid),
            path: path.into(),
            previous: None,
            author: "B".into(),
            author_email: "b@x".into(),
            author_time: time - 1,
            author_tz: "+0000".into(),
            committer_time: time,
            summary: commit.map_or(String::new(), |n| format!("c{n}")),
            boundary: false,
        }
    }

    /// A blame whose lines come from `origins`, one line each.
    fn blame(origins: Vec<Origin>) -> Blame {
        let lines = (0..origins.len())
            .map(|i| BlameLine {
                origin: i,
                orig_line: 0,
                raw: String::new(),
                text: String::new(),
            })
            .collect();
        Blame {
            origins,
            lines,
            ..Blame::default()
        }
    }

    /// A snapshot holding the commits numbered `have`.
    fn repo(have: &[u8]) -> Repo {
        let commits: Vec<Commit> = have
            .iter()
            .map(|&n| Commit {
                oid: oid(n),
                parents: Vec::new(),
                truncated: false,
                empty_tree: false,
                author_name: String::new(),
                author_email: String::new(),
                author_time: 0,
                author_date: String::new(),
                commit_time: 0,
                subject: String::new(),
            })
            .collect();
        Repo::new(
            "/x".into(),
            commits,
            Vec::new(),
            Head::Detached(CommitIx(0)),
        )
    }

    fn log(path: &str, commits: Vec<LogCommit>) -> FileLog {
        FileLog {
            path: path.into(),
            working_tree_changed: false,
            commits,
        }
    }

    fn subjects(h: &FileHistory) -> Vec<String> {
        h.rows
            .iter()
            .map(|r| match r.commit {
                Some(_) => r.subject.clone(),
                None => "wt".into(),
            })
            .collect()
    }

    #[test]
    fn the_blames_own_commits_go_in_by_committer_time_with_their_paths() {
        // The log stops at the rename 3; 1 and 2 are from before it, 5 is from another file.
        let log = log(
            "b.txt",
            vec![logged(4, 40, &[3]), logged(3, 30, &[]), logged(9, 20, &[])],
        );
        let b = blame(vec![
            origin(Some(1), "a.txt", 10),
            origin(Some(4), "b.txt", 40),
            origin(Some(5), "c.txt", 35),
            origin(Some(2), "a.txt", 25),
        ]);
        let h = FileHistory::new(log, &b, &repo(&[1, 2, 3, 4, 9]));
        assert_eq!(subjects(&h), ["c4", "c5", "c3", "c2", "c9", "c1"]);
        let row = &h.rows[h.row_of(Some(oid(2))).unwrap()];
        assert_eq!(
            row.source,
            Source::Blame {
                paths: vec!["a.txt".into()]
            }
        );
        // From the blame's data: its author and committer time.
        assert_eq!((row.author_name.as_str(), row.author_time), ("B", 24));
        assert_eq!(h.rows[0].source, Source::Log);
        // Rows the snapshot hasn't got are listed all the same.
        assert_eq!(h.rows[1].snapshot, None);
        assert_eq!(h.rows[0].snapshot, Some(CommitIx(3)));
        // Only commits the blame names own lines.
        let owns: Vec<bool> = h.rows.iter().map(|r| r.owns_lines).collect();
        assert_eq!(owns, [true, true, false, true, false, true]);
        assert_eq!(h.row_of(None), None);
    }

    #[test]
    fn a_commit_with_lines_from_several_files_has_them_all() {
        let b = blame(vec![
            origin(Some(1), "x.txt", 10),
            origin(Some(1), "y.txt", 10),
            origin(Some(1), "x.txt", 10),
        ]);
        let h = FileHistory::new(log("z.txt", Vec::new()), &b, &repo(&[]));
        assert_eq!(h.rows.len(), 1);
        assert_eq!(
            h.rows[0].source,
            Source::Blame {
                paths: vec!["x.txt".into(), "y.txt".into()]
            }
        );
    }

    #[test]
    fn the_working_tree_row_is_on_top_when_the_file_changed_or_has_uncommitted_lines() {
        let commits = || vec![logged(2, 20, &[1]), logged(1, 10, &[])];
        // Changed, but only lines taken out: the row is there and owns nothing.
        let mut changed = log("a.txt", commits());
        changed.working_tree_changed = true;
        let b = blame(vec![origin(Some(1), "a.txt", 10)]);
        let h = FileHistory::new(changed, &b, &repo(&[1, 2]));
        assert_eq!(subjects(&h), ["wt", "c2", "c1"]);
        assert!(!h.rows[0].owns_lines);
        assert_eq!(h.row_of(None), Some(0));
        // Lines of no commit always have their row.
        let b = blame(vec![
            origin(None, "a.txt", 50),
            origin(Some(2), "a.txt", 20),
        ]);
        let h = FileHistory::new(log("a.txt", commits()), &b, &repo(&[1, 2]));
        assert_eq!(h.rows[0].source, Source::WorkingTree);
        assert!(h.rows[0].owns_lines);
        // Unchanged: no row.
        let h = FileHistory::new(log("a.txt", commits()), &blame(Vec::new()), &repo(&[]));
        assert_eq!(subjects(&h), ["c2", "c1"]);
    }

    #[test]
    fn the_graph_joins_the_log_and_leaves_the_blames_commits_alone() {
        // wt; 5 merges 4 and 3, both from 1; 2 only in the blame.
        let mut l = log(
            "a.txt",
            vec![
                logged(5, 50, &[4, 3]),
                logged(4, 40, &[1]),
                logged(3, 30, &[1]),
                logged(1, 10, &[]),
            ],
        );
        l.working_tree_changed = true;
        let b = blame(vec![origin(Some(2), "old.txt", 20)]);
        let h = FileHistory::new(l, &b, &repo(&[]));
        assert_eq!(subjects(&h), ["wt", "c5", "c4", "c3", "c2", "c1"]);
        let g = h.graph();
        assert_eq!(g.len(), 6);
        let rows = g.rows(0..6);
        // The working tree leads to the newest commit of the log.
        assert_eq!(rows[0].lower.len(), 1);
        assert!(rows[1].merge);
        // 2 has no line in or out of its own; the lane to 1 passes it by.
        assert!(
            rows[4]
                .lower
                .iter()
                .all(|l| l.from == l.to && l.to != rows[4].lane)
        );
        assert!(rows[4].upper.iter().all(|l| l.to != rows[4].lane));
        assert!(!rows.iter().any(|r| r.outside));
    }

    #[test]
    fn during_a_merge_the_working_tree_leads_to_both_sides() {
        let mut l = log(
            "a.txt",
            vec![logged(3, 30, &[1]), logged(2, 20, &[1]), logged(1, 10, &[])],
        );
        l.working_tree_changed = true;
        let h = FileHistory::new(l, &blame(Vec::new()), &repo(&[]));
        let rows = h.graph().rows(0..4);
        assert!(rows[0].merge);
    }

    #[test]
    fn equal_times_keep_the_log_first_and_the_blames_order() {
        let log_rows = vec![logged(3, 30, &[]), logged(1, 10, &[])];
        let b = blame(vec![
            origin(Some(5), "a", 30),
            origin(Some(6), "a", 20),
            origin(Some(7), "a", 20),
        ]);
        let h = FileHistory::new(log("a", log_rows), &b, &repo(&[]));
        assert_eq!(subjects(&h), ["c3", "c5", "c6", "c7", "c1"]);
    }

    #[test]
    fn parses_the_listing() {
        let (a, b) = ("a".repeat(40), "b".repeat(40));
        let out = format!(
            "{b}\0{a}\0N\x1fame\0n@x\020\01970-01-01 00:00\021\0sub ject\0\n\
             {a}\0\0M\0m@x\010\01970-01-01 00:00\011\0first\0"
        );
        let commits = parse_log(out.as_bytes()).unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].parents, [Oid::from_hex(&a).unwrap()]);
        assert_eq!(commits[0].author_name, "N\x1fame");
        assert_eq!((commits[0].author_time, commits[0].commit_time), (20, 21));
        assert_eq!(commits[0].author_date, "1970-01-01 00:00");
        assert_eq!(commits[0].subject, "sub ject");
        assert!(commits[1].parents.is_empty());
        assert!(parse_log(b"x\0y\0").is_err());
    }
}
