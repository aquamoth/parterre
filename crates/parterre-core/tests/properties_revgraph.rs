//! Property tests over random inputs (adapted from a review's fuzzing).

#![allow(clippy::needless_range_loop)] // index loops read better in these tests

use parterre_core::layout::{self, LayoutEdge, LayoutInput, LayoutOptions, Point, Ranking};
use parterre_core::pattern::BranchPatterns;
use parterre_core::revgraph::{self, GraphOptions, PullRequestHead, Simplification};
use parterre_core::{Commit, CommitIx, GitRef, Head, Oid, RefKind, Repo};
use std::collections::HashSet;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
    fn chance(&mut self, p: f64) -> bool {
        (self.next() % 1_000_000) as f64 / 1e6 < p
    }
}

fn oid(i: usize) -> Oid {
    Oid::from_hex(&format!("{:040x}", i + 1)).unwrap()
}

fn random_repo(rng: &mut Rng, n: usize) -> Repo {
    // Topological: commit i's parents have index > i, except with permutation later.
    let mut parents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for c in 0..n {
        let k = match rng.below(12) {
            0 => 0,
            1..=7 => 1,
            8..=10 => 2,
            _ => 3,
        };
        for _ in 0..k {
            if c + 1 >= n {
                break;
            }
            let span = if rng.chance(0.75) {
                1 + rng.below(3)
            } else {
                1 + rng.below((n - c - 1) as u64)
            };
            let p = (c as u64 + span).min(n as u64 - 1) as usize;
            if parents[c].contains(&p) && !rng.chance(0.2) {
                continue; // occasionally allow duplicate parents
            }
            parents[c].push(p);
        }
    }
    // Random permutation of indices (git log order with clock skew is not topological).
    let mut perm: Vec<usize> = (0..n).collect();
    if rng.chance(0.5) {
        for i in (1..n).rev() {
            let j = rng.below(i as u64 + 1) as usize;
            perm.swap(i, j);
        }
    }
    let mut commits: Vec<Option<Commit>> = vec![None; n];
    for c in 0..n {
        commits[perm[c]] = Some(Commit {
            oid: oid(perm[c]),
            parents: parents[c]
                .iter()
                .map(|&p| CommitIx(perm[p] as u32))
                .collect(),
            truncated: false,
            empty_tree: parents[c].is_empty()
                && std::env::var("NOEMPTY").is_err()
                && rng.chance(0.4),
            author_name: "a".into(),
            author_email: "a@b".into(),
            author_time: (n - c) as i64,
            author_date: String::new(),
            commit_time: (n - c) as i64,
            subject: format!("c{c}"),
        });
    }
    let commits: Vec<Commit> = commits.into_iter().map(Option::unwrap).collect();
    let mut refs = Vec::new();
    let kinds = [
        RefKind::LocalBranch,
        RefKind::RemoteBranch,
        RefKind::Tag,
        RefKind::Stash,
        RefKind::Other,
    ];
    let nrefs = rng.below(1 + n as u64 / 2) as usize;
    for i in 0..nrefs {
        let kind = kinds[rng.below(kinds.len() as u64) as usize];
        let t = rng.below(n as u64) as usize;
        refs.push(GitRef {
            full_name: format!("refs/x/{i}"),
            name: format!("r{i}"),
            kind,
            target: CommitIx(t as u32),
            annotated: false,
            is_head: false,
        });
    }
    let head = match rng.below(4) {
        0 if n > 0 => {
            let c = CommitIx(rng.below(n as u64) as u32);
            refs.push(GitRef {
                full_name: "HEAD".into(),
                name: "HEAD".into(),
                kind: RefKind::DetachedHead,
                target: c,
                annotated: false,
                is_head: true,
            });
            Head::Detached(c)
        }
        1 => Head::Branch {
            name: "refs/heads/unborn".into(),
            target: None,
        },
        _ => {
            // HEAD on a branch ref, if any local branch
            if let Some(i) = refs.iter().position(|r| r.kind == RefKind::LocalBranch) {
                refs[i].is_head = true;
                Head::Branch {
                    name: refs[i].full_name.clone(),
                    target: Some(refs[i].target),
                }
            } else {
                Head::Branch {
                    name: "refs/heads/unborn".into(),
                    target: None,
                }
            }
        }
    };
    Repo::new("/tmp/x".into(), commits, refs, head)
}

fn ancestors(repo: &Repo, c: usize) -> HashSet<usize> {
    let mut seen = HashSet::new();
    let mut stack: Vec<usize> = repo.commits[c].parents.iter().map(|p| p.ix()).collect();
    while let Some(x) = stack.pop() {
        if seen.insert(x) {
            stack.extend(repo.commits[x].parents.iter().map(|p| p.ix()));
        }
    }
    seen
}

fn all_options() -> Vec<GraphOptions> {
    let mut v = Vec::new();
    for s in Simplification::ALL {
        for bits in 0..128u32 {
            v.push(GraphOptions {
                simplification: s,
                show_local_branches: bits & 1 != 0,
                show_remote_branches: bits & 2 != 0,
                show_tags: bits & 4 != 0,
                tags_make_nodes: bits & 8 != 0,
                show_stash: bits & 16 != 0,
                show_other_refs: bits & 32 != 0,
                first_parent_only: bits & 64 != 0,
                current_branch_only: bits % 11 == 3,
                show_pull_requests: bits % 4 != 1,
                ref_filter: match bits % 5 {
                    0 => "r1".into(),
                    1 => "r2, r3".into(),
                    2 => " , ".into(),
                    _ => String::new(),
                },
                hide_branches: match bits % 3 {
                    0 => "r1*".into(),
                    1 => "R? x/*".into(),
                    _ => String::new(),
                },
            });
        }
    }
    v
}

#[test]
fn revgraph_invariants_random() {
    let mut rng = Rng(0xABCDEF12345);
    let opts_all = all_options();
    let mut represented_none = 0usize;
    let mut represented_none_example = None;
    for iter in 0..80 {
        let n = rng.below(if iter % 10 == 0 { 120 } else { 25 }) as usize;
        let repo = random_repo(&mut rng, n);
        let anc: Vec<HashSet<usize>> = (0..n).map(|c| ancestors(&repo, c)).collect();
        for (oi, opts) in opts_all.iter().enumerate() {
            if oi % 7 != iter % 7 && n > 30 {
                continue;
            }
            let g = revgraph::build(&repo, opts);
            let what = format!("iter {iter} opts {opts:?}");
            // node_of consistency
            let mut seen = HashSet::new();
            for (i, node) in g.nodes.iter().enumerate() {
                assert!(seen.insert(node.commit), "{what}: duplicate node");
                assert_eq!(g.node_of(node.commit), Some(i as u32), "{what}");
                assert_eq!(g.represented_by(node.commit), Some(i as u32), "{what}");
                let out = g.edges.iter().filter(|e| e.child as usize == i).count();
                assert_eq!(node.is_merge, out > 1, "{what}: is_merge");
            }
            let mut pairs = HashSet::new();
            let mut last_child = 0;
            let mut first_seen_for_child = false;
            for (k, e) in g.edges.iter().enumerate() {
                assert!(
                    (e.child as usize) < g.nodes.len() && (e.parent as usize) < g.nodes.len(),
                    "{what}"
                );
                assert_ne!(e.child, e.parent, "{what}: self loop");
                assert!(pairs.insert((e.child, e.parent)), "{what}: duplicate edge");
                let (cc, pc) = (
                    g.nodes[e.child as usize].commit.ix(),
                    g.nodes[e.parent as usize].commit.ix(),
                );
                assert!(anc[cc].contains(&pc), "{what}: edge to non-ancestor");
                // grouping
                if k == 0 || e.child != last_child {
                    assert!(
                        k == 0 || e.child > last_child,
                        "{what}: edges not grouped by child in order"
                    );
                    last_child = e.child;
                    first_seen_for_child = e.first_parent;
                } else {
                    assert!(
                        !e.first_parent || first_seen_for_child,
                        "{what}: first-parent edge not first"
                    );
                }
            }
            // head
            if let Some(h) = repo.head_commit() {
                let hn = g
                    .node_of(h)
                    .unwrap_or_else(|| panic!("{what}: head not a node"));
                assert!(g.nodes[hn as usize].is_head);
            }
            // decorated refs
            for (ri, r) in repo.refs.iter().enumerate() {
                if opts.shows(r.kind)
                    && (r.kind != RefKind::Tag || opts.tags_make_nodes)
                    && g.represented_by(r.target).is_some()
                {
                    let nn = g
                        .node_of(r.target)
                        .unwrap_or_else(|| panic!("{what}: decorated ref {r:?} not a node"));
                    assert!(
                        g.nodes[nn as usize].refs.contains(&ri),
                        "{what}: ref label missing"
                    );
                }
            }
            // visible commits all represented
            let hidden = BranchPatterns::parse(&opts.hide_branches);
            let mut visible = vec![false; n];
            let mut stack: Vec<usize> = repo
                .refs
                .iter()
                .filter(|r| {
                    (opts.shows(r.kind) || r.is_head)
                        && (r.is_head
                            || (!opts.current_branch_only
                                && opts.filter_matches(&r.name)
                                && !hidden.matches(r.kind, &r.name)))
                })
                .map(|r| r.target.ix())
                .collect();
            stack.extend(repo.head_commit().map(|c| c.ix()));
            let fpo_of: HashSet<usize> = repo
                .refs
                .iter()
                .filter(|r| r.kind == RefKind::Stash && opts.shows(r.kind))
                .map(|r| r.target.ix())
                .collect();
            while let Some(c) = stack.pop() {
                if !visible[c] {
                    visible[c] = true;
                    let ps = &repo.commits[c].parents;
                    let ps = if opts.first_parent_only || fpo_of.contains(&c) {
                        &ps[..ps.len().min(1)]
                    } else {
                        &ps[..]
                    };
                    stack.extend(ps.iter().map(|p| p.ix()));
                }
            }
            assert_eq!(
                g.visible_commits,
                visible.iter().filter(|&&v| v).count(),
                "{what}"
            );
            for c in 0..n {
                let r = g.represented_by(CommitIx(c as u32));
                if !visible[c] {
                    assert!(r.is_none(), "{what}: invisible commit represented");
                    assert!(g.node_of(CommitIx(c as u32)).is_none());
                } else if r.is_none() {
                    represented_none += 1;
                    if represented_none_example.is_none() {
                        represented_none_example = Some(what.clone());
                    }
                }
            }
            if opts.simplification == Simplification::AllCommits {
                assert_eq!(g.nodes.len(), g.visible_commits, "{what}");
            }
            // pull requests: a head is labelled exactly when it is visible and one of its base
            // refs is shown; they change nothing else about what is visible
            if n > 0 {
                let mut pr_rng = Rng(0x5EED_0000 + (iter * 1000 + oi) as u64);
                let heads: Vec<PullRequestHead> = (0..pr_rng.below(5))
                    .map(|_| PullRequestHead {
                        commit: CommitIx(pr_rng.below(n as u64) as u32),
                        bases: (0..repo.refs.len())
                            .filter(|_| pr_rng.chance(0.3))
                            .collect(),
                    })
                    .collect();
                let gp = revgraph::build_with_pull_requests(&repo, opts, &heads);
                assert_eq!(gp.visible_commits, g.visible_commits, "{what}");
                let mut seen = HashSet::new();
                for node in &gp.nodes {
                    assert!(seen.insert(node.commit), "{what}: duplicate node");
                }
                for e in &gp.edges {
                    let (cc, pc) = (
                        gp.nodes[e.child as usize].commit.ix(),
                        gp.nodes[e.parent as usize].commit.ix(),
                    );
                    assert!(anc[cc].contains(&pc), "{what}: edge to non-ancestor");
                }
                for (k, h) in heads.iter().enumerate() {
                    let base_shown = h.bases.iter().any(|&i| {
                        let r = &repo.refs[i];
                        (opts.shows(r.kind) || r.is_head) && visible[r.target.ix()]
                    });
                    let labelled = opts.show_pull_requests && visible[h.commit.ix()] && base_shown;
                    let node = gp.node_of(h.commit);
                    let has = node.is_some_and(|x| gp.nodes[x as usize].pull_requests.contains(&k));
                    assert_eq!(has, labelled, "{what}: pull request {k} {h:?}");
                }
                let labels: usize = gp.nodes.iter().map(|x| x.pull_requests.len()).sum();
                assert!(labels <= heads.len(), "{what}");
                if !opts.show_pull_requests {
                    assert_eq!(gp.nodes.len(), g.nodes.len(), "{what}");
                }
            }
            // layout
            if oi % 13 == 0 {
                let input = LayoutInput {
                    sizes: vec![Point::new(80.0, 20.0); g.nodes.len()],
                    times: g
                        .nodes
                        .iter()
                        .map(|n| repo.commit(n.commit).commit_time)
                        .collect(),
                    edges: g
                        .edges
                        .iter()
                        .map(|e| LayoutEdge {
                            child: e.child,
                            parent: e.parent,
                            first_parent: e.first_parent,
                        })
                        .collect(),
                    priority: g
                        .nodes
                        .iter()
                        .position(|n| n.is_head)
                        .map(|h| vec![h as u32])
                        .unwrap_or_default(),
                };
                for r in Ranking::ALL {
                    let l = layout::layout(
                        &input,
                        &LayoutOptions {
                            ranking: r,
                            ..LayoutOptions::default()
                        },
                    );
                    for e in &input.edges {
                        assert!(
                            l.layers[e.parent as usize] > l.layers[e.child as usize],
                            "{what} {r:?}"
                        );
                    }
                }
            }
        }
    }
    eprintln!(
        "visible commits with represented_by == None: {represented_none} (e.g. {represented_none_example:?})"
    );
}
