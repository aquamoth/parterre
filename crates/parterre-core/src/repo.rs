//! A snapshot holds every commit reachable from any ref (except notes), so that view options
//! such as "show remote branches" can be toggled without going back to git.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::git::Listing;
use crate::oid::Oid;
use crate::upstream::Upstream;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommitIx(pub u32);

impl CommitIx {
    pub fn new(ix: usize) -> CommitIx {
        CommitIx(u32::try_from(ix).expect("fewer than 2^32 commits"))
    }

    pub fn ix(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub oid: Oid,
    /// Those in the snapshot, first parent first.
    pub parents: Vec<CommitIx>,
    /// Some parent is not in the snapshot: a shallow clone's boundary.
    pub truncated: bool,
    /// Such as an `--allow-empty` root, or an svn import's "create trunk" commit.
    pub empty_tree: bool,
    pub author_name: String,
    pub author_email: String,
    /// Unix seconds.
    pub author_time: i64,
    /// In the local time zone, `YYYY-MM-DD HH:MM`.
    pub author_date: String,
    /// Unix seconds.
    pub commit_time: i64,
    pub subject: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RefKind {
    LocalBranch,
    RemoteBranch,
    Tag,
    Stash,
    /// Only while HEAD is not on a branch.
    DetachedHead,
    /// Such as `refs/pull/*`.
    Other,
}

#[derive(Clone, Debug)]
pub struct GitRef {
    /// `refs/remotes/origin/main`.
    pub full_name: String,
    /// `origin/main`.
    pub name: String,
    pub kind: RefKind,
    /// Tags peeled.
    pub target: CommitIx,
    pub annotated: bool,
    pub is_head: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Head {
    Branch {
        name: String,
        /// `None` for an unborn branch.
        target: Option<CommitIx>,
    },
    Detached(CommitIx),
}

/// A bare main repository isn't one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// `None` for an unborn branch.
    pub head: Option<CommitIx>,
    /// `refs/heads/topic`; `None` if HEAD is detached.
    pub branch: Option<String>,
    pub locked: bool,
    /// Its folder is gone.
    pub missing: bool,
    pub open: bool,
}

impl Worktree {
    pub fn name(&self) -> String {
        folder_name(&self.path)
    }

    /// What menus call it, as the graph labels it: its branch (`topic`), or its folder's name
    /// when detached.
    pub fn label(&self) -> String {
        match &self.branch {
            Some(b) => b.strip_prefix("refs/heads/").unwrap_or(b).to_owned(),
            None => self.name(),
        }
    }
}

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

/// A detached HEAD first, then TortoiseGit's order, by full ref name (heads, remotes, stash,
/// tags).
pub fn cmp_refs_for_display(a: &GitRef, b: &GitRef) -> Ordering {
    (b.kind == RefKind::DetachedHead)
        .cmp(&(a.kind == RefKind::DetachedHead))
        .then_with(|| a.full_name.cmp(&b.full_name))
}

/// git's abbreviation length for small repositories.
pub const DEFAULT_ABBREV_LEN: usize = 7;

#[derive(Clone, Debug)]
pub struct Repo {
    /// The working tree root, or the git dir for a bare repository.
    pub path: PathBuf,
    pub commits: Vec<Commit>,
    pub refs: Vec<GitRef>,
    pub head: Head,
    /// For [`Oid::short`]: `core.abbrev`, by default sized to the object count.
    pub abbrev_len: usize,
    /// False for a bare repository, or one opened inside its `.git` folder.
    pub has_working_tree: bool,
    /// The main one first; empty if git can't list them.
    pub worktrees: Vec<Worktree>,
    /// `refs/remotes/origin/main`, as `origin/HEAD` names it.
    pub default_branch: Option<String>,
    /// In ref order.
    pub upstreams: Vec<Upstream>,
    pub(crate) listing: Listing,
    by_oid: HashMap<Oid, CommitIx>,
    generations: OnceLock<Vec<u32>>,
}

impl Repo {
    pub fn new(path: PathBuf, commits: Vec<Commit>, refs: Vec<GitRef>, head: Head) -> Repo {
        let by_oid = commits
            .iter()
            .enumerate()
            .map(|(i, c)| (c.oid, CommitIx::new(i)))
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

    /// By [`CommitIx`]: one more than the highest parent's, 1 for a root. A commit's ancestors
    /// all have lower generations than it, whatever their dates say.
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
            .map(|(i, _)| CommitIx::new(i))
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

    /// The same refs at the same commits, HEAD, default branch, upstreams and worktrees. The
    /// commits are then the same too, as a snapshot holds exactly what those reach.
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

    pub fn display_name(&self) -> String {
        folder_name(&self.path)
    }
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

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
