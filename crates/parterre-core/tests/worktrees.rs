mod common;

use std::path::Path;
use std::process::Command;

use common::TestRepo;
use parterre_core::git::load_repo;
use parterre_core::revgraph::{self, GraphOptions};
use parterre_core::{RefKind, Repo};

/// Runs git in `dir`, as [`TestRepo::git`] does in the repository.
fn git_in(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// main: A - B, with `topic` (one commit past B) checked out in `wt-topic`, a detached
/// worktree at A with a commit of its own (`detached-only`), and `gone` (one commit past B)
/// in a worktree whose folder was deleted.
fn with_worktrees() -> (TestRepo, tempfile::TempDir) {
    let mut r = TestRepo::new();
    r.commit("A");
    r.commit("B");
    let others = tempfile::tempdir().expect("tempdir");
    let at = |name: &str| others.path().join(name).to_string_lossy().into_owned();
    r.git(&["worktree", "add", "-q", "-b", "topic", &at("wt-topic")]);
    let commit = ["commit", "-q", "--allow-empty", "-m"];
    git_in(
        &others.path().join("wt-topic"),
        &[&commit[..], &["topic-only"]].concat(),
    );
    r.git(&[
        "worktree",
        "add",
        "-q",
        "--detach",
        &at("wt-detached"),
        "HEAD~1",
    ]);
    let detached = others.path().join("wt-detached");
    git_in(&detached, &[&commit[..], &["detached-only"]].concat());
    r.git(&["worktree", "add", "-q", "-b", "gone", &at("wt-gone")]);
    git_in(
        &others.path().join("wt-gone"),
        &[&commit[..], &["gone-only"]].concat(),
    );
    std::fs::remove_dir_all(others.path().join("wt-gone")).expect("delete a worktree");
    (r, others)
}

fn subjects_with_worktrees(repo: &Repo, options: &GraphOptions) -> Vec<(String, Vec<String>)> {
    let g = revgraph::build(repo, options);
    let mut out: Vec<(String, Vec<String>)> = g
        .nodes
        .iter()
        .map(|n| {
            let names = n.worktrees.iter().map(|&k| repo.worktrees[k].name());
            (repo.commit(n.commit).subject.clone(), names.collect())
        })
        .collect();
    out.sort();
    out
}

#[test]
fn worktrees_are_listed_with_their_heads() {
    let (r, _others) = with_worktrees();
    let repo = load_repo(r.path()).expect("load");
    let names: Vec<String> = repo.worktrees.iter().map(|w| w.name()).collect();
    // The main worktree first, then by path.
    assert_eq!(names[1..], ["wt-detached", "wt-gone", "wt-topic"]);
    let [main, detached, gone, topic] = &repo.worktrees[..] else {
        panic!("four worktrees: {names:?}");
    };
    assert!(main.open && !detached.open && !topic.open);
    assert_eq!(main.branch.as_deref(), Some("refs/heads/main"));
    assert_eq!(topic.branch.as_deref(), Some("refs/heads/topic"));
    assert_eq!(detached.branch, None);
    // The walk reaches the detached HEAD, though no ref does.
    let head = detached.head.expect("detached head in the snapshot");
    assert_eq!(repo.commit(head).subject, "detached-only");
    assert!(gone.missing && !topic.missing && !main.missing);
}

#[test]
fn a_locked_worktree_whose_folder_is_gone_is_missing() {
    let (r, others) = with_worktrees();
    let path = others
        .path()
        .join("wt-locked")
        .to_string_lossy()
        .into_owned();
    r.git(&["worktree", "add", "-q", "--detach", &path]);
    r.git(&["worktree", "lock", &path]);
    std::fs::remove_dir_all(&path).expect("delete a worktree");
    let repo = load_repo(r.path()).expect("load");
    let locked = repo.worktrees.iter().find(|w| w.name() == "wt-locked");
    let locked = locked.expect("listed");
    // git doesn't call a locked worktree prunable, gone or not.
    assert!(locked.locked && locked.missing);
}

#[test]
fn shown_worktrees_start_history_and_escape_the_hide_list() {
    let (r, _others) = with_worktrees();
    let repo = load_repo(r.path()).expect("load");
    let hiding = GraphOptions {
        hide_branches: "topic, gone".into(),
        show_pull_requests: false,
        ..GraphOptions::default()
    };
    let plain = |s: &str| (s.to_owned(), Vec::<String>::new());
    // Off: the hidden branches and the detached commit are left out, as before.
    assert_eq!(
        subjects_with_worktrees(&repo, &hiding),
        vec![plain("A"), plain("B")]
    );
    let g = revgraph::build(&repo, &hiding);
    let labels: Vec<&str> = g
        .nodes
        .iter()
        .flat_map(|n| n.refs.iter().map(|&i| repo.refs[i].name.as_str()))
        .collect();
    assert_eq!(labels, ["main"]);

    let shown = GraphOptions {
        show_worktrees: true,
        show_local_branches: false,
        ..hiding.clone()
    };
    assert_eq!(
        subjects_with_worktrees(&repo, &shown),
        vec![
            plain("A"),
            plain("B"),
            ("detached-only".to_owned(), vec!["wt-detached".to_owned()]),
            plain("gone-only"),
            plain("topic-only"),
        ]
    );
    let g = revgraph::build(&repo, &shown);
    let mut labels: Vec<&str> = g
        .nodes
        .iter()
        .flat_map(|n| n.refs.iter().map(|&i| repo.refs[i].name.as_str()))
        .collect();
    labels.sort();
    // The current branch, and the branches other worktrees have checked out, gone or not.
    assert_eq!(labels, ["gone", "main", "topic"]);
    assert!(
        g.nodes
            .iter()
            .flat_map(|n| &n.refs)
            .all(|&i| repo.refs[i].kind == RefKind::LocalBranch)
    );

    // "Current branch only" still means only HEAD's history.
    let current = GraphOptions {
        current_branch_only: true,
        ..shown
    };
    let subjects: Vec<String> = subjects_with_worktrees(&repo, &current)
        .into_iter()
        .map(|(s, _)| s)
        .collect();
    assert_eq!(subjects, ["A", "B"]);
}

#[test]
fn worktree_changes_count_as_changed_refs() {
    let (r, others) = with_worktrees();
    let before = load_repo(r.path()).expect("load");
    assert!(before.same_refs(&load_repo(r.path()).expect("load")));
    let topic = others
        .path()
        .join("wt-topic")
        .to_string_lossy()
        .into_owned();
    r.git(&["worktree", "lock", &topic]);
    let locked = load_repo(r.path()).expect("load");
    assert!(!before.same_refs(&locked), "locked");
    git_in(
        &others.path().join("wt-detached"),
        &["checkout", "-q", "--detach", "main"],
    );
    assert!(
        !locked.same_refs(&load_repo(r.path()).expect("load")),
        "checked out"
    );
}
