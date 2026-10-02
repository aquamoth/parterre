//! Resetting the open worktree's branch: what each of git's modes would do to its files and what
//! it would lose, and running one. Git decides what it refuses: `read-tree -m -u -n` is its own
//! dry run of `reset --keep`. The rest is predicted from status, diff and the target's tree, to
//! show the modes side by side before any of them runs.

use std::collections::HashMap;
use std::path::Path;

use crate::Oid;
use crate::branches::{Cancel, Catalog, Error, Report, io_error, lost_commits, run};
use crate::changed_files::{ChangedFile, FileStatus, compare_paths};
use crate::file_diff::{DiffOptions, FileDiff, FileDiffSpec, Rev};
use crate::git::{Git, GitError};

/// git's modes, without `--merge`: that one is for aborting a merge, which is left to the
/// command line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Soft,
    Mixed,
    Keep,
    Hard,
}

impl Mode {
    /// In order of how much they touch.
    pub const ALL: [Mode; 4] = [Mode::Soft, Mode::Mixed, Mode::Keep, Mode::Hard];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Soft => "Soft",
            Mode::Mixed => "Mixed",
            Mode::Keep => "Keep",
            Mode::Hard => "Hard",
        }
    }

    pub fn flag(self) -> &'static str {
        match self {
            Mode::Soft => "--soft",
            Mode::Mixed => "--mixed",
            Mode::Keep => "--keep",
            Mode::Hard => "--hard",
        }
    }
}

/// A reset the user agreed to, with what they saw it lose. It runs only while that's still
/// what it loses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reset {
    pub branch: String,
    /// Where the branch was.
    pub head: Oid,
    pub target: Oid,
    pub mode: Mode,
    pub commits: Vec<Oid>,
    /// Files whose changes are lost, by path.
    pub files: Vec<String>,
}

/// The command a reset runs.
pub fn command(mode: Mode, target: Oid) -> Vec<String> {
    vec!["reset".into(), mode.flag().into(), target.to_hex()]
}

/// The branch a reset to `target` would move, or why there's no reset to offer: only the open
/// worktree's branch is reset, not a detached HEAD (which includes a branch being rebased), not
/// while an operation is in progress, and not to the commit it's at.
pub fn branch(catalog: &Catalog, target: Oid) -> Result<&str, String> {
    if !catalog.has_working_tree {
        return Err("This repository has no working tree.".into());
    }
    let Some(branch) = catalog.current.as_deref() else {
        return Err("HEAD is detached: there's no branch to reset.".into());
    };
    let Some(head) = catalog.head else {
        return Err(format!("Branch {branch} has no commits yet."));
    };
    if head == target {
        return Err(format!("Branch {branch} is already there."));
    }
    if let Some(what) = catalog
        .worktrees
        .iter()
        .find(|w| w.open)
        .and_then(|w| w.in_progress)
    {
        return Err(format!(
            "{} is in progress. Finish or abort it first.",
            capitalized(what)
        ));
    }
    Ok(branch)
}

/// How a file differs, as one of `git status --short`'s letters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Added,
    Modified,
    Deleted,
}

impl Change {
    pub fn letter(self) -> char {
        match self {
            Change::Added => 'A',
            Change::Modified => 'M',
            Change::Deleted => 'D',
        }
    }

    fn word(self) -> &'static str {
        match self {
            Change::Added => "added",
            Change::Modified => "modified",
            Change::Deleted => "deleted",
        }
    }
}

/// What of a file's uncommitted changes is gone afterwards: not on disk, staged or committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lost {
    /// The staged version, which differed from the file on disk.
    StagedVersion,
    /// The changes on disk.
    Changes,
    StagedVersionAndChanges,
    /// An untracked file the target overwrites.
    Untracked,
}

impl Lost {
    fn words(self) -> &'static str {
        match self {
            Lost::StagedVersion => "staged version",
            Lost::Changes => "changes",
            Lost::StagedVersionAndChanges => "staged version and changes",
            Lost::Untracked => "untracked file",
        }
    }
}

/// A file as a mode leaves it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileOutcome {
    pub path: String,
    /// Afterwards, staged against the new HEAD: `git status --short`'s first letter.
    pub staged: Option<Change>,
    /// Afterwards, on disk against the index: its second letter.
    pub unstaged: Option<Change>,
    pub untracked: bool,
    /// Its file on disk is rewritten.
    pub updated: bool,
    pub lost: Option<Lost>,
    /// Git names it in its refusal.
    pub refused: bool,
    /// Lines added and removed on disk, now to afterwards; `None` for a binary file.
    pub lines: Option<(u32, u32)>,
}

impl FileOutcome {
    /// What it is afterwards and what's lost of it, in words: "Modified, not staged; lost:
    /// staged version".
    pub fn words(&self) -> String {
        let mut parts = Vec::new();
        match (self.staged, self.unstaged) {
            (Some(s), Some(u)) => parts.push(format!("{}, staged; {} since", s.word(), u.word())),
            (Some(s), None) => parts.push(format!("{}, staged", s.word())),
            (None, Some(u)) => parts.push(format!("{}, not staged", u.word())),
            (None, None) => {}
        }
        if self.untracked {
            parts.push("untracked".into());
        }
        if parts.is_empty() && self.lost.is_none() {
            parts.push(if self.updated { "updated" } else { "unchanged" }.into());
        }
        if let Some(lost) = self.lost {
            parts.push(format!("lost: {}", lost.words()));
        }
        if self.refused {
            parts.push("git refuses".into());
        }
        capitalized(&parts.join("; "))
    }
}

/// What's on disk at a path, against its staged version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Disk {
    /// As staged (or absent, when nothing is).
    AsStaged,
    Modified,
    Deleted,
    /// A file with nothing staged at its path.
    Untracked,
}

/// One path the reset concerns, as it is now. Versions are blob ids.
#[derive(Clone, Debug)]
struct Entry {
    path: String,
    head: Option<String>,
    index: Option<String>,
    disk: Disk,
    target: Option<String>,
    /// Its mode in the target, else in HEAD or the index.
    mode: u32,
    /// Lines added and removed going from the file on disk to the target's (`None` when
    /// binary); `None` outside when they're the same.
    to_target: Option<Option<(u32, u32)>>,
}

impl Entry {
    fn staged(&self) -> bool {
        self.index != self.head
    }

    /// Changed on disk or staged: what Keep and Mixed leave on disk as it is.
    fn local(&self) -> bool {
        self.staged() || self.disk != Disk::AsStaged
    }
}

/// The file on disk, for `git status`.
#[derive(Clone, Copy, Debug)]
enum OnDisk<'a> {
    Absent,
    Blob(&'a str),
    /// There, its content unknown: changed since it was staged, or untracked.
    Other,
}

fn change(old: Option<&str>, new: Option<&str>) -> Option<Change> {
    match (old, new) {
        (None, Some(_)) => Some(Change::Added),
        (Some(_), None) => Some(Change::Deleted),
        (Some(a), Some(b)) if a != b => Some(Change::Modified),
        _ => None,
    }
}

/// `git status --short` for a path: staged, unstaged, untracked.
fn status(
    head: Option<&str>,
    index: Option<&str>,
    disk: OnDisk,
) -> (Option<Change>, Option<Change>, bool) {
    let staged = change(head, index);
    match (index, disk) {
        (None, OnDisk::Absent) => (staged, None, false),
        (None, _) => (staged, None, true),
        (Some(_), OnDisk::Absent) => (staged, Some(Change::Deleted), false),
        (Some(i), OnDisk::Blob(b)) => (staged, (i != b).then_some(Change::Modified), false),
        (Some(_), OnDisk::Other) => (staged, Some(Change::Modified), false),
    }
}

/// What resetting the open worktree's branch to a commit would do in each mode.
#[derive(Clone, Debug)]
pub struct Preview {
    pub branch: String,
    pub head: Oid,
    pub target: Oid,
    /// Commits the branch leaves behind, and gains.
    pub behind: usize,
    pub ahead: usize,
    /// Commits nothing reaches afterwards: lost in every mode, even when their changes stay
    /// in the files.
    pub commits: Vec<Oid>,
    entries: Vec<Entry>,
    /// Git's refusal of `--keep`, from its dry run.
    keep_refusal: Option<String>,
    /// Parterre's own refusal, of every mode.
    refusal: Option<String>,
}

impl Preview {
    /// Reads the open worktree at `path`.
    pub fn load(path: &Path, target: Oid) -> Result<Preview, Error> {
        let catalog = Catalog::load(path)?;
        Preview::with_catalog(&catalog, target)
    }

    fn with_catalog(catalog: &Catalog, target: Oid) -> Result<Preview, Error> {
        let branch = branch(catalog, target).map_err(Error::Invalid)?.to_owned();
        let head = catalog.head.expect("checked by branch()");
        let git = Git::new(&catalog.root);
        let (h, t) = (head.to_hex(), target.to_hex());
        if git
            .query(&[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{t}^{{commit}}"),
            ])?
            .is_none()
        {
            return Err(Error::Invalid(
                "The commit is no longer in the repository. Reload and try again.".into(),
            ));
        }
        // Files only touched since git last looked count as changed to `--keep`, and to its dry
        // run: refresh first, as `git status` does.
        git.query(&["update-index", "-q", "--refresh"])?;
        let refusal = catalog
            .in_use_elsewhere(&branch)
            .map(|p| format!("Branch {branch} is in use by {}.", p.display()));
        let keep_refusal = match git.run(&["read-tree", "-m", "-u", "-n", &h, &t]) {
            Ok(_) => None,
            Err(GitError::Failed { stderr, .. }) => Some(stderr),
            Err(e) => return Err(e.into()),
        };
        let counts = git.run(&["rev-list", "--left-right", "--count", &format!("{h}...{t}")])?;
        let mut counts = counts.split_whitespace().map(|n| n.parse().unwrap_or(0));
        let (behind, ahead) = (counts.next().unwrap_or(0), counts.next().unwrap_or(0));
        let commits = lost_commits(
            &git,
            catalog,
            head,
            Some(&format!("refs/heads/{branch}")),
            Some(&catalog.root),
            Some(target),
        )?;
        let entries = entries(&git, &catalog.root, &h, &t)?;
        Ok(Preview {
            branch,
            head,
            target,
            behind,
            ahead,
            commits,
            entries,
            keep_refusal,
            refusal,
        })
    }

    /// Parterre's own refusal, of every mode: the branch is in use in another worktree too.
    pub fn blocked(&self) -> Option<&str> {
        self.refusal.as_deref()
    }

    /// Why `mode` can't run: git's refusal, or parterre's.
    pub fn refusal(&self, mode: Mode) -> Option<&str> {
        self.refusal.as_deref().or(match mode {
            Mode::Keep => self.keep_refusal.as_deref(),
            _ => None,
        })
    }

    /// Every file the reset concerns, as `mode` leaves it, in path order.
    pub fn files(&self, mode: Mode) -> Vec<FileOutcome> {
        let refusal = self.refusal(mode);
        self.entries
            .iter()
            .map(|e| match refusal {
                Some(r) => unchanged(e, r),
                None => outcome(e, mode),
            })
            .collect()
    }

    /// The paths whose changes `mode` loses.
    pub fn lost_files(&self, mode: Mode) -> Vec<String> {
        self.files(mode)
            .into_iter()
            .filter(|f| f.lost.is_some())
            .map(|f| f.path)
            .collect()
    }

    pub fn loses(&self, mode: Mode) -> bool {
        !self.commits.is_empty() || !self.lost_files(mode).is_empty()
    }

    /// The first of Keep, Mixed and Soft that runs and loses nothing, else Keep.
    pub fn default_mode(&self) -> Mode {
        [Mode::Keep, Mode::Mixed, Mode::Soft]
            .into_iter()
            .find(|&m| self.refusal(m).is_none() && !self.loses(m))
            .unwrap_or(Mode::Keep)
    }

    /// The reset in `mode`, with what it loses now.
    pub fn reset(&self, mode: Mode) -> Reset {
        Reset {
            branch: self.branch.clone(),
            head: self.head,
            target: self.target,
            mode,
            commits: self.commits.clone(),
            files: self.lost_files(mode),
        }
    }

    /// The file as a changed-files row: the lines `mode` changes on disk.
    pub fn changed_file(&self, outcome: &FileOutcome) -> ChangedFile {
        let spec = self.diff(outcome);
        let (added, removed) = match outcome.lines {
            Some((a, r)) => (Some(a), Some(r)),
            None => (None, None),
        };
        ChangedFile {
            path: outcome.path.clone(),
            old_path: None,
            status: spec.status,
            modes: spec.modes,
            added,
            removed,
        }
    }

    /// The diff to show for a file: for one `mode` rewrites, the file now against the target's;
    /// otherwise what stays uncommitted afterwards, the target's against the file.
    pub fn diff(&self, outcome: &FileOutcome) -> FileDiffSpec {
        let e = self
            .entries
            .iter()
            .find(|e| e.path == outcome.path)
            .expect("an outcome of this preview");
        let on_disk = match e.disk {
            Disk::AsStaged => e.index.is_some(),
            Disk::Modified | Disk::Untracked => true,
            Disk::Deleted => false,
        };
        let in_target = e.target.is_some();
        let (old, new, from, to) = if outcome.updated {
            (
                on_disk,
                in_target,
                Rev::WorkingTree,
                Rev::Commit(self.target),
            )
        } else {
            (
                in_target,
                on_disk,
                Rev::Commit(self.target),
                Rev::WorkingTree,
            )
        };
        let status = match (old, new) {
            (false, _) => FileStatus::Added,
            (_, false) => FileStatus::Deleted,
            _ => FileStatus::Modified,
        };
        let file = ChangedFile {
            path: e.path.clone(),
            old_path: None,
            status,
            modes: [if old { e.mode } else { 0 }, if new { e.mode } else { 0 }],
            added: outcome.lines.map(|l| l.0),
            removed: outcome.lines.map(|l| l.1),
        };
        FileDiffSpec::between(Some(from), to, &file)
    }

    /// What the reset does to the branch: "Moves main back 2 commits from 12a207f to 600e826."
    pub fn movement(&self, abbrev: usize) -> String {
        let (b, from, to) = (
            &self.branch,
            self.head.short(abbrev),
            self.target.short(abbrev),
        );
        match (self.behind, self.ahead) {
            (n, 0) => format!(
                "Moves {b} back {} from {from} to {to}.",
                plural(n, "commit")
            ),
            (0, n) => format!(
                "Moves {b} forward {} from {from} to {to}.",
                plural(n, "commit")
            ),
            (l, g) => format!(
                "Moves {b} from {from} to {to}, on another line: it leaves {} behind and gains {g}.",
                plural(l, "commit")
            ),
        }
    }

    /// What `mode` does here, in plain words.
    pub fn help(&self, mode: Mode, abbrev: usize) -> String {
        let (b, to) = (&self.branch, self.target.short(abbrev));
        let n = self.behind;
        let last = if n == 1 {
            "the last commit".to_owned()
        } else {
            format!("the last {n} commits")
        };
        let its = if n == 1 { "its" } else { "their" };
        let back = self.ahead == 0;
        match mode {
            Mode::Soft if back => format!("Undo {last}, keeping {its} changes staged."),
            Mode::Mixed if back => format!("Undo {last}, keeping {its} changes as unstaged edits."),
            Mode::Keep if back => {
                format!("Undo {last} and drop {its} changes, but keep your uncommitted changes.")
            }
            Mode::Hard if back => {
                format!("Undo {last} and drop {its} changes, and all your uncommitted changes.")
            }
            Mode::Soft => {
                format!("Move {b} to {to}; your files and what's staged stay as they are.")
            }
            Mode::Mixed => {
                format!("Move {b} to {to}; your files stay as they are, nothing stays staged.")
            }
            Mode::Keep => {
                format!("Move {b} to {to} and update your files, keeping your uncommitted changes.")
            }
            Mode::Hard => {
                format!("Make your files exactly {to}, dropping all uncommitted changes.")
            }
        }
    }
}

/// A file as it is now, for a mode that doesn't run.
fn unchanged(e: &Entry, refusal: &str) -> FileOutcome {
    let disk = match e.disk {
        Disk::AsStaged => e.index.as_deref().map_or(OnDisk::Absent, OnDisk::Blob),
        Disk::Deleted => OnDisk::Absent,
        Disk::Modified | Disk::Untracked => OnDisk::Other,
    };
    let (staged, unstaged, untracked) = status(e.head.as_deref(), e.index.as_deref(), disk);
    FileOutcome {
        path: e.path.clone(),
        staged,
        unstaged,
        untracked,
        updated: false,
        lost: None,
        refused: refusal.contains(&format!("'{}'", e.path)),
        lines: Some((0, 0)),
    }
}

/// A file as `mode` leaves it: HEAD at the target; Soft keeps the index and the files, Mixed
/// the files, Keep the files with uncommitted changes, and Hard nothing.
fn outcome(e: &Entry, mode: Mode) -> FileOutcome {
    let (target, index) = (e.target.as_deref(), e.index.as_deref());
    let now = match e.disk {
        Disk::AsStaged => index.map_or(OnDisk::Absent, OnDisk::Blob),
        Disk::Deleted => OnDisk::Absent,
        Disk::Modified | Disk::Untracked => OnDisk::Other,
    };
    let as_target = target.map_or(OnDisk::Absent, OnDisk::Blob);
    let ((staged, unstaged, untracked), updated) = match mode {
        Mode::Soft => (status(target, index, now), false),
        Mode::Mixed => (status(target, target, now), false),
        Mode::Keep if e.local() => (status(target, target, now), false),
        Mode::Keep => (status(target, target, as_target), e.head != e.target),
        Mode::Hard => (status(target, target, as_target), e.to_target.is_some()),
    };
    // A staged version is lost when it differs from the file on disk and the target's, and
    // the index goes; changes on disk when Hard writes over them.
    let staged_lost = e.staged()
        && index.is_some()
        && index != target
        && match mode {
            Mode::Soft => false,
            Mode::Mixed | Mode::Keep => e.disk != Disk::AsStaged,
            Mode::Hard => true,
        };
    let changes_lost = mode == Mode::Hard
        && matches!(e.disk, Disk::Modified | Disk::Untracked)
        && e.to_target.is_some();
    let lost = match (staged_lost, changes_lost) {
        _ if changes_lost && e.disk == Disk::Untracked => Some(Lost::Untracked),
        (true, true) => Some(Lost::StagedVersionAndChanges),
        (true, false) => Some(Lost::StagedVersion),
        (false, true) => Some(Lost::Changes),
        (false, false) => None,
    };
    FileOutcome {
        path: e.path.clone(),
        staged,
        unstaged,
        untracked,
        updated,
        lost,
        refused: false,
        lines: if updated {
            e.to_target.unwrap_or(Some((0, 0)))
        } else {
            Some((0, 0))
        },
    }
}

/// A `--raw -z` entry: modes, blob ids (`None` where the side is missing or not hashed, as the
/// working tree's) and path.
struct Raw {
    modes: [u32; 2],
    blobs: [Option<String>; 2],
    path: String,
}

fn raw(out: &str) -> Result<Vec<Raw>, Error> {
    let bad = || Error::Invalid("Unexpected git diff output.".into());
    let mut fields = out.split('\0');
    let mut entries = Vec::new();
    while let Some(meta) = fields.next() {
        if meta.is_empty() {
            continue;
        }
        let path = fields.next().ok_or_else(bad)?;
        let parts: Vec<&str> = meta.trim_start_matches(':').split(' ').collect();
        let [old_mode, new_mode, old, new, _status] = parts[..] else {
            return Err(bad());
        };
        let mode = |m: &str| u32::from_str_radix(m, 8).map_err(|_| bad());
        let blob = |b: &str| (!b.bytes().all(|c| c == b'0')).then(|| b.to_owned());
        let modes = [mode(old_mode)?, mode(new_mode)?];
        entries.push(Raw {
            blobs: [
                blob(old).filter(|_| modes[0] != 0),
                blob(new).filter(|_| modes[1] != 0),
            ],
            modes,
            path: path.to_owned(),
        });
    }
    Ok(entries)
}

/// `--numstat -z`, by path; `None` lines for a binary file.
fn numstat(out: &str) -> HashMap<String, Option<(u32, u32)>> {
    out.split('\0')
        .filter_map(|e| {
            let mut parts = e.splitn(3, '\t');
            let (a, r, p) = (parts.next()?, parts.next()?, parts.next()?);
            Some((p.to_owned(), a.parse().ok().zip(r.parse().ok())))
        })
        .collect()
}

/// The paths a reset concerns: those the commits moved away from changed, staged and modified
/// files, and untracked files in the target's way.
fn entries(git: &Git, root: &Path, h: &str, t: &str) -> Result<Vec<Entry>, Error> {
    let changed = raw(&git.run(&["diff-tree", "-r", "--raw", "-z", "--no-abbrev", h, t])?)?;
    let staged = raw(&git.run(&[
        "diff",
        "--cached",
        "--raw",
        "-z",
        "--no-renames",
        "--no-abbrev",
        h,
        "--",
    ])?)?;
    let unstaged = raw(&git.run(&["diff", "--raw", "-z", "--no-renames", "--no-abbrev", "--"])?)?;
    let untracked = git.run(&["ls-files", "--others", "--exclude-standard", "-z"])?;
    let to_target =
        numstat(&git.run(&["diff", "--numstat", "-z", "--no-renames", "-R", t, "--"])?);

    let mut map: HashMap<String, Entry> = HashMap::new();
    for r in &changed {
        let e = map.entry(r.path.clone()).or_insert_with(|| Entry {
            path: r.path.clone(),
            head: r.blobs[0].clone(),
            index: r.blobs[0].clone(),
            disk: Disk::AsStaged,
            target: r.blobs[1].clone(),
            mode: if r.modes[1] != 0 {
                r.modes[1]
            } else {
                r.modes[0]
            },
            to_target: None,
        });
        e.target = r.blobs[1].clone();
    }
    for r in &staged {
        let e = map.entry(r.path.clone()).or_insert_with(|| Entry {
            path: r.path.clone(),
            head: r.blobs[0].clone(),
            index: r.blobs[0].clone(),
            disk: Disk::AsStaged,
            target: r.blobs[0].clone(),
            mode: if r.modes[1] != 0 {
                r.modes[1]
            } else {
                r.modes[0]
            },
            to_target: None,
        });
        e.index = r.blobs[1].clone();
    }
    for r in &unstaged {
        let e = map.entry(r.path.clone()).or_insert_with(|| Entry {
            path: r.path.clone(),
            head: r.blobs[0].clone(),
            index: r.blobs[0].clone(),
            disk: Disk::AsStaged,
            target: r.blobs[0].clone(),
            mode: r.modes[0],
            to_target: None,
        });
        e.disk = if r.modes[1] == 0 {
            Disk::Deleted
        } else {
            Disk::Modified
        };
    }
    for path in in_the_way(git, t, untracked.split('\0').filter(|p| !p.is_empty()))? {
        let e = map.entry(path.clone()).or_insert_with(|| Entry {
            path: path.clone(),
            head: None,
            index: None,
            disk: Disk::AsStaged,
            target: None,
            mode: 0o100644,
            to_target: None,
        });
        e.disk = Disk::Untracked;
        e.to_target = overwritten(git, root, t, &path, e.target.is_some())?;
    }
    for e in map.values_mut() {
        if e.disk != Disk::Untracked {
            e.to_target = to_target.get(&e.path).copied();
        }
    }
    let mut entries: Vec<Entry> = map.into_values().collect();
    entries.sort_by(|a, b| compare_paths(&a.path, &b.path));
    Ok(entries)
}

/// The untracked files a reset to `t` writes over: where the target has something at their
/// path, or a file where they have a folder.
fn in_the_way<'a>(
    git: &Git,
    t: &str,
    untracked: impl Iterator<Item = &'a str>,
) -> Result<Vec<String>, Error> {
    let untracked: Vec<&str> = untracked.collect();
    if untracked.is_empty() {
        return Ok(Vec::new());
    }
    // Each path, then each of its folders: one question per line, so a path with a line break
    // in it can't be asked about, and counts as in the way.
    let mut asked: Vec<(usize, bool, String)> = Vec::new();
    for (i, path) in untracked.iter().enumerate() {
        if path.contains('\n') {
            continue;
        }
        asked.push((i, true, (*path).to_owned()));
        let mut at = 0;
        while let Some(slash) = path[at..].find('/') {
            at += slash;
            asked.push((i, false, path[..at].to_owned()));
            at += 1;
        }
    }
    let input: String = asked.iter().map(|(_, _, p)| format!("{t}:{p}\n")).collect();
    let out = git.run_with_input(&["cat-file", "--batch-check=%(objecttype)"], input)?;
    let mut way = vec![false; untracked.len()];
    for (path, _) in untracked
        .iter()
        .enumerate()
        .filter(|(_, p)| p.contains('\n'))
    {
        way[path] = true;
    }
    for ((i, whole, _), answer) in asked.iter().zip(out.lines()) {
        let kind = answer.trim();
        if kind.ends_with(" missing") {
            continue;
        }
        if *whole || kind == "blob" {
            way[*i] = true;
        }
    }
    Ok(untracked
        .iter()
        .zip(way)
        .filter(|(_, w)| *w)
        .map(|(p, _)| (*p).to_owned())
        .collect())
}

/// Lines added and removed when the target writes over an untracked file (`None` inside when
/// either is binary), or `None` when it writes the same.
fn overwritten(
    git: &Git,
    root: &Path,
    t: &str,
    path: &str,
    in_target: bool,
) -> Result<Option<Option<(u32, u32)>>, Error> {
    let file = root.join(path);
    let now = match std::fs::read(&file) {
        Ok(bytes) => bytes,
        // A folder where the target has a file, or gone since.
        Err(_) if !file.is_file() => return Ok(Some(None)),
        Err(e) => return Err(io_error(&file, e)),
    };
    let after = if in_target {
        git.run_bytes(&["cat-file", "blob", &format!("{t}:{path}")])?
    } else {
        Vec::new()
    };
    if now == after {
        return Ok(None);
    }
    if now.contains(&0) || after.contains(&0) {
        return Ok(Some(None));
    }
    let diff = FileDiff::new(
        &String::from_utf8_lossy(&now),
        &String::from_utf8_lossy(&after),
        DiffOptions::default(),
    );
    Ok(Some(Some((diff.added, diff.removed))))
}

/// Runs `reset` if the branch is still where it was and the reset still loses what was agreed
/// to; git has the last word on what it refuses.
pub(crate) fn execute(
    catalog: &Catalog,
    reset: &Reset,
    cancel: &Cancel,
    report: &mut Report,
) -> Result<(), Error> {
    if catalog.current.as_deref() != Some(reset.branch.as_str()) || catalog.head != Some(reset.head)
    {
        return Err(Error::Invalid(format!(
            "Branch {} moved, or is no longer checked out here. Reload and try again.",
            reset.branch
        )));
    }
    let preview = Preview::with_catalog(catalog, reset.target)?;
    if let Some(refusal) = &preview.refusal {
        return Err(Error::Invalid(refusal.clone()));
    }
    if preview.reset(reset.mode) != *reset {
        return Err(Error::Invalid(
            "What the reset would lose changed. Review it again.".into(),
        ));
    }
    let git = Git::new(&catalog.root);
    let args = command(reset.mode, reset.target);
    if !run(&git, args, cancel, report)? {
        return Err(Error::Failed(report.steps.last().unwrap().output.clone()));
    }
    Ok(())
}

fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(head: Option<&str>, index: Option<&str>, disk: Disk, target: Option<&str>) -> Entry {
        Entry {
            path: "f".into(),
            head: head.map(str::to_owned),
            index: index.map(str::to_owned),
            disk,
            target: target.map(str::to_owned),
            mode: 0o100644,
            to_target: Some(Some((1, 1))),
        }
    }

    /// The correction on "Research: moving a branch pointer" (#170): a file staged as S and
    /// edited to W since loses S in Mixed, Keep and Hard, and keeps it in Soft.
    #[test]
    fn a_partially_staged_file_loses_its_staged_version_unless_soft() {
        let e = entry(Some("h"), Some("s"), Disk::Modified, Some("h"));
        let lost: Vec<_> = Mode::ALL.map(|m| outcome(&e, m).lost).into();
        assert_eq!(
            lost,
            [
                None,
                Some(Lost::StagedVersion),
                Some(Lost::StagedVersion),
                Some(Lost::StagedVersionAndChanges)
            ]
        );
    }

    #[test]
    fn a_staged_version_the_target_has_is_not_lost() {
        let e = entry(Some("h"), Some("t"), Disk::Modified, Some("t"));
        assert_eq!(outcome(&e, Mode::Mixed).lost, None);
    }

    #[test]
    fn a_staged_new_file_becomes_untracked_in_mixed_and_is_kept() {
        let e = entry(None, Some("n"), Disk::AsStaged, None);
        let mixed = outcome(&e, Mode::Mixed);
        assert!(mixed.untracked && mixed.lost.is_none());
        assert_eq!(outcome(&e, Mode::Soft).staged, Some(Change::Added));
        assert_eq!(outcome(&e, Mode::Hard).lost, Some(Lost::StagedVersion));
    }

    #[test]
    fn an_untracked_file_in_the_way_is_staged_as_deleted_by_soft() {
        let e = entry(None, None, Disk::Untracked, Some("t"));
        let soft = outcome(&e, Mode::Soft);
        assert_eq!((soft.staged, soft.untracked), (Some(Change::Deleted), true));
        assert_eq!(outcome(&e, Mode::Hard).lost, Some(Lost::Untracked));
    }

    #[test]
    fn words_say_what_a_file_is_afterwards_and_what_it_loses() {
        let e = entry(Some("h"), Some("s"), Disk::Modified, Some("h"));
        assert_eq!(
            outcome(&e, Mode::Mixed).words(),
            "Modified, not staged; lost: staged version"
        );
        assert_eq!(
            outcome(&e, Mode::Soft).words(),
            "Modified, staged; modified since"
        );
        let clean = entry(Some("h"), Some("h"), Disk::AsStaged, Some("t"));
        assert_eq!(outcome(&clean, Mode::Keep).words(), "Updated");
    }

    #[test]
    fn raw_entries_read_missing_sides_as_none() {
        let out = ":000000 100644 0000000000000000000000000000000000000000 \
                   1111111111111111111111111111111111111111 A\0new file\0";
        let r = raw(out).unwrap();
        assert_eq!(r[0].path, "new file");
        assert_eq!(r[0].blobs[0], None);
        assert_eq!(r[0].modes, [0, 0o100644]);
    }
}
