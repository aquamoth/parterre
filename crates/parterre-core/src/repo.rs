//! In-memory snapshot of a repository's commit graph and refs.
//!
//! The snapshot holds every commit reachable from any ref (except notes), so that view options
//! such as "show remote branches" can be toggled without going back to git.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::oid::Oid;

/// Index of a commit in [`Repo::commits`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommitIx(pub u32);

impl CommitIx {
    pub fn ix(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub oid: Oid,
    /// Parents present in the snapshot, in git order (first parent first).
    pub parents: Vec<CommitIx>,
    /// True if some parent is not in the snapshot (shallow clone boundary).
    pub truncated: bool,
    /// True if the commit's tree is the empty tree (e.g. `git commit --allow-empty` roots or
    /// svn-imported "create trunk" commits).
    pub empty_tree: bool,
    pub author_name: String,
    pub author_email: String,
    /// Author timestamp, seconds since the Unix epoch.
    pub author_time: i64,
    /// Author date formatted by git in the local time zone, `YYYY-MM-DD HH:MM`.
    pub author_date: String,
    /// Committer timestamp, seconds since the Unix epoch.
    pub commit_time: i64,
    pub subject: String,
}

/// What kind of ref a label represents; drives label colour and visibility options.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RefKind {
    /// `refs/heads/*`
    LocalBranch,
    /// `refs/remotes/*`
    RemoteBranch,
    /// `refs/tags/*`
    Tag,
    /// `refs/stash`
    Stash,
    /// A detached `HEAD` (only present when HEAD is not on a branch).
    DetachedHead,
    /// Anything else, e.g. `refs/pull/*` or tool-specific namespaces.
    Other,
}

#[derive(Clone, Debug)]
pub struct GitRef {
    /// Full name, e.g. `refs/remotes/origin/main`.
    pub full_name: String,
    /// Display name, e.g. `origin/main`.
    pub name: String,
    pub kind: RefKind,
    /// The commit the ref points at (tags are peeled).
    pub target: CommitIx,
    /// True for annotated tags (tag objects), false for lightweight tags and non-tags.
    pub annotated: bool,
    /// True if this is the branch `HEAD` points at.
    pub is_head: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Head {
    /// HEAD points at a branch; the commit is `None` for an unborn branch.
    Branch {
        name: String,
        target: Option<CommitIx>,
    },
    Detached(CommitIx),
}

/// One of the repository's worktrees (`git worktree list`). A bare main repository isn't one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    /// Its folder, as git lists it.
    pub path: PathBuf,
    /// The commit checked out. `None` for an unborn branch.
    pub head: Option<CommitIx>,
    /// The branch checked out (`refs/heads/topic`); `None` if HEAD is detached.
    pub branch: Option<String>,
    /// Locked against pruning (`git worktree lock`).
    pub locked: bool,
    /// Its folder is gone.
    pub missing: bool,
    /// The worktree parterre opened.
    pub open: bool,
}

impl Worktree {
    /// Its folder's name, e.g. `t3code-42df6b45`.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// The order refs on one commit are shown in: a detached HEAD first, then TortoiseGit's order,
/// by full ref name (heads, remotes, stash, tags).
pub fn cmp_refs_for_display(a: &GitRef, b: &GitRef) -> Ordering {
    (b.kind == RefKind::DetachedHead)
        .cmp(&(a.kind == RefKind::DetachedHead))
        .then_with(|| a.full_name.cmp(&b.full_name))
}

/// git's abbreviation length for small repositories, used when git cannot tell us.
pub const DEFAULT_ABBREV_LEN: usize = 7;

#[derive(Clone, Debug)]
pub struct Repo {
    /// Working tree root, or the git dir for bare repositories.
    pub path: PathBuf,
    pub commits: Vec<Commit>,
    pub refs: Vec<GitRef>,
    pub head: Head,
    /// How many hex digits git abbreviates hashes to in this repository (`core.abbrev`; by
    /// default sized to the object count, 7 at minimum). Use with [`Oid::short`].
    pub abbrev_len: usize,
    /// False for a bare repository (or one opened inside its `.git` directory): there are no
    /// files on disk to compare with.
    pub has_working_tree: bool,
    /// The worktrees, the main one first (empty if git can't list them).
    pub worktrees: Vec<Worktree>,
    by_oid: HashMap<Oid, CommitIx>,
}

impl Repo {
    pub fn new(path: PathBuf, commits: Vec<Commit>, refs: Vec<GitRef>, head: Head) -> Repo {
        let by_oid = commits
            .iter()
            .enumerate()
            .map(|(i, c)| (c.oid, CommitIx(i as u32)))
            .collect();
        Repo {
            path,
            commits,
            refs,
            head,
            abbrev_len: DEFAULT_ABBREV_LEN,
            has_working_tree: true,
            worktrees: Vec::new(),
            by_oid,
        }
    }

    pub fn commit(&self, ix: CommitIx) -> &Commit {
        &self.commits[ix.ix()]
    }

    pub fn lookup(&self, oid: &Oid) -> Option<CommitIx> {
        self.by_oid.get(oid).copied()
    }

    /// Finds commits whose hex id starts with `prefix` (case-insensitive).
    pub fn find_by_prefix(&self, prefix: &str) -> Vec<CommitIx> {
        let prefix = prefix.to_ascii_lowercase();
        if prefix.len() < 4 || !prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Vec::new();
        }
        self.commits
            .iter()
            .enumerate()
            .filter(|(_, c)| c.oid.to_hex().starts_with(&prefix))
            .map(|(i, _)| CommitIx(i as u32))
            .collect()
    }

    /// The commit `name` stands for: `HEAD`, a ref's short or full name, or a unique hash
    /// prefix (at least 4 digits).
    pub fn resolve(&self, name: &str) -> Option<CommitIx> {
        if name == "HEAD" {
            return self.head_commit();
        }
        if let Some(r) = self
            .refs
            .iter()
            .find(|r| r.name == name || r.full_name == name)
        {
            return Some(r.target);
        }
        match self.find_by_prefix(name)[..] {
            [one] => Some(one),
            _ => None,
        }
    }

    pub fn head_commit(&self) -> Option<CommitIx> {
        match &self.head {
            Head::Branch { target, .. } => *target,
            Head::Detached(c) => Some(*c),
        }
    }

    /// For every commit, the indices into [`Repo::refs`] of the refs pointing at it, in
    /// [`cmp_refs_for_display`] order.
    pub fn refs_by_commit(&self) -> Vec<Vec<usize>> {
        let mut on = vec![Vec::new(); self.commits.len()];
        for (i, r) in self.refs.iter().enumerate() {
            on[r.target.ix()].push(i);
        }
        for refs in &mut on {
            refs.sort_by(|&a, &b| cmp_refs_for_display(&self.refs[a], &self.refs[b]));
        }
        on
    }

    /// The worktrees other than the open one that have the branch `full_name` checked out.
    pub fn worktrees_on<'a>(&'a self, full_name: &'a str) -> impl Iterator<Item = usize> + 'a {
        self.worktrees
            .iter()
            .enumerate()
            .filter(move |(_, w)| !w.open && w.branch.as_deref() == Some(full_name))
            .map(|(i, _)| i)
    }

    /// True if both snapshots have the same refs pointing at the same commits, the same HEAD
    /// and the same worktrees. The commits are then the same too, as a snapshot holds exactly
    /// what its refs and worktrees reach.
    pub fn same_refs(&self, other: &Repo) -> bool {
        let refs = |repo: &Repo| -> Vec<(String, Oid, bool)> {
            repo.refs
                .iter()
                .map(|r| (r.full_name.clone(), repo.commit(r.target).oid, r.annotated))
                .collect()
        };
        let head = |repo: &Repo| {
            let branch = match &repo.head {
                Head::Branch { name, .. } => Some(name.clone()),
                Head::Detached(_) => None,
            };
            (branch, repo.head_commit().map(|c| repo.commit(c).oid))
        };
        // Heads by id, as commit indices differ between snapshots.
        let worktrees = |repo: &Repo| -> Vec<(Worktree, Option<Oid>)> {
            repo.worktrees
                .iter()
                .map(|w| {
                    let head = w.head.map(|c| repo.commit(c).oid);
                    (
                        Worktree {
                            head: None,
                            ..w.clone()
                        },
                        head,
                    )
                })
                .collect()
        };
        head(self) == head(other)
            && refs(self) == refs(other)
            && worktrees(self) == worktrees(other)
    }

    /// Display name for the repository (directory name).
    pub fn display_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}
