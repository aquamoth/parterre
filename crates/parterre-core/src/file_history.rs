//! The rows are git's plain history of the file (`git log --no-follow --topo-order`, git's
//! default history simplification), plus:
//!
//! - a row for every commit the blame names that the log didn't list: commits from before a
//!   rename, where the log stops, and with moves across files, commits of other files. They
//!   come from the blame's own data, go in by committer time and stand alone in the graph;
//! - on top, a "Working tree changes" row when the file there differs from `HEAD`, or the blame
//!   has uncommitted lines.
//!
//! Rows own their data rather than point into the [`Repo`] snapshot, which can lack a commit
//! (history rewritten since it loaded, a merge in progress). Every line's commit has a row,
//! so choosing works both ways (#111).

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use crate::blame::Blame;
use crate::log_graph::LogGraph;
use crate::oid::Oid;
use crate::repo::{CommitIx, Repo};

pub(crate) const LOG_FORMAT: &str = "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%ad%x00%ct%x00%s";
/// As the snapshot's log dates commits.
pub(crate) const DATE_FORMAT: &str = "--date=format-local:%Y-%m-%d %H:%M";
const LOG_FIELDS: usize = 8;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileLog {
    /// For the working tree, the file's path in `HEAD`, which differs after a staged rename.
    pub path: String,
    pub working_tree_changed: bool,
    pub commits: Vec<LogCommit>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogCommit {
    pub oid: Oid,
    /// As git's history simplification rewrites them, to listed commits.
    pub parents: Vec<Oid>,
    pub author_name: String,
    pub author_email: String,
    pub author_time: i64,
    pub author_date: String,
    pub commit_time: i64,
    pub subject: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    WorkingTree,
    Log,
    /// The paths its lines had there, in line order; several only with moves across files.
    Blame {
        paths: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRow {
    /// `None` for the working tree changes.
    pub commit: Option<Oid>,
    pub source: Source,
    /// Only for rows of the log.
    pub parents: Vec<Oid>,
    pub author_name: String,
    pub author_email: String,
    pub author_time: i64,
    /// In the author's zone for a commit only the blame names and the snapshot lacks.
    pub author_date: String,
    pub commit_time: i64,
    pub subject: String,
    pub snapshot: Option<CommitIx>,
    pub owns_lines: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileHistory {
    pub path: String,
    pub rows: Vec<HistoryRow>,
    by_commit: HashMap<Option<Oid>, usize>,
}

/// A commit that owns lines of the blame, and the paths its lines had, in line order.
struct Owner {
    commit: Option<Oid>,
    first_origin: usize,
    paths: Vec<String>,
}

impl FileHistory {
    /// `log` and `blame` must be of the same file at the same revision.
    pub fn new(log: FileLog, blame: &Blame, repo: &Repo) -> FileHistory {
        let mut owners: Vec<Owner> = Vec::new();
        let mut owner_of: HashMap<Option<Oid>, usize> = HashMap::new();
        for line in &blame.lines {
            let origin = &blame.origins[line.origin];
            let i = *owner_of.entry(origin.commit).or_insert_with(|| {
                owners.push(Owner {
                    commit: origin.commit,
                    first_origin: line.origin,
                    paths: Vec::new(),
                });
                owners.len() - 1
            });
            let paths = &mut owners[i].paths;
            if !paths.contains(&origin.path) {
                paths.push(origin.path.clone());
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
                owns_lines: owner_of.contains_key(&Some(c.oid)),
            })
            .collect();
        let blame_rows: Vec<HistoryRow> = owners
            .into_iter()
            .filter_map(|owner| {
                let oid = owner.commit.filter(|oid| !listed.contains(oid))?;
                let origin = &blame.origins[owner.first_origin];
                let snapshot = repo.lookup(&oid);
                Some(HistoryRow {
                    commit: Some(oid),
                    source: Source::Blame { paths: owner.paths },
                    parents: Vec::new(),
                    author_name: origin.author.clone(),
                    author_email: origin.author_email.clone(),
                    author_time: origin.author_time,
                    author_date: snapshot.map_or_else(
                        || origin.author_date(),
                        |ix| repo.commit(ix).author_date.clone(),
                    ),
                    commit_time: origin.committer_time,
                    subject: origin.summary.clone(),
                    snapshot,
                    owns_lines: true,
                })
            })
            .collect();

        let uncommitted = owner_of.contains_key(&None);
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

    /// Every commit of the blame has a row; `None` is the working tree changes.
    pub fn row_of(&self, commit: Option<Oid>) -> Option<usize> {
        self.by_commit.get(&commit).copied()
    }

    /// The working tree changes lead to the log's tips: one, or one from each side of an
    /// unfinished merge.
    pub fn graph(&self) -> LogGraph {
        let has_child: HashSet<Oid> = self
            .rows
            .iter()
            .filter(|r| r.source == Source::Log)
            .flat_map(|r| r.parents.iter().copied())
            .collect();
        let mut parents = Vec::with_capacity(self.rows.len());
        let mut outside = Vec::with_capacity(self.rows.len());
        for (i, row) in self.rows.iter().enumerate() {
            let (row_parents, row_outside): (Vec<u32>, bool) = match row.source {
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
                        .filter_map(|&p| self.row_of(Some(p)).filter(|&k| k > i))
                        .map(|k| k as u32)
                        .collect();
                    let outside = found.len() < row.parents.len();
                    (found, outside)
                }
                Source::Blame { .. } => (Vec::new(), false),
            };
            parents.push(row_parents);
            outside.push(row_outside);
        }
        LogGraph::from_parents(parents, outside)
    }
}

/// Puts `extra` among the log's rows by committer time, keeping the log's order. On equal
/// times the log's row comes first, and `extra` keeps its order.
fn interleave(log: Vec<HistoryRow>, mut extra: Vec<HistoryRow>) -> Vec<HistoryRow> {
    extra.sort_by_key(|r| Reverse(r.commit_time));
    let mut out = Vec::with_capacity(log.len() + extra.len());
    let mut extra = extra.into_iter().peekable();
    for row in log {
        while let Some(newer) = extra.next_if(|e| e.commit_time > row.commit_time) {
            out.push(newer);
        }
        out.push(row);
    }
    out.extend(extra);
    out
}

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
        .map(
            |[
                hash,
                parents,
                name,
                email,
                authored,
                date,
                committed,
                subject,
            ]| {
                let hash = hash.trim_start_matches('\n');
                let oid = |hex: &str| Oid::from_hex(hex).ok_or_else(|| format!("bad hash {hex:?}"));
                let time = |t: &str| t.parse().map_err(|_| format!("bad time {t:?}"));
                Ok(LogCommit {
                    oid: oid(hash)?,
                    parents: parents
                        .split_ascii_whitespace()
                        .map(oid)
                        .collect::<Result<_, _>>()?,
                    author_name: (*name).to_owned(),
                    author_email: (*email).to_owned(),
                    author_time: time(authored)?,
                    author_date: (*date).to_owned(),
                    commit_time: time(committed)?,
                    subject: (*subject).to_owned(),
                })
            },
        )
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
        assert_eq!(h.rows[1].snapshot, None);
        assert_eq!(h.rows[0].snapshot, Some(CommitIx(3)));
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
