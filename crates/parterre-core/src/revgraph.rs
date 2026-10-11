//! Reduces the commit graph to a revision graph, as TortoiseGit does
//! (`docs/research/tortoisegit-revision-graph.md`): only some commits become nodes, and the
//! history between them collapses into edges. Every mode makes one pass over the commits,
//! parents first, that decides whether each commit is kept and, for a hidden one, which kept
//! commits stand for it.
//!
//! An edge holds the commits of its child that none of the ends of the child's edges has, as
//! `git log D..F3` lists them: across hidden merges both sides, and in the forks mode the own
//! commits of a branch hung off its latest merge of its base.
//!
//! In the decorated and forks modes, an undecorated root commit with an empty tree counts as
//! unchanged ("TREESAME") to git: it only appears as the end of an edge, and merge parents
//! leading to it are dropped, so merges that join otherwise empty histories disappear (their
//! commits go to the merge's other edge). Unlike git, parterre shows such a root when it carries
//! a tag or branch, rather than silently dropping a label.

use std::collections::BinaryHeap;

use serde::{Deserialize, Serialize};

use crate::pattern::BranchPatterns;
use crate::repo::{CommitIx, GitRef, RefKind, Repo};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Simplification {
    /// What `git log --simplify-by-decoration` keeps, TortoiseGit's default: commits with refs,
    /// roots, and merges that still join two independent kept lines. Parents are rewritten to
    /// their nearest kept ancestor and, as git's `simplify_merges` does, a parent that is an
    /// ancestor of another parent is dropped.
    #[default]
    Decorated,
    /// Labelled commits and the commits where their histories fork apart: the same reduction
    /// run from the tips down (a fork is a merge, seen backwards). Merges are not nodes: a
    /// hidden commit stands for all its nearest kept ancestors, so a branch that merged its base
    /// back in forks off at its latest such merge, as if rebased.
    Forks,
    /// TortoiseGit's "Show branchings and merges": also every merge, fork point and tip, and
    /// every commit whose only child is a merge.
    BranchesAndMerges,
    AllCommits,
}

impl Simplification {
    pub const ALL: [Simplification; 4] = [
        Simplification::Decorated,
        Simplification::Forks,
        Simplification::BranchesAndMerges,
        Simplification::AllCommits,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Simplification::Decorated => "Labelled commits",
            Simplification::Forks => "Labelled forks",
            Simplification::BranchesAndMerges => "Branchings and merges",
            Simplification::AllCommits => "All commits",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphOptions {
    pub simplification: Simplification,
    pub show_local_branches: bool,
    pub show_remote_branches: bool,
    pub show_tags: bool,
    /// TortoiseGit's "Show all tags": when off, a tag alone does not make a commit a node; tags
    /// still label commits that are shown for other reasons.
    pub tags_make_nodes: bool,
    pub show_stash: bool,
    /// Refs outside heads/remotes/tags/stash, e.g. `refs/pull/*` or tool checkpoints.
    pub show_other_refs: bool,
    /// Merged-in side branches without refs of their own vanish.
    pub first_parent_only: bool,
    /// Only HEAD's history (TortoiseGit's "Current branch"). Other refs still show where they
    /// point into it.
    pub current_branch_only: bool,
    /// Only refs whose name contains one of these comma-separated words (case-insensitive)
    /// start history; empty = all. Refs on commits shown anyway are always labelled.
    pub ref_filter: String,
    /// Branches matching these wildcards (see [`BranchPatterns`]) do not start history, so
    /// they vanish unless a shown ref's history contains them; there they keep their labels.
    pub hide_branches: String,
    /// See [`PullRequestHead`].
    pub show_pull_requests: bool,
    /// The other worktrees (see [`Repo::worktrees`]): the branches they have checked out, even
    /// where the local-branch switch or [`GraphOptions::hide_branches`] would hide them, and
    /// the detached HEADs, which start history like branches.
    pub show_worktrees: bool,
    /// How local branches differ from their upstreams (see [`Repo::upstreams`]). In the graph
    /// itself, the commit a branch was rebased from is a node while that lasts.
    pub show_upstreams: bool,
}

impl Default for GraphOptions {
    fn default() -> Self {
        GraphOptions {
            simplification: Simplification::default(),
            show_local_branches: true,
            show_remote_branches: true,
            show_tags: true,
            tags_make_nodes: true,
            show_stash: true,
            show_other_refs: false,
            first_parent_only: false,
            current_branch_only: false,
            ref_filter: String::new(),
            hide_branches: String::new(),
            show_pull_requests: true,
            show_worktrees: false,
            show_upstreams: true,
        }
    }
}

impl GraphOptions {
    /// Whether a ref with this display name passes [`GraphOptions::ref_filter`].
    pub fn filter_matches(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        let mut words = self
            .ref_filter
            .split(',')
            .map(str::trim)
            .filter(|w| !w.is_empty())
            .peekable();
        words.peek().is_none() || words.any(|w| name.contains(&w.to_lowercase()))
    }

    fn starts_history(&self, r: &GitRef, hidden: &BranchPatterns) -> bool {
        r.is_head || (self.name_starts_history(&r.name) && !hidden.matches(r.kind, &r.name))
    }

    /// For worktrees, which no hide list applies to.
    fn name_starts_history(&self, name: &str) -> bool {
        !self.current_branch_only && self.filter_matches(name)
    }

    pub fn shows(&self, kind: RefKind) -> bool {
        match kind {
            RefKind::LocalBranch => self.show_local_branches,
            RefKind::RemoteBranch => self.show_remote_branches,
            RefKind::Tag => self.show_tags,
            RefKind::Stash => self.show_stash,
            RefKind::DetachedHead => true,
            RefKind::Other => self.show_other_refs,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RevNode {
    pub commit: CommitIx,
    /// Indices into [`Repo::refs`] of the visible refs on this commit, in display order.
    pub refs: Vec<usize>,
    pub is_head: bool,
    /// More than one parent edge, whatever the commit's parents.
    pub is_merge: bool,
    /// Indices into the [`PullRequestHead`]s given to [`build_with_pull_requests`] of the pull
    /// requests whose head this commit is, in the order given.
    pub pull_requests: Vec<usize>,
    /// Indices into [`Repo::worktrees`] of the other worktrees whose detached HEAD this
    /// commit is, while worktrees are shown. A worktree on a branch is shown by the branch.
    pub worktrees: Vec<usize>,
}

/// An open pull request: its head commit, if that is in the repository, the refs of its base
/// branch, and the remote-tracking branch of its head branch, if fetched (indices into
/// [`Repo::refs`]). It labels its head, which it makes a node, as a tag does.
///
/// Its head branch starts history as that branch would with remote branches shown, whether
/// they are or not: the name filter, the hide list and "current branch only" apply to it, the
/// switch doesn't. It is left out if its base branch is on the hide list. With no ref of its
/// base branch at all, as for a fork's own pull request into a parent no remote points at, it
/// is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequestHead {
    pub commit: CommitIx,
    pub bases: Vec<usize>,
    pub head: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevEdge {
    pub child: u32,
    pub parent: u32,
    pub first_parent: bool,
    /// The number of commits collapsed into the edge.
    pub hidden: u32,
}

#[derive(Clone, Debug, Default)]
pub struct RevGraph {
    pub nodes: Vec<RevNode>,
    /// Grouped by child in node order, each child's first-parent edge first.
    pub edges: Vec<RevEdge>,
    node_of: Vec<Option<u32>>,
    represented_by: Vec<Option<u32>>,
    edge_commits: Vec<Vec<CommitIx>>,
    /// The number of commits reachable from the visible refs.
    pub visible_commits: usize,
    /// The number of branches left out because they match [`GraphOptions::hide_branches`].
    pub hidden_branches: usize,
}

impl RevGraph {
    pub fn node_of(&self, commit: CommitIx) -> Option<u32> {
        self.node_of.get(commit.ix()).copied().flatten()
    }

    /// The node the layout starts its leftmost line from: the first of
    /// [`Repo::layout_anchors`] that is a node. Never HEAD's, so that going to another
    /// worktree or switching branches leaves the columns where they are.
    pub fn layout_anchor(&self, repo: &Repo) -> Option<u32> {
        repo.layout_anchors()
            .into_iter()
            .find_map(|c| self.node_of(c))
    }

    /// The node a commit is shown as: itself if it is a node, else the end of an edge it is
    /// collapsed into. `None` if the commit is not visible, or if it is on no edge because it
    /// only leads to an empty-tree root that is not shown (see the module docs).
    pub fn represented_by(&self, commit: CommitIx) -> Option<u32> {
        self.represented_by.get(commit.ix()).copied().flatten()
    }

    /// The nodes that grow out of `roots`, sorted: the roots and every node whose first-parent
    /// line leads back to one of them. This is the subtree of the tree formed by each node's
    /// first edge (its first-parent edge where it has one), so a branch that forks off a root
    /// is included, while a merge that merely pulls a branch in is not: it belongs to the line
    /// it was merged into.
    pub fn subtree(&self, roots: &[usize]) -> Vec<usize> {
        let n = self.nodes.len();
        let mut children: Vec<Vec<u32>> = vec![Vec::new(); n];
        for group in self.edges.chunk_by(|a, b| a.child == b.child) {
            let first = group[0];
            children[first.parent as usize].push(first.child);
        }
        let mut inside = vec![false; n];
        let mut stack: Vec<usize> = roots.iter().copied().filter(|&r| r < n).collect();
        while let Some(node) = stack.pop() {
            if !std::mem::replace(&mut inside[node], true) {
                stack.extend(children[node].iter().map(|&c| c as usize));
            }
        }
        (0..n).filter(|&i| inside[i]).collect()
    }

    /// The commits collapsed into edge `e` (an index into [`RevGraph::edges`]), newest
    /// first. A commit below a hidden fork is on the edges of both sides.
    pub fn collapsed(&self, e: usize) -> &[CommitIx] {
        &self.edge_commits[e]
    }
}

const NO_REP: u32 = u32::MAX;

/// A kept commit that a commit's parent leads to.
#[derive(Clone, Copy, Debug)]
struct Link {
    target: u32,
    first_parent: bool,
}

pub fn build(repo: &Repo, options: &GraphOptions) -> RevGraph {
    build_with_pull_requests(repo, options, &[])
}

/// With the heads of open pull requests as labels, if [`GraphOptions::show_pull_requests`].
pub fn build_with_pull_requests(
    repo: &Repo,
    options: &GraphOptions,
    pull_requests: &[PullRequestHead],
) -> RevGraph {
    let n = repo.commits.len();
    let head = repo.head_commit().map(CommitIx::ix);

    // Branches checked out in other worktrees, shown whatever the switches and hide list say.
    let in_worktree: Vec<bool> = repo
        .refs
        .iter()
        .map(|r| {
            options.show_worktrees
                && r.kind == RefKind::LocalBranch
                && repo.worktrees_on(&r.full_name).next().is_some()
        })
        .collect();

    let mut refs_on: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut decorated = vec![false; n];
    let mut first_parent_only = vec![options.first_parent_only; n];
    for (i, r) in repo.refs.iter().enumerate() {
        if !(options.shows(r.kind) || r.is_head || in_worktree[i]) {
            continue;
        }
        let t = r.target.ix();
        refs_on[t].push(i);
        decorated[t] |= r.kind != RefKind::Tag || options.tags_make_nodes;
        // A stash commit's other parents are its index/untracked snapshots: noise.
        first_parent_only[t] |= r.kind == RefKind::Stash;
    }
    if let Some(h) = head {
        decorated[h] = true;
    }
    let parents_of = |c: usize| -> &[CommitIx] {
        let parents = &repo.commits[c].parents;
        if first_parent_only[c] {
            &parents[..parents.len().min(1)]
        } else {
            parents
        }
    };

    // Worktrees pass the hide list, which is for bulk branches nobody checks out, but not the
    // name filter or "current branch only".
    let hidden = BranchPatterns::parse(&options.hide_branches);
    let unhidden = BranchPatterns::default();
    let starts_history = |i: usize| {
        let r = &repo.refs[i];
        options.starts_history(r, if in_worktree[i] { &unhidden } else { &hidden })
    };
    let detached: Vec<(usize, usize)> = repo
        .worktrees
        .iter()
        .enumerate()
        .filter(|(_, w)| options.show_worktrees && !w.open && w.branch.is_none())
        .filter_map(|(k, w)| Some((k, w.head?.ix())))
        .filter(|&(k, c)| c < n && options.name_starts_history(&repo.worktrees[k].name()))
        .collect();
    let hidden_ref = |i: usize| hidden.matches(repo.refs[i].kind, &repo.refs[i].name);
    let shown_pull_requests: Vec<bool> = pull_requests
        .iter()
        .map(|pr| {
            options.show_pull_requests
                && pr.commit.ix() < n
                && (pr.bases.is_empty() || pr.bases.iter().any(|&i| !hidden_ref(i)))
                && !pr.head.is_some_and(hidden_ref)
        })
        .collect();
    let mut visible = vec![false; n];
    let mut stack: Vec<usize> = (0..n)
        .filter(|&c| refs_on[c].iter().any(|&i| starts_history(i)))
        .collect();
    stack.extend(
        pull_requests
            .iter()
            .zip(&shown_pull_requests)
            .filter(|&(pr, &shown)| shown && pr.head.is_some_and(starts_history))
            .map(|(pr, _)| pr.commit.ix()),
    );
    stack.extend(head);
    stack.extend(detached.iter().map(|&(_, c)| c));
    let mut visible_commits = 0;
    while let Some(c) = stack.pop() {
        if !visible[c] {
            visible[c] = true;
            visible_commits += 1;
            stack.extend(parents_of(c).iter().map(|p| p.ix()));
        }
    }

    // Branches that would start history but for the hide list, and that no shown ref reaches.
    let hidden_branches = repo
        .refs
        .iter()
        .filter(|r| {
            options.shows(r.kind)
                && !visible[r.target.ix()]
                && hidden.matches(r.kind, &r.name)
                && options.starts_history(r, &unhidden)
        })
        .count();

    let mut pulls_on: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (k, pr) in pull_requests.iter().enumerate() {
        let c = pr.commit.ix();
        if shown_pull_requests[k] && visible[c] {
            pulls_on[c].push(k);
            decorated[c] = true;
        }
    }

    let mut worktrees_on: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(k, c) in &detached {
        worktrees_on[c].push(k);
        decorated[c] = true;
    }

    // The commit a branch was rebased from stays a node while the branch and its upstream are
    // apart, wherever it is shown anyway.
    if options.show_upstreams {
        for c in repo.upstreams.iter().filter_map(|u| u.rebased_from) {
            if c.ix() < n && visible[c.ix()] {
                decorated[c.ix()] = true;
            }
        }
    }

    let mut child_count = vec![0u32; n];
    let mut has_merge_child = vec![false; n];
    for c in (0..n).filter(|&c| visible[c]) {
        let ps = parents_of(c);
        for p in ps {
            child_count[p.ix()] += 1;
            has_merge_child[p.ix()] |= ps.len() > 1;
        }
    }
    let order = parents_first_order(n, &visible, &child_count, &parents_of);

    let mode = options.simplification;
    let simplified = matches!(mode, Simplification::Decorated | Simplification::Forks);
    let fork = if mode == Simplification::Forks {
        fork_points(n, &order, &decorated, &parents_of)
    } else {
        vec![false; n]
    };

    let treesame_root = |c: usize| {
        simplified && parents_of(c).is_empty() && repo.commits[c].empty_tree && !decorated[c]
    };
    let mut rep = vec![NO_REP; n];
    let mut kept_edges: Vec<Vec<Link>> = vec![Vec::new(); n];
    let mut ancestry = Ancestry::new(n);
    let mut candidates: Vec<Link> = Vec::new();
    // In the forks mode merges are not nodes: a hidden commit stands for all its nearest kept
    // ancestors, so the nodes above a merge reach both sides.
    let mut reach: Vec<Vec<Link>> = vec![Vec::new(); n];
    for &c in &order {
        let ps = parents_of(c);
        candidates.clear();
        for (k, p) in ps.iter().enumerate() {
            let p = p.ix();
            if mode == Simplification::Forks && rep[p] != p as u32 {
                for link in &reach[p] {
                    if !candidates.iter().any(|x| x.target == link.target) {
                        candidates.push(Link {
                            target: link.target,
                            first_parent: k == 0 && link.first_parent,
                        });
                    }
                }
                continue;
            }
            let target = rep[p];
            if target != NO_REP && !candidates.iter().any(|x| x.target == target) {
                candidates.push(Link {
                    target,
                    first_parent: k == 0,
                });
            }
        }
        if simplified && candidates.len() > 1 {
            ancestry.drop_redundant(&mut candidates, &kept_edges);
            // git's mark_treesame_root_parents, keeping one if that would leave none.
            if candidates.iter().any(|x| !treesame_root(x.target as usize)) {
                candidates.retain(|x| !treesame_root(x.target as usize));
            } else {
                candidates.truncate(1);
            }
        }
        let kept = match mode {
            Simplification::AllCommits => true,
            Simplification::BranchesAndMerges => {
                decorated[c] || ps.len() != 1 || child_count[c] != 1 || has_merge_child[c]
            }
            Simplification::Decorated => decorated[c] || ps.is_empty() || candidates.len() > 1,
            Simplification::Forks => decorated[c] || fork[c] || ps.is_empty(),
        };
        if kept || candidates.is_empty() {
            rep[c] = c as u32;
            ancestry.add(c, &candidates);
            kept_edges[c] = candidates.clone();
        } else {
            rep[c] = candidates[0].target;
            if mode == Simplification::Forks {
                reach[c] = candidates.clone();
            }
        }
    }

    // Treesame roots are only shown where an edge ends on them.
    let mut referenced = vec![false; n];
    for c in (0..n).filter(|&c| rep[c] == c as u32) {
        for link in &kept_edges[c] {
            referenced[link.target as usize] = true;
        }
    }
    let is_node =
        |c: usize| visible[c] && rep[c] == c as u32 && (referenced[c] || !treesame_root(c));

    // In commit order: git log's, newest first, roughly.
    let mut node_of = vec![None; n];
    let mut nodes = Vec::new();
    for c in (0..n).filter(|&c| is_node(c)) {
        node_of[c] = Some(nodes.len() as u32);
        let mut refs = std::mem::take(&mut refs_on[c]);
        refs.sort_by(|&a, &b| crate::repo::cmp_refs_for_display(&repo.refs[a], &repo.refs[b]));
        nodes.push(RevNode {
            commit: CommitIx(c as u32),
            refs,
            is_head: Some(c) == head,
            is_merge: kept_edges[c].len() > 1,
            pull_requests: std::mem::take(&mut pulls_on[c]),
            worktrees: std::mem::take(&mut worktrees_on[c]),
        });
    }

    let mut edges = Vec::new();
    for (ni, node) in nodes.iter().enumerate() {
        let mut out: Vec<RevEdge> = kept_edges[node.commit.ix()]
            .iter()
            .map(|link| RevEdge {
                child: ni as u32,
                parent: node_of[link.target as usize].expect("representatives are nodes"),
                first_parent: link.first_parent,
                hidden: 0,
            })
            .collect();
        out.sort_by_key(|e| !e.first_parent);
        edges.extend(out);
    }

    let mut represented_by: Vec<Option<u32>> = (0..n)
        .map(|c| {
            (rep[c] != NO_REP)
                .then(|| node_of[rep[c] as usize])
                .flatten()
        })
        .collect();

    // An edge is not one run of commits: across a hidden merge it has both sides, and in the
    // forks mode a hidden commit can stand for several nodes. It holds the child's own commits,
    // those no end of its edges has, each on the edge of the parent it is reached through (else
    // the child's first edge).
    let mut edge_commits: Vec<Vec<CommitIx>> = vec![Vec::new(); edges.len()];
    let mut pos = vec![0u32; n];
    for (i, &c) in order.iter().enumerate() {
        pos[c] = i as u32;
    }
    let mut walk = OwnCommits::new(n);
    let mut own = Vec::new();
    let mut start = 0;
    for group in edges.chunk_by(|a, b| a.child == b.child) {
        let ps = parents_of(nodes[group[0].child as usize].commit.ix());
        let target = |e: &RevEdge| nodes[e.parent as usize].commit.ix() as u32;
        let through = |k: usize, t: u32| {
            let p = ps[k].ix();
            if rep[p] == p as u32 || mode != Simplification::Forks {
                rep[p] == t
            } else {
                reach[p].iter().any(|link| link.target == t)
            }
        };
        let origin: Vec<Option<usize>> = group
            .iter()
            .map(|e| (0..ps.len()).find(|&k| through(k, target(e))))
            .collect();
        let targets: Vec<usize> = group.iter().map(|e| target(e) as usize).collect();
        walk.run(
            ps,
            &targets,
            &pos,
            |c| node_of[c].is_none(),
            &parents_of,
            &mut own,
        );
        for &(c, k) in &own {
            let e = start + origin.iter().position(|&o| o == Some(k)).unwrap_or(0);
            edge_commits[e].push(CommitIx(c as u32));
        }
        start += group.len();
    }
    // A commit on several edges is shown as its representative if one of them ends there,
    // else as the end of the first.
    let mut settled = vec![false; n];
    for (e, commits) in edges.iter().zip(&edge_commits) {
        for c in commits {
            settled[c.ix()] |= represented_by[c.ix()] == Some(e.parent);
        }
    }
    for (e, commits) in edges.iter_mut().zip(&edge_commits) {
        e.hidden = commits.len() as u32;
        for c in commits {
            if !std::mem::replace(&mut settled[c.ix()], true) {
                represented_by[c.ix()] = Some(e.parent);
            }
        }
    }

    RevGraph {
        nodes,
        edges,
        node_of,
        represented_by,
        edge_commits,
        visible_commits,
        hidden_branches,
    }
}

fn parents_first_order<'a>(
    n: usize,
    visible: &[bool],
    child_count: &[u32],
    parents_of: &impl Fn(usize) -> &'a [CommitIx],
) -> Vec<usize> {
    let mut pending = child_count.to_vec();
    let mut stack: Vec<usize> = (0..n)
        .rev()
        .filter(|&c| visible[c] && pending[c] == 0)
        .collect();
    let mut order = Vec::with_capacity(n);
    while let Some(c) = stack.pop() {
        order.push(c);
        for p in parents_of(c) {
            let p = p.ix();
            pending[p] -= 1;
            if pending[p] == 0 {
                stack.push(p);
            }
        }
    }
    order.reverse();
    order
}

/// The commits where labelled histories fork apart: the decorated reduction of the reversed
/// graph, children first.
fn fork_points<'a>(
    n: usize,
    order: &[usize],
    decorated: &[bool],
    parents_of: &impl Fn(usize) -> &'a [CommitIx],
) -> Vec<bool> {
    let mut children: Vec<Vec<u32>> = vec![Vec::new(); n];
    for &c in order {
        for p in parents_of(c) {
            children[p.ix()].push(c as u32);
        }
    }
    let mut fork = vec![false; n];
    let mut rep = vec![NO_REP; n];
    let mut kept_edges: Vec<Vec<Link>> = vec![Vec::new(); n];
    let mut ancestry = Ancestry::new(n);
    let mut candidates: Vec<Link> = Vec::new();
    for &c in order.iter().rev() {
        candidates.clear();
        for &k in &children[c] {
            let target = rep[k as usize];
            if !candidates.iter().any(|x| x.target == target) {
                candidates.push(Link {
                    target,
                    first_parent: false,
                });
            }
        }
        if candidates.len() > 1 {
            ancestry.drop_redundant(&mut candidates, &kept_edges);
        }
        fork[c] = candidates.len() > 1;
        if fork[c] || decorated[c] || candidates.is_empty() {
            rep[c] = c as u32;
            ancestry.add(c, &candidates);
            kept_edges[c] = candidates.clone();
        } else {
            rep[c] = candidates[0].target;
        }
    }
    fork
}

/// A node's own commits, found as git's merge-base walk does: history is painted down from
/// the node's hidden parents and from the ends of its edges at once, newest first, and a
/// commit painted only from the node is its own. Stops once no such commit is left to paint.
struct OwnCommits {
    flags: Vec<u8>,
    /// For a commit painted from the node, the index of the node's parent it came through.
    origin: Vec<usize>,
    touched: Vec<usize>,
    queue: BinaryHeap<(u32, usize)>,
    /// Painted commits still queued that only the node has.
    live: usize,
}

const FROM_NODE: u8 = 1;
const FROM_TARGET: u8 = 2;

impl OwnCommits {
    fn new(n: usize) -> OwnCommits {
        OwnCommits {
            flags: vec![0; n],
            origin: vec![0; n],
            touched: Vec::new(),
            queue: BinaryHeap::new(),
            live: 0,
        }
    }

    /// The commits, newest first, that are reached from `parents` through commits for which
    /// `hidden` holds and that no commit of `targets` has, each with the index into `parents`
    /// it is reached through. `pos` orders commits parents first.
    fn run<'a>(
        &mut self,
        parents: &[CommitIx],
        targets: &[usize],
        pos: &[u32],
        hidden: impl Fn(usize) -> bool,
        parents_of: &impl Fn(usize) -> &'a [CommitIx],
        out: &mut Vec<(usize, usize)>,
    ) {
        out.clear();
        for &t in targets {
            self.paint(t, FROM_TARGET, 0, pos);
        }
        for (k, p) in parents.iter().enumerate() {
            if hidden(p.ix()) {
                self.paint(p.ix(), FROM_NODE, k, pos);
            }
        }
        while self.live > 0 {
            let Some((_, c)) = self.queue.pop() else {
                break;
            };
            let flags = self.flags[c];
            if flags == FROM_NODE {
                self.live -= 1;
                out.push((c, self.origin[c]));
            }
            // Only the ends' paint goes past nodes.
            for p in parents_of(c) {
                if flags != FROM_NODE || hidden(p.ix()) {
                    self.paint(p.ix(), flags, self.origin[c], pos);
                }
            }
        }
        for c in self.touched.drain(..) {
            self.flags[c] = 0;
        }
        self.queue.clear();
        self.live = 0;
    }

    fn paint(&mut self, c: usize, flag: u8, origin: usize, pos: &[u32]) {
        let old = self.flags[c];
        if old == 0 {
            self.touched.push(c);
            self.queue.push((pos[c], c));
            self.origin[c] = origin;
        }
        let new = old | flag;
        self.flags[c] = new;
        self.live = self.live + usize::from(new == FROM_NODE) - usize::from(old == FROM_NODE);
    }
}

/// Ancestry among kept commits.
struct Ancestry {
    /// The longest distance to a root over kept edges; an ancestor always has a smaller one.
    generation: Vec<u32>,
    /// The search that last reached every commit.
    stamp: Vec<u32>,
    search: u32,
    stack: Vec<u32>,
}

impl Ancestry {
    fn new(n: usize) -> Ancestry {
        Ancestry {
            generation: vec![0; n],
            stamp: vec![0; n],
            search: 0,
            stack: Vec::new(),
        }
    }

    fn add(&mut self, c: usize, parents: &[Link]) {
        self.generation[c] = parents
            .iter()
            .map(|link| self.generation[link.target as usize] + 1)
            .max()
            .unwrap_or(0);
    }

    /// Drops candidates that are ancestors of other candidates, as git's `simplify_merges`.
    fn drop_redundant(&mut self, candidates: &mut Vec<Link>, edges: &[Vec<Link>]) {
        let targets: Vec<u32> = candidates.iter().map(|link| link.target).collect();
        let keep: Vec<bool> = targets
            .iter()
            .map(|&a| {
                !targets
                    .iter()
                    .any(|&b| a != b && self.is_ancestor(a, b, edges))
            })
            .collect();
        let mut keep = keep.into_iter();
        candidates.retain(|_| keep.next() == Some(true));
    }

    /// Whether kept commit `a` is a proper ancestor of kept commit `b`.
    fn is_ancestor(&mut self, a: u32, b: u32, edges: &[Vec<Link>]) -> bool {
        let floor = self.generation[a as usize];
        self.search += 1;
        self.stack.clear();
        self.stack.push(b);
        while let Some(x) = self.stack.pop() {
            for link in &edges[x as usize] {
                let p = link.target;
                if p == a {
                    return true;
                }
                let pi = p as usize;
                if self.generation[pi] > floor && self.stamp[pi] != self.search {
                    self.stamp[pi] = self.search;
                    self.stack.push(p);
                }
            }
        }
        false
    }
}
