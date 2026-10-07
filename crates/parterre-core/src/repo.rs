//! In-memory snapshot of a repository's commit graph and refs.
//!
//! The snapshot holds every commit reachable from any ref (except notes), so that view options
//! such as "show remote branches" can be toggled without going back to git.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::git::Listing;
use crate::oid::Oid;
use crate::upstream::Upstream;

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

/// A label on a commit (see [`Repo::labels`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    /// An index into [`Repo::refs`], and into [`Repo::worktrees`] the worktree that has it
    /// checked out (for HEAD, the open one), while worktrees are shown.
    Ref {
        index: usize,
        worktree: Option<usize>,
    },
    /// An index into [`Repo::worktrees`]: another worktree whose detached HEAD this is.
    Worktree(usize),
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
    /// The remote branch `origin/HEAD` points at (`refs/remotes/origin/main`), if any.
    pub default_branch: Option<String>,
    /// The local branches that have an upstream, in ref order.
    pub upstreams: Vec<Upstream>,
    /// What git listed of the refs and worktrees, for the branch catalogue (#310).
    pub(crate) listing: Listing,
    by_oid: HashMap<Oid, CommitIx>,
    /// Every commit's generation, worked out the first time it's needed.
    generations: OnceLock<Vec<u32>>,
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
            default_branch: None,
            upstreams: Vec::new(),
            listing: Listing::default(),
            by_oid,
            generations: OnceLock::new(),
        }
    }

    /// Every commit's generation, by [`CommitIx`]: one more than its highest parent's, 1 for a
    /// root. A commit's ancestors all have lower generations than it, whatever their dates say.
    pub fn generations(&self) -> &[u32] {
        self.generations.get_or_init(|| generations_of(self))
    }

    /// True if `ancestor` is `from` or one of its ancestors, as `git merge-base --is-ancestor`
    /// says: walks down from `from`, no lower than `ancestor`'s generation.
    pub fn reaches(&self, from: CommitIx, ancestor: CommitIx) -> bool {
        let generation = self.generations();
        let floor = generation[ancestor.ix()];
        let mut seen = vec![false; self.commits.len()];
        let mut stack = vec![from];
        while let Some(c) = stack.pop() {
            if c == ancestor {
                return true;
            }
            if std::mem::replace(&mut seen[c.ix()], true) {
                continue;
            }
            stack.extend(
                self.commit(c)
                    .parents
                    .iter()
                    .filter(|p| generation[p.ix()] >= floor && !seen[p.ix()]),
            );
        }
        false
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

    /// Where the graph's leftmost line may start, best first: the local branch named like the
    /// default branch, the default branch itself, then the main worktree's HEAD. None of them
    /// moves when the open worktree or its HEAD changes, so neither do the columns.
    pub fn layout_anchors(&self) -> Vec<CommitIx> {
        let branch = |full_name: &str| {
            self.refs
                .iter()
                .find(|r| r.full_name == full_name)
                .map(|r| r.target)
        };
        let default = self.default_branch.as_deref();
        let local = default
            .and_then(|d| d.strip_prefix("refs/remotes/origin/"))
            .and_then(|name| branch(&format!("refs/heads/{name}")));
        let remote = default.and_then(branch);
        let main = self.worktrees.first().and_then(|w| w.head);
        [local, remote, main].into_iter().flatten().collect()
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

    /// The upstream of the local branch `branch` (an index into [`Repo::refs`]), if it has one.
    pub fn upstream_of(&self, branch: usize) -> Option<&Upstream> {
        self.upstreams.iter().find(|u| u.branch == branch)
    }

    /// The worktrees other than the open one that have the branch `full_name` checked out.
    pub fn worktrees_on<'a>(&'a self, full_name: &'a str) -> impl Iterator<Item = usize> + 'a {
        self.worktrees
            .iter()
            .enumerate()
            .filter(move |(_, w)| !w.open && w.branch.as_deref() == Some(full_name))
            .map(|(i, _)| i)
    }

    /// The worktrees other than the open one whose detached HEAD is `commit`.
    pub fn detached_worktrees_at(&self, commit: CommitIx) -> impl Iterator<Item = usize> + '_ {
        self.worktrees
            .iter()
            .enumerate()
            .filter(move |(_, w)| !w.open && w.branch.is_none() && w.head == Some(commit))
            .map(|(i, _)| i)
    }

    /// A commit's labels, worktrees first: `refs` (indices into [`Repo::refs`], in
    /// [`cmp_refs_for_display`] order) and the `detached` worktrees at it. HEAD comes first,
    /// then, with `worktrees_shown`, the branches other worktrees have checked out, then the
    /// detached worktrees, then the other refs. With `worktrees_shown`, every ref a worktree
    /// has checked out names it: HEAD the open one.
    pub fn labels(&self, refs: &[usize], detached: &[usize], worktrees_shown: bool) -> Vec<Label> {
        let open = self.worktrees.iter().position(|w| w.open);
        let label = |index: usize| {
            let r = &self.refs[index];
            let worktree = match worktrees_shown {
                false => None,
                true if r.is_head => open,
                true => self.worktrees_on(&r.full_name).next(),
            };
            Label::Ref { index, worktree }
        };
        let refs: Vec<Label> = refs.iter().map(|&i| label(i)).collect();
        let is_head =
            |l: &Label| matches!(*l, Label::Ref { index, .. } if self.refs[index].is_head);
        let in_worktree = |l: &Label| {
            matches!(
                l,
                Label::Ref {
                    worktree: Some(_),
                    ..
                }
            )
        };
        let head = refs.iter().filter(|l| is_head(l));
        let worktrees = refs.iter().filter(|l| !is_head(l) && in_worktree(l));
        let others = refs.iter().filter(|l| !is_head(l) && !in_worktree(l));
        head.chain(worktrees)
            .copied()
            .chain(detached.iter().map(|&k| Label::Worktree(k)))
            .chain(others.copied())
            .collect()
    }

    /// The text of a label: a ref's short name, or a worktree's folder name. While worktrees
    /// are shown, the open worktree's detached HEAD is named after its folder too, as it is
    /// when another worktree is open.
    pub fn label_name(&self, label: Label) -> String {
        match label {
            Label::Ref {
                index,
                worktree: Some(k),
            } if self.refs[index].kind == RefKind::DetachedHead => self.worktrees[k].name(),
            Label::Ref { index, .. } => self.refs[index].name.clone(),
            Label::Worktree(k) => self.worktrees[k].name(),
        }
    }

    /// True if both snapshots have the same refs pointing at the same commits, the same HEAD,
    /// the same default branch, the same upstreams and the same worktrees. The commits are then the same too, as a
    /// snapshot holds exactly what its refs and worktrees reach.
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
        let upstreams = |repo: &Repo| -> Vec<(String, String)> {
            repo.upstreams
                .iter()
                .map(|u| (repo.refs[u.branch].full_name.clone(), u.name.clone()))
                .collect()
        };
        head(self) == head(other)
            && refs(self) == refs(other)
            && worktrees(self) == worktrees(other)
            && self.default_branch == other.default_branch
            && upstreams(self) == upstreams(other)
    }

    /// Display name for the repository (directory name).
    pub fn display_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// Every commit's generation (see [`Repo::generations`]).
fn generations_of(repo: &Repo) -> Vec<u32> {
    let n = repo.commits.len();
    let mut generation = vec![0u32; n];
    let mut stack = Vec::new();
    for start in 0..n {
        if generation[start] != 0 {
            continue;
        }
        stack.push((start, false));
        while let Some((c, expanded)) = stack.pop() {
            if generation[c] != 0 {
                continue;
            }
            let parents = &repo.commits[c].parents;
            if expanded {
                let highest = parents.iter().map(|p| generation[p.ix()]).max();
                generation[c] = highest.unwrap_or(0) + 1;
            } else {
                stack.push((c, true));
                stack.extend(
                    parents
                        .iter()
                        .filter(|p| generation[p.ix()] == 0)
                        .map(|p| (p.ix(), false)),
                );
            }
        }
    }
    generation
}
