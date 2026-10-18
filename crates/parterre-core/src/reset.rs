//! Git decides what a reset refuses: `read-tree -m -u -n` is its own dry run of `reset --keep`.
//! The rest is predicted from status, diff and the target's tree, to show the modes side by
//! side before any of them runs.

use std::collections::HashMap;
use std::path::Path;

use crate::Oid;
use crate::branches::{Catalog, Error, Report, io_error, lost_commits};
use crate::changed_files::{ChangedFile, FileStatus, compare_paths};
use crate::file_diff::{DiffOptions, FileDiff, FileDiffSpec, Rev};
use crate::git::{Git, GitError};
use crate::operation::{refuse_if_moved, run_or_fail, verify_commit};
use crate::text::{capitalized, plural};
use parterre_util::CancelTree;

/// Without `--merge`, which is for aborting a merge: that's left to the command line.
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reset {
    pub branch: String,
    pub head: Oid,
    pub target: Oid,
    pub mode: Mode,
    pub lost_commits: Vec<Oid>,
    pub lost_files: Vec<String>,
}

pub fn command(mode: Mode, target: Oid) -> Vec<String> {
    vec!["reset".into(), mode.flag().into(), target.to_hex()]
}

/// The branch a reset to `target` would move, or why there's no reset to offer.
pub fn branch(catalog: &Catalog, target: Oid) -> Result<&str, String> {
    branch_and_head(catalog, target).map(|(branch, _)| branch)
}

fn branch_and_head(catalog: &Catalog, target: Oid) -> Result<(&str, Oid), String> {
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
    if let Some(operation) = catalog
        .worktrees
        .iter()
        .find(|w| w.open)
        .and_then(|w| w.in_progress)
    {
        return Err(format!(
            "{} is in progress. Finish or abort it first.",
            capitalized(operation.name())
        ));
    }
    Ok((branch, head))
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lost {
    StagedVersion,
    Changes,
    StagedVersionAndChanges,
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
    pub staged: Option<Change>,
    pub unstaged: Option<Change>,
    pub untracked: bool,
    /// Its file on disk is rewritten.
    pub updated: bool,
    pub lost: Option<Lost>,
    /// Git names it in its refusal.
    pub refused: bool,
    /// Lines added and removed on disk; `None` for a binary file.
    pub lines: Option<(u32, u32)>,
}

impl FileOutcome {
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
    /// Or absent, where nothing is staged.
    AsStaged,
    Modified,
    Deleted,
    Untracked,
}

/// The file on disk against the target's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToTarget {
    Same,
    /// Lines added and removed; `None` for a binary file.
    Differs(Option<(u32, u32)>),
}

impl ToTarget {
    fn differs(self) -> bool {
        self != ToTarget::Same
    }
}

/// One path the reset concerns, as it is now. Versions are blob ids.
#[derive(Clone, Debug)]
struct Entry {
    path: String,
    head: Option<String>,
    index: Option<String>,
    disk: Disk,
    target: Option<String>,
    mode: u32,
    to_target: ToTarget,
}

impl Entry {
    /// As HEAD has it, staged and on disk, and in the target too.
    fn new(path: &str, blob: Option<String>, mode: u32) -> Entry {
        Entry {
            path: path.to_owned(),
            head: blob.clone(),
            index: blob.clone(),
            disk: Disk::AsStaged,
            target: blob,
            mode,
            to_target: ToTarget::Same,
        }
    }

    fn is_staged(&self) -> bool {
        self.index != self.head
    }

    fn has_local_changes(&self) -> bool {
        self.is_staged() || self.disk != Disk::AsStaged
    }

    fn on_disk(&self) -> OnDisk<'_> {
        match self.disk {
            Disk::AsStaged => self.index.as_deref().map_or(OnDisk::Absent, OnDisk::Blob),
            Disk::Deleted => OnDisk::Absent,
            Disk::Modified | Disk::Untracked => OnDisk::Other,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum OnDisk<'a> {
    Absent,
    Blob(&'a str),
    /// Changed since it was staged, or untracked: its blob id is unknown.
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

#[derive(Clone, Debug)]
pub struct Preview {
    pub branch: String,
    pub head: Oid,
    pub target: Oid,
    pub behind: usize,
    pub ahead: usize,
    /// Lost in every mode, even when their changes stay in the files.
    pub lost_commits: Vec<Oid>,
    entries: Vec<Entry>,
    keep_refusal: Option<String>,
    refusal: Option<String>,
}

impl Preview {
    pub fn load(path: &Path, target: Oid) -> Result<Preview, Error> {
        let catalog = Catalog::load(path)?;
        Preview::with_catalog(&catalog, target)
    }

    fn with_catalog(catalog: &Catalog, target: Oid) -> Result<Preview, Error> {
        let (branch, head) = branch_and_head(catalog, target).map_err(Error::Invalid)?;
        let branch = branch.to_owned();
        let git = Git::new(&catalog.root);
        verify_commit(&git, target)?;
        let (head_hex, target_hex) = (head.to_hex(), target.to_hex());
        // Files only touched since git last looked count as changed to `--keep`, and to its dry
        // run: refresh first, as `git status` does.
        git.query(&["update-index", "-q", "--refresh"])?;
        let refusal = catalog
            .in_use_elsewhere(&branch)
            .map(|p| format!("Branch {branch} is in use by {}.", p.display()));
        let keep_refusal = match git.run(&["read-tree", "-m", "-u", "-n", &head_hex, &target_hex]) {
            Ok(_) => None,
            Err(GitError::Failed { stderr, .. }) => Some(stderr),
            Err(e) => return Err(e.into()),
        };
        let counts = git.run(&[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{head_hex}...{target_hex}"),
        ])?;
        let mut counts = counts.split_whitespace().map(|n| n.parse().unwrap_or(0));
        let (behind, ahead) = (counts.next().unwrap_or(0), counts.next().unwrap_or(0));
        let lost_commits = lost_commits(
            &git,
            catalog,
            head,
            &[format!("refs/heads/{branch}")],
            &[catalog.root.as_path()],
            Some(target),
        )?;
        let entries = entries(&git, &catalog.root, &head_hex, &target_hex)?;
        Ok(Preview {
            branch,
            head,
            target,
            behind,
            ahead,
            lost_commits,
            entries,
            keep_refusal,
            refusal,
        })
    }

    pub fn blocked(&self) -> Option<&str> {
        self.refusal.as_deref()
    }

    pub fn refusal(&self, mode: Mode) -> Option<&str> {
        self.refusal.as_deref().or(match mode {
            Mode::Keep => self.keep_refusal.as_deref(),
            _ => None,
        })
    }

    /// In path order.
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

    pub fn lost_files(&self, mode: Mode) -> Vec<String> {
        self.files(mode)
            .into_iter()
            .filter(|f| f.lost.is_some())
            .map(|f| f.path)
            .collect()
    }

    pub fn loses(&self, mode: Mode) -> bool {
        !self.lost_commits.is_empty() || !self.lost_files(mode).is_empty()
    }

    pub fn default_mode(&self) -> Mode {
        [Mode::Keep, Mode::Mixed, Mode::Soft]
            .into_iter()
            .find(|&m| self.refusal(m).is_none() && !self.loses(m))
            .unwrap_or(Mode::Keep)
    }

    pub fn reset(&self, mode: Mode) -> Reset {
        Reset {
            branch: self.branch.clone(),
            head: self.head,
            target: self.target,
            mode,
            lost_commits: self.lost_commits.clone(),
            lost_files: self.lost_files(mode),
        }
    }

    pub fn changed_file(&self, outcome: &FileOutcome) -> ChangedFile {
        self.compared(outcome).0
    }

    /// For a file the mode rewrites, the file now against the target's; otherwise what stays
    /// uncommitted afterwards, the target's against the file.
    pub fn diff(&self, outcome: &FileOutcome) -> FileDiffSpec {
        let (file, from, to) = self.compared(outcome);
        FileDiffSpec::between(Some(from), to, &file)
    }

    fn compared(&self, outcome: &FileOutcome) -> (ChangedFile, Rev, Rev) {
        let e = self
            .entries
            .iter()
            .find(|e| e.path == outcome.path)
            .expect("an outcome of this preview");
        let on_disk = !matches!(e.on_disk(), OnDisk::Absent);
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
        (file, from, to)
    }

    pub fn movement(&self, abbrev: usize) -> String {
        let (branch, from, to) = (
            &self.branch,
            self.head.short(abbrev),
            self.target.short(abbrev),
        );
        match (self.behind, self.ahead) {
            (n, 0) => format!(
                "Moves {branch} back {} from {from} to {to}.",
                plural(n, "commit")
            ),
            (0, n) => format!(
                "Moves {branch} forward {} from {from} to {to}.",
                plural(n, "commit")
            ),
            (left, gained) => format!(
                "Moves {branch} from {from} to {to}, on another line: it leaves {} behind and \
                 gains {gained}.",
                plural(left, "commit")
            ),
        }
    }

    pub fn help(&self, mode: Mode, abbrev: usize) -> String {
        let (branch, to) = (&self.branch, self.target.short(abbrev));
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
                format!("Move {branch} to {to}; your files and what's staged stay as they are.")
            }
            Mode::Mixed => {
                format!("Move {branch} to {to}; your files stay as they are, nothing stays staged.")
            }
            Mode::Keep => format!(
                "Move {branch} to {to} and update your files, keeping your uncommitted changes."
            ),
            Mode::Hard => {
                format!("Make your files exactly {to}, dropping all uncommitted changes.")
            }
        }
    }
}

fn unchanged(e: &Entry, refusal: &str) -> FileOutcome {
    let (staged, unstaged, untracked) = status(e.head.as_deref(), e.index.as_deref(), e.on_disk());
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

/// HEAD moves to the target; Soft keeps the index and the files, Mixed the files, Keep the
/// files with uncommitted changes, and Hard nothing.
fn outcome(e: &Entry, mode: Mode) -> FileOutcome {
    let (target, index) = (e.target.as_deref(), e.index.as_deref());
    let now = e.on_disk();
    let as_target = target.map_or(OnDisk::Absent, OnDisk::Blob);
    let ((staged, unstaged, untracked), updated) = match mode {
        Mode::Soft => (status(target, index, now), false),
        Mode::Mixed => (status(target, target, now), false),
        Mode::Keep if e.has_local_changes() => (status(target, target, now), false),
        Mode::Keep => (status(target, target, as_target), e.head != e.target),
        Mode::Hard => (status(target, target, as_target), e.to_target.differs()),
    };
    // A staged version is lost when it differs from the file on disk and the target's, and
    // the index goes; changes on disk when Hard writes over them.
    let staged_lost = e.is_staged()
        && index.is_some()
        && index != target
        && match mode {
            Mode::Soft => false,
            Mode::Mixed | Mode::Keep => e.disk != Disk::AsStaged,
            Mode::Hard => true,
        };
    let changes_lost = mode == Mode::Hard
        && matches!(e.disk, Disk::Modified | Disk::Untracked)
        && e.to_target.differs();
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
        lines: match e.to_target {
            ToTarget::Differs(lines) if updated => lines,
            _ => Some((0, 0)),
        },
    }
}

/// A `--raw -z` entry. A blob id is `None` where its side is missing or not hashed, as the
/// working tree's.
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

/// The paths the commits moved away from changed, staged and modified files, and untracked
/// files in the target's way.
fn entries(git: &Git, root: &Path, head: &str, target: &str) -> Result<Vec<Entry>, Error> {
    let changed = raw(&git.run(&[
        "diff-tree",
        "-r",
        "--raw",
        "-z",
        "--no-abbrev",
        head,
        target,
    ])?)?;
    let staged = raw(&git.run(&[
        "diff",
        "--cached",
        "--raw",
        "-z",
        "--no-renames",
        "--no-abbrev",
        head,
        "--",
    ])?)?;
    let unstaged = raw(&git.run(&["diff", "--raw", "-z", "--no-renames", "--no-abbrev", "--"])?)?;
    let untracked = git.run(&["ls-files", "--others", "--exclude-standard", "-z"])?;
    let to_target = numstat(&git.run(&[
        "diff",
        "--numstat",
        "-z",
        "--no-renames",
        "-R",
        target,
        "--",
    ])?);

    let newer_mode = |r: &Raw| {
        if r.modes[1] != 0 {
            r.modes[1]
        } else {
            r.modes[0]
        }
    };
    let mut map: HashMap<String, Entry> = HashMap::new();
    for r in &changed {
        let e = map
            .entry(r.path.clone())
            .or_insert_with(|| Entry::new(&r.path, r.blobs[0].clone(), newer_mode(r)));
        e.target = r.blobs[1].clone();
    }
    for r in &staged {
        let e = map
            .entry(r.path.clone())
            .or_insert_with(|| Entry::new(&r.path, r.blobs[0].clone(), newer_mode(r)));
        e.index = r.blobs[1].clone();
    }
    for r in &unstaged {
        let e = map
            .entry(r.path.clone())
            .or_insert_with(|| Entry::new(&r.path, r.blobs[0].clone(), r.modes[0]));
        e.disk = if r.modes[1] == 0 {
            Disk::Deleted
        } else {
            Disk::Modified
        };
    }
    for path in in_the_way(git, target, untracked.split('\0').filter(|p| !p.is_empty()))? {
        let e = map
            .entry(path.clone())
            .or_insert_with(|| Entry::new(&path, None, 0o100644));
        e.disk = Disk::Untracked;
        e.to_target = overwritten(git, root, target, &path, e.target.as_deref())?;
    }
    for e in map.values_mut() {
        if e.disk != Disk::Untracked {
            e.to_target = to_target
                .get(&e.path)
                .map_or(ToTarget::Same, |&lines| ToTarget::Differs(lines));
        }
    }
    let mut entries: Vec<Entry> = map.into_values().collect();
    entries.sort_by(|a, b| compare_paths(&a.path, &b.path));
    Ok(entries)
}

/// The untracked files a reset to `target` writes over: where the target has something at
/// their path, or a file where they have a folder.
fn in_the_way<'a>(
    git: &Git,
    target: &str,
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
        for (slash, _) in path.match_indices('/') {
            asked.push((i, false, path[..slash].to_owned()));
        }
    }
    let input: String = asked
        .iter()
        .map(|(_, _, p)| format!("{target}:{p}\n"))
        .collect();
    let out = git.run_with_input(&["cat-file", "--batch-check=%(objecttype)"], input)?;
    let mut in_way: Vec<bool> = untracked.iter().map(|p| p.contains('\n')).collect();
    for ((i, is_whole_path, _), answer) in asked.iter().zip(out.lines()) {
        let kind = answer.trim();
        if !kind.ends_with(" missing") && (*is_whole_path || kind == "blob") {
            in_way[*i] = true;
        }
    }
    Ok(untracked
        .iter()
        .zip(in_way)
        .filter(|(_, w)| *w)
        .map(|(p, _)| (*p).to_owned())
        .collect())
}

/// `Same` where the target has what git makes of the file on `git add`, so line endings alone
/// (`core.autocrlf`) are no loss.
fn overwritten(
    git: &Git,
    root: &Path,
    target: &str,
    path: &str,
    target_blob: Option<&str>,
) -> Result<ToTarget, Error> {
    let file = root.join(path);
    let now = match std::fs::read(&file) {
        Ok(bytes) => bytes,
        // A folder where the target has a file, or gone since.
        Err(_) if !file.is_file() => return Ok(ToTarget::Differs(None)),
        Err(e) => return Err(io_error(&file, e)),
    };
    if let Some(blob) = target_blob
        && git.run(&["hash-object", "--", path])?.trim() == blob
    {
        return Ok(ToTarget::Same);
    }
    let after = match target_blob {
        Some(_) => git.run_bytes(&["cat-file", "blob", &format!("{target}:{path}")])?,
        None => Vec::new(),
    };
    if now.contains(&0) || after.contains(&0) {
        return Ok(ToTarget::Differs(None));
    }
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace("\r\n", "\n");
    let diff = FileDiff::new(&text(&now), &text(&after), DiffOptions::default());
    Ok(ToTarget::Differs(Some((diff.added, diff.removed))))
}

pub(crate) fn execute(
    catalog: &Catalog,
    reset: &Reset,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<(), Error> {
    refuse_if_moved(catalog, &reset.branch, reset.head)?;
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
    run_or_fail(&git, command(reset.mode, reset.target), cancel, report)
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
            to_target: ToTarget::Differs(Some((1, 1))),
        }
    }

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
