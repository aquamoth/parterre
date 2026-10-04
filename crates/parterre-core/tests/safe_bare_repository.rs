//! Bare repositories when git only works with ones it is told about: `safe.bareRepository`
//! set to `explicit`, which Git 3.0 makes the default (#228).
mod common;

use std::process::Command;

use common::TestRepo;
use parterre_core::Oid;
use parterre_core::branches::{Action, Branches, Catalog, Create, Outcome};
use parterre_core::git::{Git, load_repo};
use parterre_util::CancelTree;

/// Set for git through the environment, in command-line scope: git ignores the setting in a
/// repository's own config. The test sets it by running itself again, since changing its own
/// environment takes `unsafe`.
const EXPLICIT: [(&str, &str); 3] = [
    ("GIT_CONFIG_COUNT", "1"),
    ("GIT_CONFIG_KEY_0", "safe.bareRepository"),
    ("GIT_CONFIG_VALUE_0", "explicit"),
];

/// Whether this is the run with [`EXPLICIT`]; if not, runs `test` again with it and checks
/// that it passed.
fn explicit(test: &str) -> bool {
    if EXPLICIT
        .iter()
        .all(|(k, v)| std::env::var_os(k).is_some_and(|value| value == *v))
    {
        return true;
    }
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .envs(EXPLICIT)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    // A name that matches no test would run none, and pass.
    assert!(
        out.status.success() && stdout.contains("1 passed"),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    false
}

#[test]
fn a_bare_repository_opens_and_changes_when_git_only_takes_explicit_ones() {
    if !explicit("a_bare_repository_opens_and_changes_when_git_only_takes_explicit_ones") {
        return;
    }
    let mut r = TestRepo::new();
    let tip = Oid::from_hex(&r.commit("only commit")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let bare = dir.path().join("bare.git");
    r.git(&["clone", "-q", "--bare", ".", bare.to_str().unwrap()]);

    let root = Git::new(&bare).repo_root().unwrap();
    assert_eq!(
        std::fs::canonicalize(root).unwrap(),
        std::fs::canonicalize(&bare).unwrap()
    );
    let repo = load_repo(&bare).unwrap();
    assert_eq!(repo.commits.len(), 1);
    let catalog = Catalog::load(&bare).unwrap();
    assert!(!catalog.has_working_tree);
    assert_eq!(catalog.current.as_deref(), Some("main"));

    let created = Branches::new(&bare).execute(
        Action::Create(Create {
            start: tip,
            name: "topic".into(),
            track: None,
            switch: false,
        }),
        None,
        &CancelTree::default(),
    );
    assert!(matches!(created, Outcome::Done(_)), "{created:?}");
    assert_eq!(Catalog::load(&bare).unwrap().locals.len(), 2);

    // A linked worktree of it, which git finds through the worktree's `.git` file.
    let wt = dir.path().join("wt");
    r.git(&[
        "--git-dir",
        bare.to_str().unwrap(),
        "worktree",
        "add",
        "-q",
        wt.to_str().unwrap(),
        "topic",
    ]);
    let catalog = Catalog::load(&wt).unwrap();
    assert!(catalog.has_working_tree);
    assert_eq!(catalog.current.as_deref(), Some("topic"));
}
