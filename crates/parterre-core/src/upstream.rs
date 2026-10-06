//! Local branches against their upstreams: the commits on one side only, and what a push or a
//! pull would do with each of them (see [`Side`]).
//!
//! Ahead and behind are worked out from the snapshot's own commits. Only a branch that has
//! diverged from its upstream asks git more: which of the upstream's commits have a copy on it
//! (`rev-list --cherry-mark`).

use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::git::Git;
use crate::repo::{CommitIx, Repo};
use crate::revgraph::RevGraph;

/// Where a commit between a branch and its upstream stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    /// On the branch only: a push sends it.
    Ahead,
    /// On the upstream only, and the branch replaced none of them: a pull brings it.
    Behind,
    /// On the upstream only, with no counterpart on a branch that replaced others: a force
    /// push would lose it.
    Lost,
    /// On the upstream only, with a counterpart on the branch: the same patch, or the same
    /// author, author date and subject, which rebase, amend and conflict resolution keep.
    Replaced,
}

/// A local branch with an upstream, and how the two differ.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upstream {
    /// Index into [`Repo::refs`] of the local branch.
    pub branch: usize,
    /// The upstream's full name: `refs/remotes/origin/topic`, or `refs/heads/main` for a local
    /// one.
    pub name: String,
    /// Index into [`Repo::refs`] of the upstream. `None` when it is gone: deleted on the remote
    /// and pruned.
    pub target: Option<usize>,
    /// The commits on one side only, newest first (parents after their children).
    pub commits: Vec<(CommitIx, Side)>,
    /// The commit the branch was rebased from (the newest it replaced), while the upstream has
    /// moved on past it. No ref points at it then, and the graph shows it anyway.
    pub rebased_from: Option<CommitIx>,
}

impl Upstream {
    /// The upstream's short name, as git prints it: `origin/topic`.
    pub fn short_name(&self) -> &str {
        self.name
            .strip_prefix("refs/remotes/")
            .or_else(|| self.name.strip_prefix("refs/heads/"))
            .unwrap_or(&self.name)
    }

    /// True when the upstream no longer exists.
    pub fn is_gone(&self) -> bool {
        self.target.is_none()
    }

    /// git's ahead count: commits on the branch only.
    pub fn ahead(&self) -> usize {
        self.commits
            .iter()
            .filter(|&&(_, s)| s == Side::Ahead)
            .count()
    }

    /// git's behind count: commits on the upstream only, whatever became of them.
    pub fn behind(&self) -> usize {
        self.commits.len() - self.ahead()
    }

    /// True when the branch was rebased (or otherwise rewritten) after it was pushed: the
    /// upstream has commits it replaced.
    pub fn is_rebased(&self) -> bool {
        self.commits.iter().any(|&(_, s)| s == Side::Replaced)
    }

    /// True when a force push would lose commits of the upstream.
    pub fn loses_commits(&self) -> bool {
        self.commits.iter().any(|&(_, s)| s == Side::Lost)
    }

    /// The edges of `graph` that hold commits of [`Upstream::commits`], each with the sides
    /// of the commits along it, child end first: the child's own commit, then the commits
    /// collapsed into the edge, newest first. `None` for a commit on both sides.
    pub fn edge_sides(&self, graph: &RevGraph) -> Vec<(usize, Vec<Option<Side>>)> {
        if self.commits.is_empty() {
            return Vec::new();
        }
        let sides: HashMap<CommitIx, Side> = self.commits.iter().copied().collect();
        let mut out = Vec::new();
        for (e, &edge) in graph.edges.iter().enumerate() {
            let child = graph.nodes[edge.child as usize].commit;
            let mut along = std::iter::once(child).chain(graph.collapsed(e).iter().copied());
            if !along.any(|c| sides.contains_key(&c)) {
                continue;
            }
            let along: Vec<Option<Side>> = std::iter::once(child)
                .chain(graph.collapsed(e).iter().copied())
                .map(|c| sides.get(&c).copied())
                .collect();
            out.push((e, along));
        }
        out
    }
}

/// The upstream of every local branch that has one, by the branch's full name and the
/// upstream's (from `for-each-ref`'s `%(upstream)`), classified against `repo`. A diverged
/// branch also asks `git` for copies of the upstream's commits.
pub fn load(git: &Git, repo: &Repo, configured: &[(String, String)]) -> Vec<Upstream> {
    let index: HashMap<&str, usize> = repo
        .refs
        .iter()
        .enumerate()
        .map(|(i, r)| (r.full_name.as_str(), i))
        .collect();
    let mut out = Vec::new();
    for (branch_name, name) in configured {
        let Some(&branch) = index.get(branch_name.as_str()) else {
            continue;
        };
        let target = index.get(name.as_str()).copied();
        let mut upstream = Upstream {
            branch,
            name: name.clone(),
            target,
            commits: Vec::new(),
            rebased_from: None,
        };
        if let Some(u) = target {
            let (a, b) = (repo.refs[branch].target, repo.refs[u].target);
            if a != b {
                let (ahead, behind) = difference(repo, repo.generations(), a, b);
                classify(git, repo, &mut upstream, ahead, behind);
            }
        }
        out.push(upstream);
    }
    out
}

/// Fills in `upstream`'s commits: `ahead` and `behind` as [`difference`] gives them.
fn classify(
    git: &Git,
    repo: &Repo,
    upstream: &mut Upstream,
    ahead: Vec<CommitIx>,
    behind: Vec<CommitIx>,
) {
    let commits = &mut upstream.commits;
    commits.extend(ahead.iter().map(|&c| (c, Side::Ahead)));
    if ahead.is_empty() || behind.is_empty() {
        commits.extend(behind.iter().map(|&c| (c, Side::Behind)));
        return;
    }
    let branch = repo.refs[upstream.branch].full_name.as_str();
    let copied = copied_onto_branch(git, branch, &upstream.name);
    // Rebase, amend and conflict resolution keep the author, the author date and (unless
    // reworded) the subject. Several commits in one second by one author are common in scripts
    // and agents, so the subject has to match too.
    let authored = |c: CommitIx| {
        let c = repo.commit(c);
        let (email, name) = (c.author_email.as_str(), c.author_name.as_str());
        (email, name, c.author_time, c.subject.as_str())
    };
    let rewritten: HashSet<_> = ahead.iter().map(|&c| authored(c)).collect();
    let replaced: Vec<bool> = behind
        .iter()
        .map(|&c| copied.contains(&repo.commit(c).oid.to_hex()) || rewritten.contains(&authored(c)))
        .collect();
    // Without anything replaced it is plain divergence: a pull brings the rest.
    let rebased = replaced.iter().any(|&r| r);
    for (c, replaced) in behind.into_iter().zip(replaced) {
        let side = match (rebased, replaced) {
            (false, _) => Side::Behind,
            (true, true) => Side::Replaced,
            (true, false) => Side::Lost,
        };
        commits.push((c, side));
    }
    let tip = upstream.target.map(|u| repo.refs[u].target);
    upstream.rebased_from = upstream
        .commits
        .iter()
        .find(|&&(_, s)| s == Side::Replaced)
        .map(|&(c, _)| c)
        .filter(|&c| Some(c) != tip);
}

/// The upstream's own commits with a patch-equivalent copy on the branch.
fn copied_onto_branch(git: &Git, branch: &str, upstream: &str) -> HashSet<String> {
    let range = format!("{branch}...{upstream}");
    let out = git
        .query(&["rev-list", "--right-only", "--cherry-mark", &range, "--"])
        .ok()
        .flatten()
        .unwrap_or_default();
    out.lines()
        .filter_map(|l| l.strip_prefix('='))
        .map(str::to_owned)
        .collect()
}

/// The commits reachable from `a` but not from `b`, and from `b` but not from `a`, newest
/// first. Walks down from both by generation, as far as either side has commits of its own.
fn difference(
    repo: &Repo,
    generation: &[u32],
    a: CommitIx,
    b: CommitIx,
) -> (Vec<CommitIx>, Vec<CommitIx>) {
    const A: u8 = 1;
    const B: u8 = 2;
    const BOTH: u8 = A | B;
    let mut flags: HashMap<CommitIx, u8> = HashMap::new();
    let mut queue = BinaryHeap::new();
    // Queued commits that only one side reaches so far: the walk ends when there are none.
    let mut one_sided = 0usize;
    let key = |c: CommitIx| (generation[c.ix()], repo.commit(c).commit_time, c);
    for (c, f) in [(a, A), (b, B)] {
        let mark = flags.entry(c).or_insert(0);
        if *mark == 0 {
            queue.push(key(c));
            one_sided += 1;
        } else {
            one_sided -= 1;
        }
        *mark |= f;
    }
    while one_sided > 0 {
        let Some((_, _, c)) = queue.pop() else {
            break;
        };
        let f = flags[&c];
        if f != BOTH {
            one_sided -= 1;
        }
        for &p in &repo.commit(c).parents {
            let mark = flags.entry(p).or_insert(0);
            let before = *mark;
            if before | f == before {
                continue;
            }
            *mark |= f;
            // Generations only fall along parents, so `p` is still queued if it was reached.
            if before == 0 {
                queue.push(key(p));
                if *mark != BOTH {
                    one_sided += 1;
                }
            } else if *mark == BOTH {
                one_sided -= 1;
            }
        }
    }
    let mut ahead = Vec::new();
    let mut behind = Vec::new();
    for (&c, &f) in &flags {
        match f {
            A => ahead.push(c),
            B => behind.push(c),
            _ => {}
        }
    }
    let newest_first = |x: &CommitIx, y: &CommitIx| key(*y).cmp(&key(*x));
    ahead.sort_by(newest_first);
    behind.sort_by(newest_first);
    (ahead, behind)
}
