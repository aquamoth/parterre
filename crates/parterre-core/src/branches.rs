//! Local branch operations and their safety checks. No graph visibility or GUI state enters
//! the loss calculation. Git's porcelain remains the final authority on checkout and deletion.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::git::{Git, GitError};
use crate::{Oid, Repo};

#[derive(Clone, Debug)]
pub struct LocalBranch {
    pub name: String,
    pub tip: Oid,
    /// Logical remote/branch name, including an upstream that has not been fetched.
    pub upstream: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RemoteBranch {
    pub name: String,
    pub tip: Oid,
}

/// One of the repository's worktrees, as the worktree tool needs it.
#[derive(Clone, Debug)]
pub struct Worktree {
    /// Its folder, as git lists it.
    pub path: PathBuf,
    /// `None` for an unborn branch.
    pub head: Option<Oid>,
    /// The branch checked out; `None` when detached.
    pub branch: Option<String>,
    pub main: bool,
    /// The worktree this catalogue was loaded from.
    pub open: bool,
    /// The lock's reason (empty when none was given), when it's locked.
    pub locked: Option<String>,
    /// Its folder is gone.
    pub missing: bool,
    /// An operation git left unfinished there, such as "a rebase".
    pub in_progress: Option<&'static str>,
    /// The rebase stopped there, when the operation in progress is one.
    pub rebasing: Option<Rebasing>,
    /// The commit being merged there (`MERGE_HEAD`), when the operation in progress is a merge.
    pub merging: Option<Oid>,
    /// The cherry-pick stopped there, when the operation in progress is one.
    pub picking: Option<Picking>,
    /// The commit being reverted there (`REVERT_HEAD`), when the operation in progress is a
    /// revert.
    pub reverting: Option<Oid>,
}

/// Why the open worktree is stuck.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stuck {
    /// An operation git left unfinished there, such as "a rebase".
    InProgress(&'static str),
    /// Conflicted files with no operation in progress, as a stash that couldn't be put back
    /// cleanly leaves them.
    Conflicts,
}

impl Stuck {
    /// Why an operation isn't offered.
    pub fn reason(self) -> String {
        match self {
            Stuck::InProgress(what) => {
                let mut chars = what.chars();
                let what: String = chars
                    .next()
                    .map(|c| c.to_uppercase().chain(chars).collect())
                    .unwrap_or_default();
                format!("{what} is in progress in this worktree")
            }
            Stuck::Conflicts => "This worktree has conflicted files".into(),
        }
    }
}

/// A rebase git left stopped in a worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rebasing {
    /// The branch being rebased; `None` for a detached HEAD.
    pub branch: Option<String>,
    /// The commit it is being replayed onto. Git records no name for it.
    pub onto: Option<Oid>,
    /// The commits replayed so far, the one it stopped at included, of how many.
    pub done: usize,
    pub total: usize,
}

/// A cherry-pick git left stopped in a worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picking {
    /// The commit it stopped at (`CHERRY_PICK_HEAD`).
    pub commit: Oid,
    /// The commits picked so far, the one it stopped at included, of how many: 1 of 1 when
    /// only one was picked.
    pub done: usize,
    pub total: usize,
}

/// The cherry-pick in a worktree's administrative folder, if one is stopped there at `head`.
/// Git's sequencer keeps the commits still to pick, the one stopped at first, and where HEAD
/// was when it started.
fn picking(git: &Git, admin: &Path, reftable: bool, head: Option<Oid>) -> Option<Picking> {
    let commit = pseudo_ref(admin, reftable, "CHERRY_PICK_HEAD")?;
    let sequencer = admin.join("sequencer");
    let todo = std::fs::read_to_string(sequencer.join("todo")).unwrap_or_default();
    let left = todo
        .lines()
        .filter(|l| l.starts_with("pick ") || l.starts_with("p "))
        .count();
    let start = std::fs::read_to_string(sequencer.join("head"))
        .ok()
        .and_then(|s| Oid::from_hex(s.trim()));
    let (done, total) = match (left, start, head) {
        (1.., Some(start), Some(head)) => {
            let range = format!("{}..{}", start.to_hex(), head.to_hex());
            let picked: usize = git
                .query(&["rev-list", "--count", &range])
                .ok()
                .flatten()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            (picked + 1, picked + left)
        }
        _ => (1, 1),
    };
    Some(Picking {
        commit,
        done,
        total,
    })
}

/// A pseudo-ref such as `CHERRY_PICK_HEAD` in a worktree's administrative folder: a file, or,
/// with a reftable, a ref.
fn pseudo_ref(admin: &Path, reftable: bool, name: &str) -> Option<Oid> {
    if let Ok(text) = std::fs::read_to_string(admin.join(name)) {
        return Oid::from_hex(text.lines().next()?.trim());
    }
    if !reftable {
        return None;
    }
    let found = Git::new(admin)
        .query(&["--git-dir=.", "rev-parse", "--verify", "--quiet", name])
        .ok()
        .flatten()?;
    Oid::from_hex(found.trim())
}

/// The rebase in a worktree's administrative folder, if one is stopped there.
fn rebasing(admin: &Path) -> Option<Rebasing> {
    let (dir, done, total) = if admin.join("rebase-merge").is_dir() {
        (admin.join("rebase-merge"), "msgnum", "end")
    } else if admin.join("rebase-apply").is_dir() {
        (admin.join("rebase-apply"), "next", "last")
    } else {
        return None;
    };
    let read = |name: &str| {
        std::fs::read_to_string(dir.join(name))
            .map(|s| s.trim().to_owned())
            .unwrap_or_default()
    };
    let number = |name: &str| read(name).parse().unwrap_or(0);
    Some(Rebasing {
        branch: read("head-name")
            .strip_prefix("refs/heads/")
            .map(str::to_owned),
        onto: Oid::from_hex(&read("onto")),
        done: number(done),
        total: number(total),
    })
}

impl Worktree {
    /// Its folder's name.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// A fresh catalogue for menus and creation forms. Refresh after repository changes; execution
/// always reads Git again, rather than trusting the catalogue the user first saw.
#[derive(Clone, Debug)]
pub struct Catalog {
    pub locals: Vec<LocalBranch>,
    pub remotes: Vec<RemoteBranch>,
    pub remote_names: Vec<String>,
    /// Each remote's default branch, as its `<remote>/HEAD` names it: `origin/main`.
    pub remote_defaults: Vec<String>,
    pub occupied: HashMap<String, PathBuf>,
    pub current: Option<String>,
    pub head: Option<Oid>,
    pub has_working_tree: bool,
    /// Every worktree, the main one first. Not the main one of a bare repository.
    pub worktrees: Vec<Worktree>,
    /// The main worktree's folder (a bare repository's git dir).
    pub main: PathBuf,
    /// The open worktree's folder.
    pub root: PathBuf,
    /// The open worktree's conflicted (unmerged) files.
    pub conflicted: Vec<String>,
    /// The stash entry parterre set the open worktree's changes aside in before the revert in
    /// progress there, such as `stash@{0}`.
    pub stashed_for_revert: Option<String>,
    auto_setup_rebase: bool,
    roots: Vec<(String, Oid)>,
    heads: Vec<(PathBuf, Oid)>,
    /// Every worktree's use of a branch: checked out, or being rebased or bisected there.
    /// Unlike `occupied`, a branch checked out in two worktrees is in it twice.
    uses: Vec<(String, PathBuf)>,
}

/// `git worktree list --porcelain -z`'s listing, or the older form's made to look the same.
/// Unlike the viewer's compatibility fallback, safety checks propagate listing errors.
fn worktree_listing(git: &Git, root: &Path) -> Result<String, Error> {
    match git.run(&["worktree", "list", "--porcelain", "-z"]) {
        Ok(listing) => Ok(listing),
        Err(GitError::Failed { .. }) => {
            // Git 2.34 lacks -z. Never turn a failed listing into an empty catalogue;
            // retry its older form, rejecting unrecognised fields (e.g. a split path).
            let plain = git.run(&["worktree", "list", "--porcelain"])?;
            if root.to_string_lossy().contains(['\n', '\r'])
                || plain.lines().any(|line| {
                    !matches!(
                        line.split(' ').next(),
                        Some(
                            "" | "worktree"
                                | "HEAD"
                                | "branch"
                                | "bare"
                                | "detached"
                                | "locked"
                                | "prunable"
                        )
                    )
                })
            {
                return Err(Error::Invalid("Cannot safely read worktree paths with this Git version. Update Git to 2.36 or newer.".into()));
            }
            Ok(plain.replace('\n', "\0"))
        }
        Err(error) => Err(error.into()),
    }
}

impl Catalog {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let git = Git::new(path);
        let location = git.location()?;
        let root = location.root().to_owned();
        let has_working_tree = location.work_tree.is_some();
        // The four listings at once rather than one after the other: each is a git start,
        // which on Windows is most of what it costs (#309).
        let (config, listing, conflicted, refs) = std::thread::scope(|s| {
            let config = s.spawn(|| git.config());
            let listing = s.spawn(|| worktree_listing(&git, &root));
            let conflicted = s.spawn(|| {
                // Conflicted files outlive an operation: an autostash or `git stash pop` that
                // conflicted leaves them with none in progress.
                if has_working_tree {
                    git.run(&["diff", "--name-only", "-z", "--diff-filter=U"])
                } else {
                    Ok(String::new())
                }
            });
            let refs = git.run(&["for-each-ref", "--format=%(refname)%00%(objectname)%00%(objecttype)%00%(*objectname)%00%(*objecttype)%00%(upstream:remotename)%00%(upstream:remoteref)%00%(symref)", "refs/heads", "refs/remotes", "refs/tags"]);
            (
                config.join().expect("config read"),
                listing.join().expect("worktree listing"),
                conflicted.join().expect("conflict listing"),
                refs,
            )
        });
        let config = config?;
        let listing = listing?;
        let refs = refs?;
        let conflicted: Vec<String> = conflicted?
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect();
        let auto_setup_rebase = matches!(
            config.get("branch.autoSetupRebase"),
            Some("always" | "remote")
        );
        let mut remote_names = config.remotes();
        // A remote may itself contain '/', so match the longest configured prefix.
        remote_names.sort_by_key(|r| std::cmp::Reverse(r.len()));
        let mut locals = Vec::new();
        let mut remotes = Vec::new();
        let mut remote_defaults = Vec::new();
        let mut roots = Vec::new();
        for line in refs.lines() {
            let f: Vec<_> = line.split('\0').collect();
            if f.len() != 8 {
                return Err(Error::Invalid("Unexpected Git reference listing.".into()));
            }
            let tip = if f[2] == "commit" {
                Some(parse_oid(f[1])?)
            } else if f[4] == "commit" {
                Some(parse_oid(f[3])?)
            } else if f[2] == "tag" {
                git.query(&["rev-parse", "--verify", &format!("{}^{{commit}}", f[0])])?
                    .map(|s| parse_oid(&s))
                    .transpose()?
            } else {
                None
            };
            let Some(tip) = tip else { continue };
            // `origin/HEAD` names the remote's default branch.
            if f[0].starts_with("refs/remotes/")
                && let Some(default) = f[7].strip_prefix("refs/remotes/")
            {
                remote_defaults.push(default.to_owned());
            }
            roots.push((f[0].to_owned(), tip));
            if let Some(name) = f[0].strip_prefix("refs/heads/") {
                let upstream = (!f[5].is_empty() && !f[6].is_empty()).then(|| {
                    format!(
                        "{}/{}",
                        f[5],
                        f[6].strip_prefix("refs/heads/").unwrap_or(f[6])
                    )
                });
                locals.push(LocalBranch {
                    name: name.to_owned(),
                    tip,
                    upstream,
                });
            } else if let Some(name) = f[0].strip_prefix("refs/remotes/")
                && !name.ends_with("/HEAD")
            {
                remotes.push(RemoteBranch {
                    name: name.to_owned(),
                    tip,
                });
            }
        }
        locals.sort_by(|a, b| a.name.cmp(&b.name));
        remotes.sort_by(|a, b| a.name.cmp(&b.name));
        let mut occupied = HashMap::new();
        let mut uses = Vec::new();
        let main_place = listing
            .split('\0')
            .find_map(|s| s.strip_prefix("worktree "))
            .map(PathBuf::from)
            .unwrap_or_else(|| root.clone());
        let common = location.common_dir;
        // Each linked worktree's administrative folder, by the worktree's own folder.
        let mut admins = vec![(main_place.clone(), common.clone())];
        let linked = common.join("worktrees");
        match std::fs::read_dir(&linked) {
            Ok(entries) => {
                for entry in entries {
                    let dir = entry.map_err(|e| io_error(&linked, e))?.path();
                    let backlink = dir.join("gitdir");
                    let text =
                        std::fs::read_to_string(&backlink).map_err(|e| io_error(&backlink, e))?;
                    let path = dir.join(text.trim());
                    if let Some(p) = path.parent() {
                        admins.push((p.to_owned(), dir));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(&linked, e)),
        }
        let reftable = common.join("reftable").is_dir();
        let mut records: Vec<Vec<&str>> = vec![Vec::new()];
        for field in listing.split('\0') {
            if field.is_empty() {
                records.push(Vec::new());
            } else {
                records.last_mut().expect("never empty").push(field);
            }
        }
        // HEAD's branch and commit are in the open worktree's record. Without one to match (a
        // bare repository, or inside a `.git` folder) git is asked (#309).
        let open_record = records.iter().filter(|_| has_working_tree).find(|record| {
            record.iter().any(|f| {
                f.strip_prefix("worktree ")
                    .is_some_and(|p| crate::worktree_folder::same_path(Path::new(p), &root))
            })
        });
        let (current, head) = match open_record {
            Some(record) => (
                record
                    .iter()
                    .find_map(|f| f.strip_prefix("branch refs/heads/"))
                    .map(str::to_owned),
                record
                    .iter()
                    .find_map(|f| f.strip_prefix("HEAD "))
                    .filter(|oid| !oid.bytes().all(|b| b == b'0'))
                    .map(parse_oid)
                    .transpose()?,
            ),
            None => (
                git.query(&["symbolic-ref", "--quiet", "HEAD"])?
                    .map(|s| s.trim_start_matches("refs/heads/").to_owned()),
                git.query(&["rev-parse", "--verify", "HEAD^{commit}"])?
                    .map(|s| parse_oid(&s))
                    .transpose()?,
            ),
        };
        let mut worktrees = Vec::new();
        let mut heads = Vec::new();
        for record in records.iter().filter(|r| !r.is_empty()) {
            let Some(path) = record.iter().find_map(|f| f.strip_prefix("worktree ")) else {
                return Err(Error::Invalid("Unexpected Git worktree listing.".into()));
            };
            let path = PathBuf::from(path);
            let head = record
                .iter()
                .find_map(|f| f.strip_prefix("HEAD "))
                .filter(|oid| !oid.bytes().all(|b| b == b'0'))
                .map(parse_oid)
                .transpose()?;
            if let Some(head) = head {
                heads.push((path.clone(), head));
            }
            let branch = record
                .iter()
                .find_map(|f| f.strip_prefix("branch refs/heads/"))
                .map(str::to_owned);
            if let Some(name) = &branch {
                occupied.insert(name.clone(), path.clone());
                uses.push((name.clone(), path.clone()));
            }
            if record.contains(&"bare") {
                continue;
            }
            let locked = record.iter().find_map(|f| {
                (*f == "locked")
                    .then(String::new)
                    .or_else(|| f.strip_prefix("locked ").map(str::to_owned))
            });
            let admin = admins
                .iter()
                .find(|(p, _)| crate::worktree_folder::same_path(p, &path))
                .map(|(_, a)| a.clone());
            let in_progress = match &admin {
                Some(admin) => in_progress(admin, reftable),
                None => None,
            };
            let rebasing = admin.as_deref().and_then(rebasing);
            let merging = admin.as_deref().and_then(|a| {
                let text = std::fs::read_to_string(a.join("MERGE_HEAD")).ok()?;
                Oid::from_hex(text.lines().next()?.trim())
            });
            let picking = admin
                .as_deref()
                .and_then(|a| picking(&git, a, reftable, head));
            let reverting = admin
                .as_deref()
                .filter(|_| in_progress == Some("a revert"))
                .and_then(|a| pseudo_ref(a, reftable, "REVERT_HEAD"));
            worktrees.push(Worktree {
                main: path == main_place,
                open: crate::worktree_folder::same_path(&path, &root),
                missing: !path.is_dir(),
                path,
                head,
                branch,
                locked,
                in_progress,
                rebasing,
                merging,
                picking,
                reverting,
            });
        }
        for (place, admin) in &admins {
            reservations(admin, place, &mut occupied, &mut uses)?;
        }
        let stashed_for_revert = worktrees
            .iter()
            .find(|w| w.open)
            .and_then(|w| w.reverting)
            .and_then(|oid| crate::revert::stash_entry(&git, oid));
        Ok(Self {
            locals,
            remotes,
            remote_defaults,
            remote_names,
            occupied,
            current,
            head,
            has_working_tree,
            worktrees,
            main: main_place,
            root,
            conflicted,
            stashed_for_revert,
            auto_setup_rebase,
            roots,
            heads,
            uses,
        })
    }

    /// Why the open worktree is stuck, if it is: an operation in progress, or conflicted files.
    /// Until that's sorted out with git, nothing that changes that worktree's HEAD, index or
    /// files is offered.
    pub fn stuck(&self) -> Option<Stuck> {
        let in_progress = self
            .worktrees
            .iter()
            .find(|w| w.open)
            .and_then(|w| w.in_progress);
        match in_progress {
            Some(what) => Some(Stuck::InProgress(what)),
            None if !self.conflicted.is_empty() => Some(Stuck::Conflicts),
            None => None,
        }
    }

    /// A worktree other than the open one that has branch `name` checked out, or is rebasing
    /// or bisecting it. Git 2.34 moves such a branch without a word, leaving that worktree's
    /// index stale, and `git reset` never checks other worktrees in any version.
    pub fn in_use_elsewhere(&self, name: &str) -> Option<&Path> {
        self.uses
            .iter()
            .find(|(n, p)| n == name && !crate::worktree_folder::same_path(p, &self.root))
            .map(|(_, p)| p.as_path())
    }

    pub fn trackers(&self, upstream: &str) -> Vec<&str> {
        self.locals
            .iter()
            .filter(|b| b.upstream.as_deref() == Some(upstream))
            .map(|b| b.name.as_str())
            .collect()
    }

    pub fn tracking_parts<'a>(&'a self, typed: &'a str) -> Option<(&'a str, &'a str)> {
        self.remote_names.iter().find_map(|r| {
            typed
                .strip_prefix(&format!("{r}/"))
                .filter(|b| !b.is_empty())
                .map(|b| (r.as_str(), b))
        })
    }

    pub fn suggested_name(&self, upstream: &str) -> String {
        let Some((_, base)) = self.tracking_parts(upstream) else {
            return String::new();
        };
        if !valid_name(base) {
            return String::new();
        }
        let base = if self
            .locals
            .iter()
            .any(|b| base.starts_with(&format!("{}/", b.name)))
        {
            base.replace('/', "-")
        } else {
            base.to_owned()
        };
        if self.name_error(&base).is_none() {
            return base;
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|n| self.name_error(n).is_none())
            .unwrap()
    }

    pub fn name_error(&self, name: &str) -> Option<&'static str> {
        if !valid_name(name) {
            return Some("Enter a valid Git branch name.");
        }
        if self.locals.iter().any(|b| b.name == name) {
            Some("A local branch with this name already exists.")
        } else if self.locals.iter().any(|b| {
            b.name.starts_with(&format!("{name}/")) || name.starts_with(&format!("{}/", b.name))
        }) {
            Some("This name conflicts with an existing branch path.")
        } else {
            None
        }
    }

    pub fn track_error(&self, track: &str) -> Option<&'static str> {
        if track.trim().is_empty() {
            return None;
        }
        match self.tracking_parts(track.trim()) {
            Some((_, name)) if valid_name(&format!("upstream/{name}")) => None,
            Some(_) => Some("Enter a valid remote branch name."),
            None => {
                Some("Use a configured remote followed by a branch name, such as origin/topic.")
            }
        }
    }
}

/// The operation git left unfinished in a worktree, from its administrative folder. Rebases,
/// merges and bisects always leave files; cherry-picks and reverts leave refs, which a reftable
/// keeps out of the folder.
fn in_progress(admin: &Path, reftable: bool) -> Option<&'static str> {
    if admin.join("rebase-merge").is_dir() {
        return Some("a rebase");
    }
    if admin.join("rebase-apply").is_dir() {
        return Some(if admin.join("rebase-apply/applying").exists() {
            "git am"
        } else {
            "a rebase"
        });
    }
    if admin.join("MERGE_HEAD").is_file() {
        return Some("a merge");
    }
    let pseudo = |name: &str| {
        admin.join(name).is_file()
            || reftable
                && Git::new(admin)
                    .query(&["--git-dir=.", "rev-parse", "--verify", "--quiet", name])
                    .ok()
                    .flatten()
                    .is_some()
    };
    if pseudo("CHERRY_PICK_HEAD") {
        return Some("a cherry-pick");
    }
    if pseudo("REVERT_HEAD") {
        return Some("a revert");
    }
    if admin.join("sequencer").is_dir() {
        return Some("a cherry-pick or revert");
    }
    if admin.join("BISECT_START").is_file() {
        return Some("a bisect");
    }
    None
}

fn reservations(
    dir: &Path,
    worktree: &Path,
    occupied: &mut HashMap<String, PathBuf>,
    uses: &mut Vec<(String, PathBuf)>,
) -> Result<(), Error> {
    for name in [
        "rebase-merge/head-name",
        "rebase-apply/head-name",
        "BISECT_START",
    ] {
        let path = dir.join(name);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let name = text.trim().trim_start_matches("refs/heads/");
                if !name.is_empty() && Oid::from_hex(name).is_none() {
                    occupied.insert(name.to_owned(), worktree.to_owned());
                    uses.push((name.to_owned(), worktree.to_owned()));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(&path, e)),
        }
    }
    Ok(())
}

fn valid_name(name: &str) -> bool {
    // Git's ref-format rules, applied without spawning a process on every UI keystroke.
    !name.is_empty()
        && name != "HEAD"
        && name != "@"
        && !name.starts_with('-')
        && !name.contains("..")
        && !name.contains("@{")
        && !name.ends_with('.')
        && !name
            .chars()
            .any(|c| c <= '\u{20}' || c == '\u{7f}' || "~^:?*[\\".contains(c))
        && name
            .split('/')
            .all(|p| !p.is_empty() && !p.starts_with('.') && !p.ends_with(".lock"))
}

pub(crate) fn parse_oid(s: &str) -> Result<Oid, Error> {
    Oid::from_hex(s).ok_or_else(|| Error::Invalid("Unexpected Git commit id.".into()))
}

pub(crate) fn io_error(path: &Path, source: std::io::Error) -> Error {
    Error::Git(GitError::Read {
        path: path.to_owned(),
        source,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Create {
    pub start: Oid,
    pub name: String,
    /// Empty/None means no upstream; otherwise a configured remote followed by a branch.
    pub track: Option<String>,
    pub switch: bool,
}

/// Creation-field state: the tracking name follows local edits until the user chooses
/// or types a particular remote branch. Selecting a remote starts that automatic mode again.
#[derive(Clone, Debug)]
pub struct CreateDraft {
    name: String,
    remote: Option<String>,
    track_name: String,
    name_edited: bool,
    track_edited: bool,
    suggestion_source: Option<String>,
}

impl CreateDraft {
    pub fn new(catalog: &Catalog, start: Oid, prefer: Option<&str>) -> Self {
        let remote = prefer
            .and_then(|s| catalog.tracking_parts(s))
            .map(|(remote, _)| remote.to_owned())
            .or_else(|| catalog.remote_names.iter().min().cloned());
        let upstream = prefer.or_else(|| {
            catalog
                .remotes
                .iter()
                .find(|r| {
                    r.tip == start
                        && catalog.trackers(&r.name).is_empty()
                        && catalog
                            .tracking_parts(&r.name)
                            .is_some_and(|(r, _)| Some(r) == remote.as_deref())
                })
                .map(|r| r.name.as_str())
        });
        let parts = upstream.and_then(|s| catalog.tracking_parts(s));
        Self {
            name: upstream
                .map(|s| catalog.suggested_name(s))
                .unwrap_or_default(),
            remote,
            track_name: parts
                .map(|(_, branch)| branch.to_owned())
                .unwrap_or_default(),
            name_edited: false,
            // A remote Switch is already an explicit choice of that upstream branch.
            track_edited: prefer.is_some(),
            suggestion_source: upstream.map(str::to_owned),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn remote(&self) -> Option<&str> {
        self.remote.as_deref()
    }
    pub fn track_name(&self) -> &str {
        &self.track_name
    }

    pub fn upstream(&self) -> Option<String> {
        self.remote
            .as_ref()
            .map(|remote| format!("{remote}/{}", self.track_name.trim()))
    }

    pub fn set_name(&mut self, name: String) {
        self.name = name;
        self.name_edited = true;
        self.follow_name();
    }

    pub fn set_remote(&mut self, remote: Option<String>) {
        self.remote = remote;
        self.track_edited = false;
        self.suggestion_source = None;
        self.follow_name();
    }

    /// A selection counts even when it matches the current automatically generated name.
    pub fn set_track_name(&mut self, catalog: &Catalog, branch: String) {
        self.track_name = branch;
        self.track_edited = true;
        self.suggestion_source = self.upstream();
        let suggested = self.suggested_name(catalog);
        if !self.name_edited && !suggested.is_empty() {
            self.name = suggested;
        }
    }

    pub fn can_restore_track_name(&self) -> bool {
        self.remote.is_some() && (self.track_edited || self.track_name != self.name)
    }

    /// Resume following local-name edits without changing the local name or selected remote.
    pub fn restore_track_name(&mut self) {
        self.track_edited = false;
        self.follow_name();
    }

    pub fn suggested_name(&self, catalog: &Catalog) -> String {
        self.suggestion_source
            .as_deref()
            .map(|s| catalog.suggested_name(s))
            .unwrap_or_default()
    }

    pub fn restore_suggested_name(&mut self, catalog: &Catalog) {
        let suggested = self.suggested_name(catalog);
        if !suggested.is_empty() {
            self.name = suggested;
            self.name_edited = false;
            self.follow_name();
        }
    }

    pub fn remote_branches(&self, catalog: &Catalog) -> Vec<String> {
        catalog
            .remotes
            .iter()
            .filter_map(|r| {
                let (remote, branch) = catalog.tracking_parts(&r.name)?;
                (Some(remote) == self.remote()).then(|| branch.to_owned())
            })
            .collect()
    }

    fn follow_name(&mut self) {
        if self.remote.is_none() {
            self.track_name.clear();
        } else if !self.track_edited {
            self.track_name.clone_from(&self.name);
        }
    }
}

/// What a new worktree checks out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Checkout {
    /// A new branch at the start commit, with an optional upstream as in [`Create::track`].
    New { name: String, track: Option<String> },
    /// An existing local branch that no worktree has, whose tip is the start commit.
    Existing(String),
    /// The start commit, detached.
    Detached,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddWorktree {
    pub start: Oid,
    /// The new worktree's folder: absolute, and empty or not there yet.
    pub path: PathBuf,
    pub checkout: Checkout,
}

/// A local branch, at the tip it had when offered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchTip {
    pub name: String,
    pub tip: Oid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Create(Create),
    Switch(String),
    /// Switches the open worktree to a commit, detached.
    Detach(Oid),
    /// Deletes local branches, each still at the tip it was offered at.
    DeleteBranches(Vec<BranchTip>),
    AddWorktree(AddWorktree),
    /// Removes other worktrees from git and deletes their folders, one by one, and then, when
    /// `branches` is set, the local branches they had.
    DeleteWorktrees {
        paths: Vec<PathBuf>,
        branches: bool,
    },
    /// Moves the open worktree's branch, with what the user agreed to lose.
    Reset(Box<crate::reset::Reset>),
    /// Replays the open worktree's branch onto another commit.
    Rebase(Box<crate::rebase::Rebase>),
    /// Merges another commit into the open worktree's branch.
    Merge(Box<crate::merge::Merge>),
    /// Cherry-picks commits onto the open worktree's branch.
    CherryPick(Box<crate::cherry_pick::CherryPick>),
    /// Reverts a commit on the open worktree's branch, with a new commit at its tip.
    Revert(Box<crate::revert::Revert>),
    /// Puts back the changes parterre stashed before an operation, and drops the entry.
    RestoreStash(Oid),
    /// Finishes a conflicted file in the open worktree with one git command.
    Resolve(Box<crate::conflicts::Resolve>),
    /// Fetches every remote, pruning what's gone from them.
    Fetch,
    /// Pulls into the open worktree's branch.
    Pull(Box<crate::remote::Pull>),
    /// Pushes a local branch to its own name on a remote.
    Push(Box<crate::remote::Push>),
    /// Sets a local branch's upstream.
    SetUpstream(Box<crate::remote::SetUpstream>),
    /// Deletes branches on their remotes, each still at the tip it was offered at.
    DeleteRemoteBranches(Vec<crate::remote::RemoteBranchTip>),
}

impl Action {
    pub fn label(&self) -> String {
        match self {
            Self::Create(c) => {
                if c.switch {
                    format!("Create and switch to {}", c.name)
                } else {
                    format!("Create branch {}", c.name)
                }
            }
            Self::Switch(b) => format!("Switch to {b}"),
            Self::Detach(oid) => format!("Switch to {} (detached)", short(*oid)),
            Self::DeleteBranches(branches) => match branches.as_slice() {
                [one] => format!("Delete branch {}", one.name),
                branches => format!("Delete {} local branches", branches.len()),
            },
            Self::AddWorktree(a) => format!("Add worktree {}", folder(&a.path)),
            Self::DeleteWorktrees { paths, .. } => match paths.as_slice() {
                [path] => format!("Delete worktree {}", folder(path)),
                paths => format!("Delete {} worktrees", paths.len()),
            },
            Self::Reset(r) => format!("Reset {} to {}", r.branch, short(r.target)),
            Self::Rebase(r) => format!(
                "Rebase {} onto {}",
                r.branch,
                crate::rebase::short_target(r)
            ),
            Self::Merge(m) => format!("Merge {} into {}", crate::merge::short_target(m), m.branch),
            Self::CherryPick(c) => format!("Cherry-pick {} onto {}", c.name, c.branch),
            Self::Revert(r) => format!("Revert {} in {}", short(r.commit), r.name()),
            Self::RestoreStash(_) => "Restore stashed changes".into(),
            Self::Resolve(r) => r.label(),
            Self::Fetch => "Fetch".into(),
            Self::Pull(p) => format!("Pull {}", p.branch),
            Self::Push(p) => format!("Push {} to {}", p.branch, p.remote),
            Self::SetUpstream(s) => format!("Set upstream of {} to {}", s.branch, s.upstream),
            Self::DeleteRemoteBranches(branches) => match branches.as_slice() {
                [one] => format!("Delete remote branch {}", one.name()),
                branches => format!("Delete {} remote branches", branches.len()),
            },
        }
    }

    /// Fetching, pulling and pushing: they reach a remote, so their output is shown as it
    /// comes, and they can be cancelled.
    pub fn is_network(&self) -> bool {
        matches!(
            self,
            Self::Fetch | Self::Pull(_) | Self::Push(_) | Self::DeleteRemoteBranches(_)
        )
    }
}

fn short(oid: Oid) -> String {
    oid.to_hex()[..7].to_owned()
}

fn folder(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Failed(String),
    #[error("Operation cancelled.")]
    Cancelled,
}

use parterre_util::CancelTree;

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub steps: Vec<Step>,
    /// Done, but with something the user must see to: shown in orange until closed.
    pub attention: Option<Attention>,
    /// The entry parterre stashed the changes in for the operation, left for the user to
    /// restore.
    pub stash: Option<Stashed>,
    /// The commit the operation made, for the log to select.
    pub created: Option<Oid>,
    /// What went wrong is likely out of date: a fetch would show how things stand.
    pub suggest_fetch: bool,
}

/// A stash entry parterre made, by its commit: other worktrees and sessions share the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stashed {
    pub oid: Oid,
    /// Its name when it was made, such as `stash@{0}`, for the command shown. It's found by
    /// its commit when restored.
    pub name: String,
    /// The files in it, by path in the worktree.
    pub files: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Attention {
    pub title: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct Step {
    pub args: Vec<String>,
    pub output: String,
    pub success: bool,
}

/// What an operation would lose, for the user to approve first. The approval is tied to the
/// HEADs concerned, the exact endangered commit ids and changed files. Deleting worktrees always
/// asks: with nothing lost, this is a confirmation.
#[derive(Clone, Debug)]
pub struct Warning {
    pub action: Action,
    /// Every commit lost.
    pub commits: Vec<Oid>,
    /// A force push's: the remote's commits it replaces with copies the branch has.
    pub replaced: Vec<Oid>,
    /// Each branch or worktree the action deletes, in its order, with what it loses. Empty for
    /// a switch.
    pub deletions: Vec<Deletion>,
    pub repo: Arc<Repo>,
    pub commands: Vec<Vec<String>>,
    /// The HEAD the warning was about, or, for a force push, the remote branch's tip.
    pub(crate) head: Option<Oid>,
}

impl Warning {
    /// Nothing is lost; it only confirms.
    pub fn is_confirmation(&self) -> bool {
        self.commits.is_empty()
            && self
                .deletions
                .iter()
                .all(|d| d.files.is_empty() && d.refusal.is_none())
    }

    /// Whether deleting worktrees also deletes their branches.
    pub fn deletes_branches(&self) -> bool {
        matches!(self.action, Action::DeleteWorktrees { branches: true, .. })
    }

    /// Deletes the worktrees' branches too, or not: what's lost and the commands follow.
    pub fn set_deletes_branches(&mut self, on: bool) {
        if let Action::DeleteWorktrees { branches, .. } = &mut self.action {
            *branches = on;
            self.commits = worktree_commits(&self.deletions, on);
            self.commands = worktree_commands(&self.deletions, on);
        }
    }
}

/// A branch or worktree an action deletes, and what deleting it loses.
#[derive(Clone, Debug)]
pub struct Deletion {
    /// The branch's name, or the worktree's folder name.
    pub name: String,
    /// The worktree's folder.
    pub path: Option<PathBuf>,
    /// The lost commits it reaches. Another deletion can reach the same ones.
    pub commits: Vec<Oid>,
    /// Uncommitted changes to files that aren't ignored, by path in its worktree.
    pub files: Vec<String>,
    /// Git's own refusal, when it refused after parterre found nothing at risk.
    pub refusal: Option<String>,
    /// The worktree's local branch, which can go with it.
    pub branch: Option<WorktreeBranch>,
    /// The worktree's HEAD or the branch's tip.
    pub(crate) head: Option<Oid>,
}

impl Deletion {
    /// The same loss, whatever git said before.
    fn same_loss(&self, other: &Self) -> bool {
        self.head == other.head
            && self.commits == other.commits
            && self.files == other.files
            && self.branch == other.branch
    }
}

/// A worktree's local branch, checked out there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeBranch {
    pub name: String,
    pub tip: Oid,
    /// The lost commits it reaches once it and the worktrees are all gone.
    pub commits: Vec<Oid>,
}

/// Approved deletions, as loaded again just before running them.
pub(crate) fn same_losses(approved: &[Deletion], now: &[Deletion]) -> bool {
    approved.len() == now.len() && approved.iter().zip(now).all(|(a, b)| a.same_loss(b))
}

/// Every commit the deletions lose, once.
pub(crate) fn all_commits(deletions: &[Deletion]) -> Vec<Oid> {
    let mut commits: Vec<Oid> = deletions.iter().flat_map(|d| d.commits.clone()).collect();
    commits.sort_by_key(|o| o.to_hex());
    commits.dedup();
    commits
}

#[derive(Debug)]
pub enum Outcome {
    Done(Report),
    Warning(Warning),
    Failed {
        error: Error,
        report: Report,
    },
    /// A pull that asks how to reconcile a diverged branch first.
    Diverged(Box<crate::remote::Diverged>),
}

#[derive(Clone, Debug)]
pub struct Branches {
    path: PathBuf,
    /// Where the network commands' output goes as it comes.
    live: Option<crate::remote::Live>,
}

impl Branches {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            live: None,
        }
    }

    /// The network commands (fetch, pull, push) stream their output to `live`.
    pub fn with_live(mut self, live: crate::remote::Live) -> Self {
        self.live = Some(live);
        self
    }

    /// Commands shown by the form. Execution freezes its start by full OID and validates again.
    pub fn commands(catalog: &Catalog, action: &Action) -> Result<Vec<Vec<String>>, Error> {
        let words = |s: &[&str]| s.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        match action {
            Action::Create(c) => {
                if let Some(error) = catalog.name_error(&c.name) {
                    return Err(Error::Invalid(error.into()));
                }
                let start = c.start.to_hex();
                let create = if c.switch {
                    words(&["switch", "--create", &c.name, "--no-track", &start])
                } else {
                    words(&["branch", "--no-track", "--", &c.name, &start])
                };
                let mut commands = vec![create];
                commands.extend(track_commands(catalog, &c.name, c.track.as_deref())?);
                Ok(commands)
            }
            Action::AddWorktree(a) => {
                let path = a.path.to_string_lossy().into_owned();
                if !a.path.is_absolute() {
                    return Err(Error::Invalid("Choose a folder for the worktree.".into()));
                }
                let start = a.start.to_hex();
                Ok(match &a.checkout {
                    Checkout::New { name, track } => {
                        if let Some(error) = catalog.name_error(name) {
                            return Err(Error::Invalid(error.into()));
                        }
                        let mut commands = vec![words(&[
                            "worktree",
                            "add",
                            "--no-track",
                            "-b",
                            name,
                            "--",
                            &path,
                            &start,
                        ])];
                        commands.extend(track_commands(catalog, name, track.as_deref())?);
                        commands
                    }
                    Checkout::Existing(name) => {
                        vec![words(&["worktree", "add", "--", &path, name])]
                    }
                    Checkout::Detached => {
                        vec![words(&["worktree", "add", "--detach", "--", &path, &start])]
                    }
                })
            }
            Action::DeleteWorktrees { paths, branches } => {
                let mut commands: Vec<_> = paths.iter().map(|p| remove(p, false)).collect();
                let names: Vec<String> = find_worktrees(catalog, paths)?
                    .into_iter()
                    .filter_map(|w| worktree_branch(catalog, w))
                    .map(|b| b.name)
                    .collect();
                if *branches && !names.is_empty() {
                    commands.push(delete_command("-d", names));
                }
                Ok(commands)
            }
            Action::Detach(oid) => Ok(vec![words(&["switch", "--detach", &oid.to_hex()])]),
            Action::Switch(name) => Ok(vec![words(&["switch", "--no-guess", "--", name])]),
            Action::DeleteBranches(branches) => {
                let mut force = words(&["branch", "-D", "--"]);
                force.extend(branches.iter().map(|b| b.name.clone()));
                Ok(vec![force])
            }
            Action::Reset(r) => Ok(vec![crate::reset::command(r.mode, r.target)]),
            Action::Rebase(r) => Ok(vec![crate::rebase::command(r)]),
            Action::Merge(m) => Ok(crate::merge::commands(m)),
            Action::CherryPick(c) => Ok(crate::cherry_pick::commands(c)),
            Action::Revert(r) => Ok(crate::revert::commands(r)),
            Action::RestoreStash(_) => Ok(vec![words(&["stash", "pop"])]),
            Action::Resolve(r) => Ok(r.conflict.commands(r.answer)),
            Action::Fetch => Ok(vec![crate::remote::fetch_command()]),
            Action::Pull(p) => Ok(vec![crate::remote::pull_command(p.how)]),
            Action::Push(p) => Ok(vec![crate::remote::push_command(catalog, p, false)]),
            Action::SetUpstream(s) => Ok(vec![crate::remote::set_upstream_command(s)]),
            Action::DeleteRemoteBranches(branches) => Ok(branches
                .iter()
                .map(crate::remote::delete_remote_command)
                .collect()),
        }
    }

    pub fn execute(
        &self,
        action: Action,
        approval: Option<&Warning>,
        cancel: &CancelTree,
    ) -> Outcome {
        let mut report = Report::default();
        if let Action::Pull(pull) = &action {
            let pulled = Catalog::load(&self.path).and_then(|catalog| {
                crate::remote::pull(&catalog, pull, cancel, &mut report, self.live.as_ref())
            });
            return match pulled {
                Ok(Some(diverged)) => Outcome::Diverged(Box::new(diverged)),
                Ok(None) => Outcome::Done(report),
                Err(error) => Outcome::Failed { error, report },
            };
        }
        let requested = action.clone();
        match self.execute_inner(action, approval, cancel, &mut report) {
            Ok(Some(warning)) => Outcome::Warning(warning),
            Ok(None) => Outcome::Done(report),
            Err(error) => {
                let error = if let Action::Create(c) = &requested
                    && !report.steps.is_empty()
                    && Git::new(&self.path)
                        .query(&["rev-parse", "--verify", &format!("refs/heads/{}", c.name)])
                        .ok()
                        .flatten()
                        .is_some_and(|tip| tip == c.start.to_hex())
                {
                    let checked_out = Git::new(&self.path)
                        .query(&["symbolic-ref", "--quiet", "HEAD"])
                        .ok()
                        .flatten()
                        .is_some_and(|head| head == format!("refs/heads/{}", c.name));
                    let detail = if report.steps.first().is_some_and(|s| s.success) {
                        format!("Configuring its upstream did not finish: {error}")
                    } else {
                        error.to_string()
                    };
                    Error::Failed(format!(
                        "Branch {} now exists{}. {detail}",
                        c.name,
                        if checked_out {
                            " and is checked out"
                        } else {
                            ""
                        }
                    ))
                } else if let Action::AddWorktree(a) = &requested
                    && report.steps.len() > 1
                    && report.steps.first().is_some_and(|s| s.success)
                {
                    Error::Failed(format!(
                        "Worktree {} was added. Configuring its branch's upstream did not finish: {error}",
                        folder(&a.path)
                    ))
                } else {
                    error
                };
                Outcome::Failed { error, report }
            }
        }
    }

    fn execute_inner(
        &self,
        action: Action,
        approval: Option<&Warning>,
        cancel: &CancelTree,
        report: &mut Report,
    ) -> Result<Option<Warning>, Error> {
        let git = Git::new(&self.path);
        let catalog = Catalog::load(&self.path)?;
        if let Action::DeleteBranches(branches) = &action {
            let approved = |head: Option<Oid>, commits: &[Oid]| {
                commits.is_empty()
                    || approval.is_some_and(|w| {
                        w.action == action && w.head == head && w.commits == commits
                    })
            };
            return self.delete_branches(&catalog, branches, &approved, cancel, report);
        }
        if let Action::DeleteWorktrees { paths, branches } = &action {
            return self.delete_worktrees(&catalog, paths, *branches, approval, cancel, report);
        }
        if let Action::Reset(reset) = &action {
            crate::reset::execute(&catalog, reset, cancel, report)?;
            return Ok(None);
        }
        if let Action::Rebase(rebase) = &action {
            crate::rebase::execute(&catalog, rebase, cancel, report)?;
            return Ok(None);
        }
        if let Action::Merge(merge) = &action {
            crate::merge::execute(&catalog, merge, cancel, report)?;
            return Ok(None);
        }
        if let Action::CherryPick(pick) = &action {
            crate::cherry_pick::execute(&catalog, pick, cancel, report)?;
            return Ok(None);
        }
        if let Action::Revert(revert) = &action {
            crate::revert::execute(&catalog, revert, cancel, report)?;
            return Ok(None);
        }
        if let Action::RestoreStash(stash) = &action {
            crate::revert::restore(&catalog, *stash, cancel, report)?;
            return Ok(None);
        }
        if let Action::Resolve(resolve) = &action {
            crate::conflicts::execute(&catalog, resolve, cancel, report)?;
            return Ok(None);
        }
        let live = self.live.as_ref();
        match &action {
            Action::Fetch => {
                crate::remote::fetch(&catalog, cancel, report, live)?;
                return Ok(None);
            }
            Action::Push(push) => {
                return crate::remote::push(
                    &catalog, &action, push, approval, cancel, report, live,
                );
            }
            Action::SetUpstream(set) => {
                crate::remote::set_upstream(&catalog, set, cancel, report)?;
                return Ok(None);
            }
            Action::DeleteRemoteBranches(branches) => {
                return crate::remote::delete_remote_branches(
                    &catalog, &action, branches, approval, cancel, report, live,
                );
            }
            _ => {}
        }
        let mut commands = Self::commands(&catalog, &action)?;
        let switching = matches!(
            &action,
            Action::Switch(_) | Action::Detach(_) | Action::Create(Create { switch: true, .. })
        );
        if switching && !catalog.has_working_tree {
            return Err(Error::Invalid(
                "This repository has no working tree to switch.".into(),
            ));
        }
        match &action {
            Action::Detach(oid) => {
                verify_commit(&git, *oid)?;
                if catalog.current.is_none() && catalog.head == Some(*oid) {
                    return Ok(None);
                }
            }
            Action::AddWorktree(a) => {
                verify_commit(&git, a.start)?;
                let registered: Vec<PathBuf> =
                    catalog.worktrees.iter().map(|w| w.path.clone()).collect();
                if crate::worktree_folder::taken(&a.path, &registered) {
                    return Err(Error::Invalid(format!(
                        "The folder {} already exists.",
                        a.path.display()
                    )));
                }
                match &a.checkout {
                    Checkout::New { name, .. } => {
                        git.run(&["check-ref-format", &format!("refs/heads/{name}")])?;
                    }
                    Checkout::Existing(name) => {
                        if !catalog
                            .locals
                            .iter()
                            .any(|b| b.name == *name && b.tip == a.start)
                        {
                            return Err(Error::Invalid(format!(
                                "Branch {name} moved or no longer exists. Reload and try again."
                            )));
                        }
                        check_occupied(&catalog, name)?;
                    }
                    Checkout::Detached => {}
                }
            }
            Action::DeleteBranches(_)
            | Action::DeleteWorktrees { .. }
            | Action::Reset(_)
            | Action::Rebase(_)
            | Action::Merge(_)
            | Action::CherryPick(_)
            | Action::Revert(_)
            | Action::RestoreStash(_)
            | Action::Resolve(_)
            | Action::Fetch
            | Action::Pull(_)
            | Action::Push(_)
            | Action::SetUpstream(_)
            | Action::DeleteRemoteBranches(_) => {
                unreachable!("handled above")
            }
            Action::Switch(name) => {
                if !catalog.locals.iter().any(|b| b.name == *name) {
                    return Err(Error::Invalid(
                        "The local branch no longer exists. Reload and try again.".into(),
                    ));
                }
                if catalog.current.as_ref() == Some(name) {
                    return Ok(None);
                }
                check_occupied(&catalog, name)?;
            }
            Action::Create(c) => {
                // Git is the final ref-name authority as well as the namespace collision guard.
                git.run(&["check-ref-format", &format!("refs/heads/{}", c.name)])?;
            }
        }
        // Leaving a detached HEAD.
        let start = catalog
            .head
            .filter(|_| switching && catalog.current.is_none());
        if let Some(start) = start {
            let future_root = match &action {
                Action::Create(c) => Some(c.start),
                Action::Detach(oid) => Some(*oid),
                _ => None,
            };
            let commits = lost_commits(
                &git,
                &catalog,
                start,
                &[],
                &[catalog.root.as_path()],
                future_root,
            )?;
            if !commits.is_empty()
                && !approval.is_some_and(|w| {
                    w.action == action && w.head == catalog.head && w.commits == commits
                })
            {
                let repo = Arc::new(git.load()?);
                if commits.iter().any(|oid| repo.lookup(oid).is_none()) {
                    return Err(Error::Invalid(
                        "The repository changed while checking lost commits. Reload and try again."
                            .into(),
                    ));
                }
                return Ok(Some(Warning {
                    action,
                    commits,
                    replaced: Vec::new(),
                    deletions: Vec::new(),
                    head: catalog.head,
                    repo,
                    commands: commands.clone(),
                }));
            }
        }
        for args in commands.drain(..) {
            if !run(&git, args, cancel, report)? {
                return Err(Error::Failed(report.steps.last().unwrap().output.clone()));
            }
        }
        Ok(None)
    }

    /// Deleting local branches. Git's own check (`branch -d`) only asks whether HEAD or each
    /// branch's upstream has its commits, so parterre works out what they lose together first,
    /// and warns before anything is deleted. Git then deletes the ones it agrees to; the rest
    /// are forced only when that loses nothing, or only what was approved: `approved` is asked
    /// with HEAD and the commits lost.
    fn delete_branches(
        &self,
        catalog: &Catalog,
        branches: &[BranchTip],
        approved: &dyn Fn(Option<Oid>, &[Oid]) -> bool,
        cancel: &CancelTree,
        report: &mut Report,
    ) -> Result<Option<Warning>, Error> {
        let git = Git::new(&self.path);
        check_branches(catalog, branches)?;
        let deletions = branch_losses(&git, catalog, branches)?;
        let commits = all_commits(&deletions);
        if !approved(catalog.head, &commits) {
            let action = Action::DeleteBranches(branches.to_vec());
            return branch_warning(&git, action, catalog.head, deletions, commits);
        }
        let safe = delete_command("-d", branches.iter().map(|b| b.name.clone()));
        if run(&git, safe, cancel, report)? {
            return Ok(None);
        }
        let output = report.steps.last().unwrap().output.clone();
        // A refusal on a merged branch is unrelated to commit loss (e.g. a lock or
        // permissions). Check Git's merge predicate instead of parsing its stderr.
        let refreshed = Catalog::load(&self.path)?;
        let left: Vec<BranchTip> = branches
            .iter()
            .filter(|b| refreshed.locals.iter().any(|l| l.name == b.name))
            .cloned()
            .collect();
        if left.is_empty() {
            return Err(Error::Failed(output));
        }
        check_branches(&refreshed, &left)?;
        for b in &left {
            let upstream = git.query(&[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{}@{{upstream}}^{{commit}}", b.name),
            ])?;
            let merged_into = upstream
                .map(|s| parse_oid(&s))
                .transpose()?
                .or(refreshed.head);
            let Some(into) = merged_into else {
                return Err(Error::Failed(output));
            };
            if git
                .query(&[
                    "merge-base",
                    "--is-ancestor",
                    &b.tip.to_hex(),
                    &into.to_hex(),
                ])?
                .is_some()
            {
                return Err(Error::Failed(output));
            }
        }
        // The ones git deleted were merged: what the rest lose is what was approved.
        let deletions = branch_losses(&git, &refreshed, &left)?;
        let commits = all_commits(&deletions);
        if !approved(refreshed.head, &commits) {
            let action = Action::DeleteBranches(left);
            return branch_warning(&git, action, refreshed.head, deletions, commits);
        }
        let force = delete_command("-D", left.into_iter().map(|b| b.name));
        if !run(&git, force, cancel, report)? {
            return Err(Error::Failed(report.steps.last().unwrap().output.clone()));
        }
        Ok(None)
    }

    /// Deleting other worktrees. Git's own check misses lost work there (it deletes ignored
    /// files and a clean detached HEAD's commits without a word), so parterre always asks first:
    /// a confirmation when nothing is lost, a warning listing what each loses. Only after that,
    /// or after git refused anyway, does it force. They go one by one; the ones git refuses are
    /// asked about again, together, and a failure stops the rest. With `branches`, the branches
    /// of the ones removed go next, as deleting branches does, without asking again for what
    /// was approved.
    fn delete_worktrees(
        &self,
        catalog: &Catalog,
        paths: &[PathBuf],
        branches: bool,
        approval: Option<&Warning>,
        cancel: &CancelTree,
        report: &mut Report,
    ) -> Result<Option<Warning>, Error> {
        let action = &Action::DeleteWorktrees {
            paths: paths.to_vec(),
            branches,
        };
        let git = Git::new(&self.path);
        let wts = find_worktrees(catalog, paths)?;
        let deletions = worktree_losses(&git, catalog, &wts)?;
        let commits = worktree_commits(&deletions, branches);
        let approved = approval.filter(|w| {
            w.action == *action && w.commits == commits && same_losses(&w.deletions, &deletions)
        });
        let Some(approved) = approved else {
            return Ok(Some(Warning {
                action: action.clone(),
                replaced: Vec::new(),
                head: catalog.head,
                repo: Arc::new(git.load()?),
                commands: worktree_commands(&deletions, branches),
                deletions,
                commits,
            }));
        };
        let mut refused = Vec::new();
        let mut gone = Vec::new();
        for (wt, approved) in wts.iter().zip(&approved.deletions) {
            let force = !approved.files.is_empty() || approved.refusal.is_some();
            let removed = run(&git, remove(&wt.path, force), cancel, report)?;
            let output = report
                .steps
                .last()
                .map(|s| s.output.clone())
                .unwrap_or_default();
            let after = Catalog::load(&self.path)?;
            let registered = after
                .worktrees
                .iter()
                .any(|w| crate::worktree_folder::same_path(&w.path, &wt.path));
            if !registered {
                if wt.path.exists() {
                    // Windows: a file in use stops the deletion halfway, after git has let go.
                    return Err(Error::Failed(format!(
                        "The worktree is gone from git, but its folder was left behind: {}. Delete it yourself.{}",
                        wt.path.display(),
                        if output.is_empty() {
                            String::new()
                        } else {
                            format!("\n\n{output}")
                        }
                    )));
                }
                if let Some(b) = approved.branch.as_ref().filter(|_| branches) {
                    gone.push(BranchTip {
                        name: b.name.clone(),
                        tip: b.tip,
                    });
                }
                continue;
            }
            if removed || force {
                return Err(Error::Failed(output));
            }
            refused.push((wt.path.clone(), output));
        }
        if !gone.is_empty() {
            // What deleting them loses now is at most what was approved: the worktrees that
            // are left, and their branches, only keep more.
            let allowed = &approved.commits;
            let approved =
                |_: Option<Oid>, commits: &[Oid]| commits.iter().all(|c| allowed.contains(c));
            let after = Catalog::load(&self.path)?;
            if let Some(warning) = self.delete_branches(&after, &gone, &approved, cancel, report)? {
                return Ok(Some(warning));
            }
        }
        if refused.is_empty() {
            return Ok(None);
        }
        // Git refused although nothing was found at risk: it changed since, or git knows of
        // something parterre doesn't (submodules). Ask again, with what's at risk now.
        let after = Catalog::load(&self.path)?;
        let paths: Vec<PathBuf> = refused.iter().map(|(p, _)| p.clone()).collect();
        let wts = find_worktrees(&after, &paths)?;
        let mut deletions = worktree_losses(&git, &after, &wts)?;
        for (d, (_, output)) in deletions.iter_mut().zip(refused) {
            d.refusal = Some(output);
        }
        Ok(Some(Warning {
            action: Action::DeleteWorktrees { paths, branches },
            replaced: Vec::new(),
            head: after.head,
            repo: Arc::new(git.load()?),
            commands: worktree_commands(&deletions, branches),
            commits: worktree_commits(&deletions, branches),
            deletions,
        }))
    }
}

/// The warning before deleting branches loses `commits`.
fn branch_warning(
    git: &Git,
    action: Action,
    head: Option<Oid>,
    deletions: Vec<Deletion>,
    commits: Vec<Oid>,
) -> Result<Option<Warning>, Error> {
    let repo = Arc::new(git.load()?);
    if commits.iter().any(|oid| repo.lookup(oid).is_none()) {
        return Err(Error::Invalid(
            "The repository changed while checking lost commits. Reload and try again.".into(),
        ));
    }
    let force = delete_command("-D", deletions.iter().map(|d| d.name.clone()));
    Ok(Some(Warning {
        action,
        commits,
        replaced: Vec::new(),
        deletions,
        repo,
        commands: vec![force],
        head,
    }))
}

/// Each branch may be deleted, and is where it was offered.
fn check_branches(catalog: &Catalog, branches: &[BranchTip]) -> Result<(), Error> {
    if branches.is_empty() {
        return Err(Error::Invalid("Choose a branch to delete.".into()));
    }
    for (i, b) in branches.iter().enumerate() {
        if branches[..i].iter().any(|o| o.name == b.name) {
            return Err(Error::Invalid(format!(
                "Branch {} is listed twice.",
                b.name
            )));
        }
        check_occupied(catalog, &b.name)?;
        if !catalog
            .locals
            .iter()
            .any(|l| l.name == b.name && l.tip == b.tip)
        {
            return Err(Error::Invalid(format!(
                "Branch {} changed. Reload and review it before deleting.",
                b.name
            )));
        }
    }
    Ok(())
}

/// What deleting the branches would lose: the commits only each reaches once they're all gone.
fn branch_losses(
    git: &Git,
    catalog: &Catalog,
    branches: &[BranchTip],
) -> Result<Vec<Deletion>, Error> {
    let excluded: Vec<String> = branches
        .iter()
        .map(|b| format!("refs/heads/{}", b.name))
        .collect();
    branches
        .iter()
        .map(|b| {
            Ok(Deletion {
                name: b.name.clone(),
                path: None,
                commits: lost_commits(git, catalog, b.tip, &excluded, &[], None)?,
                files: Vec::new(),
                refusal: None,
                branch: None,
                head: Some(b.tip),
            })
        })
        .collect()
}

/// `git worktree remove`, forced when it would lose changes.
fn remove(path: &Path, force: bool) -> Vec<String> {
    let mut args = vec!["worktree".to_owned(), "remove".to_owned()];
    if force {
        args.push("--force".to_owned());
    }
    args.extend(["--".to_owned(), path.to_string_lossy().into_owned()]);
    args
}

/// The worktrees at `paths`, when each may be deleted.
fn find_worktrees<'a>(catalog: &'a Catalog, paths: &[PathBuf]) -> Result<Vec<&'a Worktree>, Error> {
    if paths.is_empty() {
        return Err(Error::Invalid("Choose a worktree to delete.".into()));
    }
    let mut wts: Vec<&Worktree> = Vec::new();
    for path in paths {
        let wt = find_worktree(catalog, path)?;
        if wts.iter().any(|w| std::ptr::eq(*w, wt)) {
            return Err(Error::Invalid(format!(
                "Worktree {} is listed twice.",
                wt.name()
            )));
        }
        wts.push(wt);
    }
    Ok(wts)
}

/// The worktree at `path`, when it may be deleted.
fn find_worktree<'a>(catalog: &'a Catalog, path: &Path) -> Result<&'a Worktree, Error> {
    let Some(wt) = catalog
        .worktrees
        .iter()
        .find(|w| crate::worktree_folder::same_path(&w.path, path))
    else {
        return Err(Error::Invalid(
            "The worktree is no longer there. Reload and try again.".into(),
        ));
    };
    if wt.main {
        return Err(Error::Invalid("The main worktree can't be deleted.".into()));
    }
    if wt.open {
        return Err(Error::Invalid(
            "The open worktree can't be deleted. Go to another worktree first.".into(),
        ));
    }
    if let Some(reason) = &wt.locked {
        return Err(Error::Invalid(if reason.is_empty() {
            "The worktree is locked.".to_owned()
        } else {
            format!("The worktree is locked: {reason}")
        }));
    }
    // An operation in progress there is no reason to keep it: deleting the worktree ends it,
    // through the usual warnings, and its branch is left where it was.
    Ok(wt)
}

/// What deleting the worktrees would lose: each one's changed files, and the commits only its
/// detached HEAD reaches once they're all gone. A branch's commits stay with the branch, and
/// are counted apart for when it goes too.
fn worktree_losses(
    git: &Git,
    catalog: &Catalog,
    wts: &[&Worktree],
) -> Result<Vec<Deletion>, Error> {
    let leaving: Vec<&Path> = wts.iter().map(|w| w.path.as_path()).collect();
    let branches: Vec<Option<BranchTip>> =
        wts.iter().map(|w| worktree_branch(catalog, w)).collect();
    let excluded: Vec<String> = branches
        .iter()
        .flatten()
        .map(|b| format!("refs/heads/{}", b.name))
        .collect();
    wts.iter()
        .zip(branches)
        .map(|(wt, branch)| {
            let files = if wt.missing {
                Vec::new()
            } else {
                changed_files(&wt.path)?
            };
            let commits = match (&wt.branch, wt.head) {
                (None, Some(head)) => lost_commits(git, catalog, head, &[], &leaving, None)?,
                _ => Vec::new(),
            };
            let branch = match branch {
                Some(b) => Some(WorktreeBranch {
                    commits: lost_commits(git, catalog, b.tip, &excluded, &leaving, None)?,
                    name: b.name,
                    tip: b.tip,
                }),
                None => None,
            };
            Ok(Deletion {
                name: wt.name(),
                path: Some(wt.path.clone()),
                commits,
                files,
                refusal: None,
                branch,
                head: wt.head,
            })
        })
        .collect()
}

/// The local branch a worktree has checked out, with its tip. A branch being rebased there
/// has none: its tip isn't what the worktree shows, and *Abort* comes first.
fn worktree_branch(catalog: &Catalog, wt: &Worktree) -> Option<BranchTip> {
    let name = wt.branch.as_ref()?;
    catalog
        .locals
        .iter()
        .find(|b| b.name == *name)
        .map(|b| BranchTip {
            name: b.name.clone(),
            tip: b.tip,
        })
}

/// Every commit deleting the worktrees loses, with their branches or without.
fn worktree_commits(deletions: &[Deletion], branches: bool) -> Vec<Oid> {
    let mut commits = all_commits(deletions);
    if branches {
        commits.extend(
            deletions
                .iter()
                .flat_map(|d| d.branch.iter())
                .flat_map(|b| b.commits.clone()),
        );
        commits.sort_by_key(|o| o.to_hex());
        commits.dedup();
    }
    commits
}

/// Removing the worktrees, forced where they lose changes or git refused, and then deleting
/// their branches: `-d`, or `-D` when that loses commits.
fn worktree_commands(deletions: &[Deletion], branches: bool) -> Vec<Vec<String>> {
    let mut commands: Vec<Vec<String>> = deletions
        .iter()
        .map(|d| {
            let force = !d.files.is_empty() || d.refusal.is_some();
            remove(d.path.as_deref().unwrap(), force)
        })
        .collect();
    let owned: Vec<&WorktreeBranch> = deletions.iter().flat_map(|d| d.branch.iter()).collect();
    if branches && !owned.is_empty() {
        let lossy = owned.iter().any(|b| !b.commits.is_empty());
        let names = owned.iter().map(|b| b.name.clone());
        commands.push(delete_command(if lossy { "-D" } else { "-d" }, names));
    }
    commands
}

/// `git branch <flag> -- <names>`.
fn delete_command(flag: &str, names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut args = vec!["branch".to_owned(), flag.to_owned(), "--".to_owned()];
    args.extend(names);
    args
}

/// Configures a new branch's upstream: one already fetched, or one that doesn't exist yet.
fn track_commands(
    catalog: &Catalog,
    name: &str,
    track: Option<&str>,
) -> Result<Vec<Vec<String>>, Error> {
    let words = |s: &[&str]| s.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let Some(track) = track.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(Vec::new());
    };
    let Some((remote, branch)) = catalog.tracking_parts(track) else {
        return Err(Error::Invalid(
            "Use a configured remote followed by a branch name, such as origin/topic.".into(),
        ));
    };
    if catalog.track_error(track).is_some() {
        return Err(Error::Invalid("Enter a valid remote branch name.".into()));
    }
    if catalog.remotes.iter().any(|r| r.name == track) {
        return Ok(vec![words(&[
            "branch",
            &format!("--set-upstream-to=refs/remotes/{track}"),
            "--",
            name,
        ])]);
    }
    // Configuration works before a remote ref exists, and does not change the selected start
    // commit even when the upstream points somewhere else.
    let mut commands = vec![
        words(&[
            "config",
            "--local",
            "--replace-all",
            &format!("branch.{name}.remote"),
            remote,
        ]),
        words(&[
            "config",
            "--local",
            "--replace-all",
            &format!("branch.{name}.merge"),
            &format!("refs/heads/{branch}"),
        ]),
    ];
    if catalog.auto_setup_rebase {
        commands.push(words(&[
            "config",
            "--local",
            "--replace-all",
            &format!("branch.{name}.rebase"),
            "true",
        ]));
    }
    Ok(commands)
}

fn verify_commit(git: &Git, oid: Oid) -> Result<(), Error> {
    if git
        .query(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{}^{{commit}}", oid.to_hex()),
        ])?
        .is_none()
    {
        return Err(Error::Invalid(
            "The commit is no longer in the repository. Reload and try again.".into(),
        ));
    }
    Ok(())
}

/// Uncommitted changes to files that aren't ignored, in the worktree at `path`: staged,
/// modified and untracked. Ignored files never count.
fn changed_files(path: &Path) -> Result<Vec<String>, Error> {
    let out = Git::new(path).run(&[
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=none",
    ])?;
    let mut files = Vec::new();
    let mut fields = out.split('\0');
    while let Some(entry) = fields.next() {
        if entry.len() < 4 {
            continue;
        }
        let (status, file) = entry.split_at(3);
        files.push(file.to_owned());
        // A rename or copy is followed by the path it came from.
        if status.starts_with(['R', 'C']) {
            fields.next();
        }
    }
    files.sort();
    Ok(files)
}

fn check_occupied(catalog: &Catalog, name: &str) -> Result<(), Error> {
    if catalog.current.as_deref() == Some(name) {
        return Err(Error::Invalid(format!(
            "Branch {name} is the current branch."
        )));
    }
    if let Some(place) = catalog.occupied.get(name) {
        return Err(Error::Invalid(format!(
            "Branch {name} is in use by {}.",
            place.display()
        )));
    }
    Ok(())
}

/// Commits reachable from `start` that nothing else will reach: no ref but the `excluded`
/// ones, and no worktree's HEAD but the ones at `leaving`.
pub(crate) fn lost_commits(
    git: &Git,
    catalog: &Catalog,
    start: Oid,
    excluded: &[String],
    leaving: &[&Path],
    future_root: Option<Oid>,
) -> Result<Vec<Oid>, Error> {
    let mut protected: HashSet<Oid> = catalog
        .roots
        .iter()
        .filter(|(name, _)| !excluded.contains(name))
        .map(|(_, oid)| *oid)
        .collect();
    protected.extend(
        catalog
            .heads
            .iter()
            .filter(|(p, _)| {
                !leaving
                    .iter()
                    .any(|l| crate::worktree_folder::same_path(p, l))
            })
            .map(|(_, oid)| *oid),
    );
    let mut input = format!("{}\n", start.to_hex());
    if let Some(oid) = future_root {
        protected.insert(oid);
    }
    for oid in protected {
        input.push_str(&format!("^{}\n", oid.to_hex()));
    }
    let result = git.run_with_input(&["rev-list", "--stdin"], input)?;
    let mut commits = result
        .lines()
        .map(parse_oid)
        .collect::<Result<Vec<_>, _>>()?;
    commits.sort_by_key(|o| o.to_hex());
    Ok(commits)
}

pub(crate) fn run(
    git: &Git,
    args: Vec<String>,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<bool, Error> {
    run_with(git, args, &[], cancel, report)
}

/// [`run`], with `env` set for git.
pub(crate) fn run_with(
    git: &Git,
    args: Vec<String>,
    env: &[(&str, &str)],
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<bool, Error> {
    let mut child = cancel
        .start(git.operation_command(&args).envs(env.iter().copied()))
        .map_err(|e| match e {
            parterre_util::Start::Cancelled => Error::Cancelled,
            parterre_util::Start::Spawn(e) => GitError::Spawn(e).into(),
        })?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stderr.read_to_end(&mut b);
        b
    });
    let mut output = Vec::new();
    let read_result = stdout.read_to_end(&mut output);
    let error = errors.join().unwrap_or_default();
    let status = child.wait();
    let cancelled = cancel.finish();
    let status = status.map_err(GitError::Spawn)?;
    output.extend(error);
    report.steps.push(Step {
        args,
        output: String::from_utf8_lossy(&output).trim().to_owned(),
        success: status.success(),
    });
    if cancelled {
        return Err(Error::Cancelled);
    }
    read_result.map_err(GitError::Spawn)?;
    Ok(status.success())
}

/// Display only: execution always passes an argument vector directly to Git.
pub fn command_text(args: &[String]) -> String {
    let quote = |s: &str| {
        if !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/_-.:=".contains(&b))
        {
            s.to_owned()
        } else {
            format!("'{}'", s.replace('\'', "'\\''"))
        }
    };
    format!(
        "git {}",
        args.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" ")
    )
}
