mod common;

use common::TestRepo;
use parterre_core::revgraph::{self, GraphOptions, PullRequestHead, RevGraph, Simplification};
use parterre_core::{Head, RefKind, Repo};

/// Subjects of the graph's nodes, sorted, for order-independent comparison.
fn node_subjects(repo: &Repo, g: &RevGraph) -> Vec<String> {
    let mut s: Vec<String> = g
        .nodes
        .iter()
        .map(|n| repo.commit(n.commit).subject.clone())
        .collect();
    s.sort();
    s
}

/// Edges as (child subject, parent subject, first_parent, hidden), sorted.
fn edge_list(repo: &Repo, g: &RevGraph) -> Vec<(String, String, bool, u32)> {
    let subject = |node: u32| repo.commit(g.nodes[node as usize].commit).subject.clone();
    let mut e: Vec<_> = g
        .edges
        .iter()
        .map(|e| {
            (
                subject(e.child),
                subject(e.parent),
                e.first_parent,
                e.hidden,
            )
        })
        .collect();
    e.sort();
    e
}

fn with_mode(simplification: Simplification) -> GraphOptions {
    GraphOptions {
        simplification,
        ..GraphOptions::default()
    }
}

fn edge(c: &str, p: &str, first: bool, hidden: u32) -> (String, String, bool, u32) {
    (c.into(), p.into(), first, hidden)
}

/// main: A - B - C ------- M
///            \           /
/// feature:    D ------- E
fn feature_merge() -> TestRepo {
    let mut r = TestRepo::new();
    r.commit("A");
    r.commit("B");
    r.branch("feature");
    r.commit("D");
    r.commit("E");
    r.checkout("main");
    r.commit("C");
    r.merge("feature", "M");
    r
}

#[test]
fn loads_commits_refs_and_head() {
    let r = feature_merge();
    r.git(&["tag", "-a", "-m", "annotated", "v1", "HEAD~1"]);
    let repo = r.load();
    assert_eq!(repo.commits.len(), 6);
    assert!(
        matches!(&repo.head, Head::Branch { name, target: Some(_) } if name == "refs/heads/main")
    );
    let names: Vec<(&str, RefKind, bool)> = repo
        .refs
        .iter()
        .map(|r| (r.name.as_str(), r.kind, r.annotated))
        .collect();
    assert!(names.contains(&("main", RefKind::LocalBranch, false)));
    assert!(names.contains(&("feature", RefKind::LocalBranch, false)));
    assert!(names.contains(&("v1", RefKind::Tag, true)));
    let merge = repo.head_commit().unwrap();
    assert_eq!(repo.commit(merge).parents.len(), 2);
    let tag = repo.refs.iter().find(|r| r.name == "v1").unwrap();
    assert_eq!(
        repo.commit(tag.target).subject,
        "C",
        "annotated tags are peeled"
    );
}

#[test]
fn decorated_mode_matches_simplify_by_decoration() {
    let r = feature_merge();
    let repo = r.load();
    let g = revgraph::build(&repo, &with_mode(Simplification::Decorated));
    // B (fork point) and C are undecorated; M's first parent rewrites to A, which is an
    // ancestor of E, so git drops it and M hangs off E only.
    assert_eq!(node_subjects(&repo, &g), ["A", "E", "M"]);
    assert_eq!(
        edge_list(&repo, &g),
        [edge("E", "A", true, 2), edge("M", "E", false, 0)]
    );
}

#[test]
fn subtrees_follow_first_parents() {
    let r = feature_merge();
    let repo = r.load();
    // Subjects of the subtree grown from the nodes with the given subjects.
    let subtree = |g: &RevGraph, roots: &[&str]| {
        let subject = |i: usize| repo.commit(g.nodes[i].commit).subject.clone();
        let roots: Vec<usize> = (0..g.nodes.len())
            .filter(|&i| roots.contains(&subject(i).as_str()))
            .collect();
        let mut s: Vec<String> = g.subtree(&roots).into_iter().map(subject).collect();
        s.sort();
        s
    };
    let g = revgraph::build(&repo, &with_mode(Simplification::AllCommits));
    // The feature branch without the merge, which belongs to main.
    assert_eq!(subtree(&g, &["D"]), ["D", "E"]);
    assert_eq!(subtree(&g, &["C"]), ["C", "M"]);
    assert_eq!(subtree(&g, &["B"]), ["B", "C", "D", "E", "M"]);
    assert_eq!(subtree(&g, &["C", "E"]), ["C", "E", "M"]);
    // A node whose only edge is a merge edge hangs off that edge.
    let g = revgraph::build(&repo, &with_mode(Simplification::Decorated));
    assert_eq!(subtree(&g, &["E"]), ["E", "M"]);
}

#[test]
fn decorated_mode_keeps_merges_joining_independent_lines() {
    let mut r = feature_merge();
    // Tag C: now M's parents rewrite to C and E, neither an ancestor of the other.
    r.git(&["tag", "c-tag", "main~1"]);
    r.commit("F");
    let repo = r.load();
    let g = revgraph::build(&repo, &with_mode(Simplification::Decorated));
    assert_eq!(node_subjects(&repo, &g), ["A", "C", "E", "F", "M"]);
    assert_eq!(
        edge_list(&repo, &g),
        [
            edge("C", "A", true, 1),
            edge("E", "A", true, 2),
            edge("F", "M", true, 0),
            edge("M", "C", true, 0),
            edge("M", "E", false, 0),
        ]
    );
}

#[test]
fn branches_and_merges_mode_matches_tortoisegit_collapse() {
    let r = feature_merge();
    let repo = r.load();
    let g = revgraph::build(&repo, &with_mode(Simplification::BranchesAndMerges));
    // Kept: root A, fork point B, merge sources C and E, merge M. D is a pass-through.
    assert_eq!(node_subjects(&repo, &g), ["A", "B", "C", "E", "M"]);
    assert_eq!(
        edge_list(&repo, &g),
        [
            edge("B", "A", true, 0),
            edge("C", "B", true, 0),
            edge("E", "B", true, 1),
            edge("M", "C", true, 0),
            edge("M", "E", false, 0),
        ]
    );
}

#[test]
fn all_commits_mode_keeps_everything() {
    let r = feature_merge();
    let repo = r.load();
    let g = revgraph::build(&repo, &with_mode(Simplification::AllCommits));
    assert_eq!(g.nodes.len(), 6);
    assert_eq!(g.edges.len(), 6);
    assert!(g.edges.iter().all(|e| e.hidden == 0));
}

#[test]
fn hiding_a_branch_hides_its_exclusive_history() {
    let mut r = TestRepo::new();
    r.commit("A");
    r.branch("side");
    r.commit("S");
    r.checkout("main");
    r.commit("B");
    let repo = r.load();
    let mut opts = with_mode(Simplification::AllCommits);
    assert_eq!(revgraph::build(&repo, &opts).nodes.len(), 3);
    opts.show_local_branches = false;
    // Only HEAD's branch remains visible.
    let g = revgraph::build(&repo, &opts);
    assert_eq!(node_subjects(&repo, &g), ["A", "B"]);
}

#[test]
fn tags_need_not_create_nodes() {
    let mut r = TestRepo::new();
    r.commit("A");
    r.commit("B");
    r.git(&["tag", "t"]);
    r.commit("C");
    let repo = r.load();
    let mut opts = with_mode(Simplification::Decorated);
    assert_eq!(
        node_subjects(&repo, &revgraph::build(&repo, &opts)),
        ["A", "B", "C"]
    );
    opts.tags_make_nodes = false;
    let g = revgraph::build(&repo, &opts);
    assert_eq!(node_subjects(&repo, &g), ["A", "C"]);
    assert_eq!(edge_list(&repo, &g), [edge("C", "A", true, 1)]);
}

#[test]
fn stash_shows_as_single_edge_to_its_base() {
    let mut r = TestRepo::new();
    r.commit("A");
    std::fs::write(r.path().join("f.txt"), "x").unwrap();
    r.git(&["add", "f.txt"]);
    r.git(&["stash", "-q"]);
    let repo = r.load();
    let g = revgraph::build(&repo, &with_mode(Simplification::AllCommits));
    let stash = g
        .nodes
        .iter()
        .position(|n| n.refs.iter().any(|&i| repo.refs[i].kind == RefKind::Stash))
        .expect("stash node");
    assert_eq!(
        g.edges.iter().filter(|e| e.child == stash as u32).count(),
        1
    );
    assert_eq!(g.nodes.len(), 2, "index snapshot commit is not shown");
}

#[test]
fn detached_head_gets_a_label() {
    let mut r = TestRepo::new();
    r.commit("A");
    r.commit("B");
    r.checkout("HEAD~1");
    let repo = r.load();
    assert!(matches!(repo.head, Head::Detached(_)));
    let g = revgraph::build(&repo, &GraphOptions::default());
    let head = g.nodes.iter().find(|n| n.is_head).expect("head node");
    assert_eq!(repo.commit(head.commit).subject, "A");
    assert_eq!(repo.refs[head.refs[0]].kind, RefKind::DetachedHead);
}

#[test]
fn decorated_mode_drops_merges_of_empty_rooted_histories() {
    // An unrelated history whose root has an empty tree, merged in (like an svn import).
    let mut r = TestRepo::new();
    std::fs::write(r.path().join("f.txt"), "x").unwrap();
    r.git(&["add", "f.txt"]);
    r.commit("A");
    r.git(&["checkout", "-q", "--orphan", "imported"]);
    r.git(&["rm", "-q", "-r", "-f", "."]);
    r.commit("R");
    std::fs::write(r.path().join("g.txt"), "y").unwrap();
    r.git(&["add", "g.txt"]);
    r.commit("S");
    r.checkout("main");
    r.git(&[
        "merge",
        "-q",
        "--allow-unrelated-histories",
        "-m",
        "M",
        "imported",
    ]);
    r.git(&["branch", "-D", "imported"]);
    let repo = r.load();
    let g = revgraph::build(&repo, &with_mode(Simplification::Decorated));
    // git log --simplify-by-decoration shows only M; A is shown as the end of M's edge.
    assert_eq!(node_subjects(&repo, &g), ["A", "M"]);
    assert_eq!(edge_list(&repo, &g), [edge("M", "A", true, 0)]);
    let g = revgraph::build(&repo, &with_mode(Simplification::BranchesAndMerges));
    assert!(node_subjects(&repo, &g).contains(&"R".to_owned()));
}

#[test]
fn filters_limit_history_to_matching_refs() {
    let mut r = TestRepo::new();
    r.commit("A");
    r.branch("feature/x");
    r.commit("X");
    r.checkout("main");
    r.branch("bugfix/y");
    r.commit("Y");
    r.checkout("main");
    r.commit("B");
    r.git(&["tag", "t-on-b"]);
    let repo = r.load();

    let mut opts = with_mode(Simplification::AllCommits);
    assert_eq!(
        node_subjects(&repo, &revgraph::build(&repo, &opts)),
        ["A", "B", "X", "Y"]
    );

    opts.current_branch_only = true;
    let g = revgraph::build(&repo, &opts);
    assert_eq!(node_subjects(&repo, &g), ["A", "B"]);
    let labels: Vec<&str> = g
        .nodes
        .iter()
        .flat_map(|n| n.refs.iter().map(|&i| repo.refs[i].name.as_str()))
        .collect();
    assert!(
        labels.contains(&"t-on-b"),
        "refs inside the shown history keep their labels"
    );

    opts.current_branch_only = false;
    opts.ref_filter = "FEATURE, nothing".into();
    assert_eq!(
        node_subjects(&repo, &revgraph::build(&repo, &opts)),
        ["A", "B", "X"]
    );
}

/// Labels of the graph's nodes, by node subject.
fn labels(repo: &Repo, g: &RevGraph) -> Vec<(String, Vec<String>)> {
    let mut v: Vec<(String, Vec<String>)> = g
        .nodes
        .iter()
        .map(|n| {
            let mut names: Vec<String> =
                n.refs.iter().map(|&i| repo.refs[i].name.clone()).collect();
            names.sort();
            (repo.commit(n.commit).subject.clone(), names)
        })
        .collect();
    v.sort();
    v
}

/// A - B ------ M       main
/// |   |\      /
/// |   | R1 --'         release/1: merged, so it stays
/// |   Q - F            pipeline/q, feature/f: F grows out of Q, so Q stays
/// R2 - P               release/2, pipeline/p: leaves, so both go
#[test]
fn hidden_branches_vanish_only_as_leaves() {
    let mut r = TestRepo::new();
    r.commit("A");
    r.branch("release/2");
    let r2 = r.commit("R2");
    r.branch("pipeline/p");
    r.commit("P");
    r.checkout("main");
    r.commit("B");
    r.branch("release/1");
    r.commit("R1");
    r.checkout("main");
    r.branch("pipeline/q");
    let q = r.commit("Q");
    r.branch("feature/f");
    r.commit("F");
    r.checkout("main");
    r.merge("release/1", "M");
    r.git(&["update-ref", "refs/remotes/origin/release/2", &r2]);
    r.git(&["update-ref", "refs/remotes/origin/pipeline/q", &q]);
    let repo = r.load();

    let mut opts = with_mode(Simplification::AllCommits);
    assert_eq!(
        node_subjects(&repo, &revgraph::build(&repo, &opts)),
        ["A", "B", "F", "M", "P", "Q", "R1", "R2"]
    );

    opts.hide_branches = "pipeline/*, release/*".into();
    let g = revgraph::build(&repo, &opts);
    assert_eq!(node_subjects(&repo, &g), ["A", "B", "F", "M", "Q", "R1"]);
    assert_eq!(
        g.hidden_branches, 3,
        "release/2, origin/release/2, pipeline/p"
    );
    opts.ref_filter = "main, feature, pipeline".into();
    assert_eq!(
        revgraph::build(&repo, &opts).hidden_branches,
        1,
        "only pipeline/p; the filter takes out the release branches anyway"
    );
    opts.ref_filter.clear();
    let l = labels(&repo, &g);
    assert!(l.contains(&("R1".into(), vec!["release/1".into()])));
    assert!(l.contains(&(
        "Q".into(),
        vec!["origin/pipeline/q".into(), "pipeline/q".into()]
    )));

    // The current branch is never hidden; what grows out of it still is.
    r.checkout("release/2");
    let repo = r.load();
    let g = revgraph::build(&repo, &opts);
    assert_eq!(
        node_subjects(&repo, &g),
        ["A", "B", "F", "M", "Q", "R1", "R2"]
    );
    assert!(labels(&repo, &g).contains(&(
        "R2".into(),
        vec!["origin/release/2".into(), "release/2".into()]
    )));

    // Hidden branches stay out of Labelled commits too: the decorated R1 and Q remain nodes.
    let g = revgraph::build(
        &repo,
        &GraphOptions {
            hide_branches: "pipeline/* release/*".into(),
            current_branch_only: false,
            ..with_mode(Simplification::Decorated)
        },
    );
    assert_eq!(node_subjects(&repo, &g), ["A", "F", "M", "Q", "R1", "R2"]);
}

#[test]
fn reads_full_commit_messages() {
    let mut r = TestRepo::new();
    r.commit("Subject line\n\nBody paragraph\nsecond line");
    let repo = r.load();
    let c = repo.head_commit().unwrap();
    assert_eq!(repo.commit(c).subject, "Subject line");
    let msg = parterre_core::git::Git::new(r.path())
        .message(&repo.commit(c).oid)
        .unwrap();
    assert_eq!(msg, "Subject line\n\nBody paragraph\nsecond line");
}

#[test]
fn lists_commits_collapsed_into_an_edge() {
    let r = feature_merge();
    let repo = r.load();
    let g = revgraph::build(&repo, &with_mode(Simplification::BranchesAndMerges));
    let subject = |c: parterre_core::CommitIx| repo.commit(c).subject.clone();
    // E -> B collapses D.
    let e_to_b = g
        .edges
        .iter()
        .find(|e| subject(g.nodes[e.child as usize].commit) == "E")
        .copied()
        .unwrap();
    let hidden: Vec<String> = g
        .collapsed_commits(&repo, e_to_b, 10)
        .into_iter()
        .map(subject)
        .collect();
    assert_eq!(hidden, ["D"]);

    let g = revgraph::build(&repo, &with_mode(Simplification::Decorated));
    // E -> A collapses D and B (newest first).
    let e_to_a = g
        .edges
        .iter()
        .find(|e| subject(g.nodes[e.child as usize].commit) == "E")
        .copied()
        .unwrap();
    assert_eq!(e_to_a.hidden, 2);
    let hidden: Vec<String> = g
        .collapsed_commits(&repo, e_to_a, 10)
        .into_iter()
        .map(subject)
        .collect();
    assert_eq!(hidden, ["D", "B"]);
    assert_eq!(g.collapsed_commits(&repo, e_to_a, 1).len(), 1);
}

#[test]
fn tags_of_tags_are_peeled_to_their_commit() {
    let mut r = TestRepo::new();
    r.commit("A");
    r.git(&["tag", "-a", "-m", "one", "t1"]);
    r.git(&["tag", "-a", "-m", "two", "t2", "t1"]);
    r.commit("B");
    let repo = r.load();
    let t2 = repo
        .refs
        .iter()
        .find(|r| r.name == "t2")
        .expect("nested tag loaded");
    assert_eq!(repo.commit(t2.target).subject, "A");
    assert!(t2.annotated);
}

#[test]
fn separator_characters_in_subjects_and_names_are_harmless() {
    let mut r = TestRepo::new();
    r.git(&["config", "user.name", "Wei\x1fZhang"]);
    r.commit("subject with \x1e record and \x1f unit separators");
    r.commit("second");
    let repo = r.load();
    assert_eq!(repo.commits.len(), 2);
    let first = repo.commits.iter().find(|c| c.parents.is_empty()).unwrap();
    assert_eq!(
        first.subject,
        "subject with \x1e record and \x1f unit separators"
    );
    assert_eq!(first.author_name, "Wei\x1fZhang");
    assert!(first.commit_time > 0);
}

#[test]
fn empty_repository_loads_without_commits() {
    let r = TestRepo::new();
    let repo = r.load();
    assert!(repo.commits.is_empty());
    assert!(matches!(repo.head, Head::Branch { target: None, .. }));
    let g = revgraph::build(&repo, &GraphOptions::default());
    assert!(g.nodes.is_empty());
}

#[test]
fn loads_from_inside_the_git_directory() {
    let mut r = TestRepo::new();
    r.commit("A");
    let repo = parterre_core::git::load_repo(&r.path().join(".git")).expect("load from .git");
    assert_eq!(repo.commits.len(), 1);
}

#[test]
fn notes_are_not_walked() {
    let mut r = TestRepo::new();
    r.commit("A");
    r.git(&["notes", "add", "-m", "a note"]);
    let repo = r.load();
    assert_eq!(
        repo.commits.len(),
        1,
        "the notes commit is not part of the history"
    );
    assert!(
        repo.refs
            .iter()
            .all(|r| !r.full_name.starts_with("refs/notes/"))
    );
}

/// main: A - B - C           origin/main at C
///            \
/// feature:    D - E         origin/feature at E, a pull request's head at D
fn pull_request_repo() -> TestRepo {
    let mut r = TestRepo::new();
    r.commit("A");
    r.commit("B");
    r.branch("feature");
    r.commit("D");
    r.commit("E");
    r.checkout("main");
    r.commit("C");
    r.git(&["update-ref", "refs/remotes/origin/main", "main"]);
    r.git(&["update-ref", "refs/remotes/origin/feature", "feature"]);
    r.git(&["branch", "-D", "feature"]);
    r
}

/// A pull request's head on the commit with `subject`, into the branch named `base`.
fn pull_request_head(repo: &Repo, subject: &str, base: &str) -> PullRequestHead {
    PullRequestHead {
        commit: repo
            .commits
            .iter()
            .position(|c| c.subject == subject)
            .map(|i| parterre_core::CommitIx(i as u32))
            .unwrap(),
        bases: (0..repo.refs.len())
            .filter(|&i| repo.refs[i].name == base)
            .collect(),
    }
}

/// Subjects of the nodes that carry pull requests, with the pull requests' indices.
fn pull_request_nodes(repo: &Repo, g: &RevGraph) -> Vec<(String, Vec<usize>)> {
    g.nodes
        .iter()
        .filter(|n| !n.pull_requests.is_empty())
        .map(|n| {
            (
                repo.commit(n.commit).subject.clone(),
                n.pull_requests.clone(),
            )
        })
        .collect()
}

#[test]
fn pull_request_heads_label_commits_and_make_them_nodes() {
    let repo = pull_request_repo().load();
    let heads = [pull_request_head(&repo, "D", "origin/main")];
    let mut opts = with_mode(Simplification::Decorated);
    opts.show_pull_requests = true;
    let g = revgraph::build_with_pull_requests(&repo, &opts, &heads);
    // D was collapsed into the edge from E; as a pull request's head it is a node of its own.
    assert_eq!(node_subjects(&repo, &g), ["A", "C", "D", "E"]);
    assert_eq!(pull_request_nodes(&repo, &g), [("D".to_owned(), vec![0])]);
    assert!(
        g.nodes
            .iter()
            .all(|n| n.refs.iter().all(|&r| r < repo.refs.len()))
    );

    // Turned off, they are ignored.
    opts.show_pull_requests = false;
    let g = revgraph::build_with_pull_requests(&repo, &opts, &heads);
    assert_eq!(node_subjects(&repo, &g), ["A", "C", "E"]);
    assert!(pull_request_nodes(&repo, &g).is_empty());
}

#[test]
fn pull_requests_need_a_visible_base_and_head() {
    let repo = pull_request_repo().load();
    let mut opts = with_mode(Simplification::Decorated);
    opts.show_pull_requests = true;

    // Into a branch that isn't in the repository, or not shown.
    let unknown = [pull_request_head(&repo, "D", "origin/gone")];
    let g = revgraph::build_with_pull_requests(&repo, &opts, &unknown);
    assert!(pull_request_nodes(&repo, &g).is_empty());
    let heads = [pull_request_head(&repo, "D", "origin/main")];
    let g = revgraph::build_with_pull_requests(
        &repo,
        &GraphOptions {
            show_remote_branches: false,
            ..opts.clone()
        },
        &heads,
    );
    assert!(pull_request_nodes(&repo, &g).is_empty());

    // A head that no shown branch reaches does not bring its history in: pull requests label
    // commits, they don't start history.
    let into_main = [pull_request_head(&repo, "E", "main")];
    let g = revgraph::build_with_pull_requests(
        &repo,
        &GraphOptions {
            current_branch_only: true,
            ..opts.clone()
        },
        &into_main,
    );
    assert_eq!(node_subjects(&repo, &g), ["A", "C"]);
    assert!(pull_request_nodes(&repo, &g).is_empty());

    // Several on one commit, in the order given.
    let two = [
        pull_request_head(&repo, "C", "main"),
        pull_request_head(&repo, "E", "main"),
        pull_request_head(&repo, "C", "origin/main"),
    ];
    let g = revgraph::build_with_pull_requests(&repo, &opts, &two);
    assert_eq!(
        pull_request_nodes(&repo, &g),
        [("C".to_owned(), vec![0, 2]), ("E".to_owned(), vec![1])]
    );
}
