//! The worktree tool's operations (adding, deleting, and switching to a detached HEAD) against
//! real, disposable repositories. Deleting a worktree can lose work that git's own check
//! misses, so its cases are covered one by one.
mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::TestRepo;
use parterre_core::Oid;
use parterre_core::branches::{Action, AddWorktree, Branches, Catalog, Checkout, Outcome, Warning};
use parterre_util::CancelTree;

fn oid(s: &str) -> Oid {
    Oid::from_hex(s).unwrap()
}

fn execute(r: &TestRepo, a: Action, approval: Option<&Warning>) -> Outcome {
    Branches::new(r.path()).execute(a, approval, &CancelTree::default())
}

fn done(out: Outcome) {
    assert!(matches!(out, Outcome::Done(_)), "{out:?}");
}

fn warning(out: Outcome) -> Warning {
    match out {
        Outcome::Warning(w) => w,
        other => panic!("expected a warning: {other:?}"),
    }
}

fn failed(out: Outcome) -> String {
    match out {
        Outcome::Failed { error, .. } => error.to_string(),
        other => panic!("expected a failure: {other:?}"),
    }
}

/// Runs git in `dir`; returns its trimmed output.
fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(parterre_core::git::program())
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A repository with one file committed, and a folder outside it for worktrees.
fn repository() -> (TestRepo, tempfile::TempDir, Oid) {
    let mut r = TestRepo::new();
    r.write("file", b"base\n");
    let base = oid(&r.commit_all("base"));
    (r, tempfile::tempdir().unwrap(), base)
}

/// Adds a worktree `name` in `others` with git itself: `extra` holds options (`-b topic`,
/// `--detach`) and, last, an optional commit to check out.
fn worktree(r: &TestRepo, others: &Path, name: &str, extra: &[&str]) -> PathBuf {
    let path = others.join(name);
    let p = path.to_string_lossy().into_owned();
    let commit = extra
        .last()
        .filter(|a| !a.starts_with('-') && extra.len() >= 2 && extra[extra.len() - 2] != "-b")
        .or_else(|| {
            extra
                .first()
                .filter(|a| extra.len() == 1 && !a.starts_with('-'))
        });
    let mut args = vec!["worktree", "add", "-q"];
    args.extend(extra.iter().filter(|a| Some(*a) != commit));
    args.push(&p);
    args.extend(commit);
    r.git(&args);
    path
}

/// The worktree's path as the catalogue lists it, as the menus pass it on.
fn listed(r: &TestRepo, name: &str) -> PathBuf {
    Catalog::load(r.path())
        .unwrap()
        .worktrees
        .into_iter()
        .find(|w| w.name() == name)
        .unwrap_or_else(|| panic!("no worktree {name}"))
        .path
}

fn deletion(r: &TestRepo, name: &str) -> Action {
    deletions(r, &[name])
}

fn deletions(r: &TestRepo, names: &[&str]) -> Action {
    Action::DeleteWorktrees {
        paths: names.iter().map(|n| listed(r, n)).collect(),
        branches: false,
    }
}

/// The deletion, with the worktrees' branches too.
fn with_branches(action: Action) -> Action {
    match action {
        Action::DeleteWorktrees { paths, .. } => Action::DeleteWorktrees {
            paths,
            branches: true,
        },
        other => panic!("not a worktree deletion: {other:?}"),
    }
}

fn branch_exists(r: &TestRepo, name: &str) -> bool {
    !r.git(&["branch", "--list", name]).is_empty()
}

fn registered(r: &TestRepo, name: &str) -> bool {
    Catalog::load(r.path())
        .unwrap()
        .worktrees
        .iter()
        .any(|w| w.name() == name)
}

fn add(start: Oid, path: PathBuf, checkout: Checkout) -> Action {
    Action::AddWorktree(AddWorktree {
        start,
        path,
        checkout,
    })
}

// ---------------------------------------------------------------------------------------------
// The catalogue.

#[test]
fn the_catalogue_lists_every_worktree_with_its_state() {
    let (r, others, _) = repository();
    let locked = worktree(&r, others.path(), "locked", &["-b", "l"]);
    r.git(&[
        "worktree",
        "lock",
        "--reason",
        "on a USB stick",
        &locked.to_string_lossy(),
    ]);
    let busy = worktree(&r, others.path(), "busy", &["-b", "b"]);
    std::fs::write(busy.join("file"), "busy\n").unwrap();
    git_in(&busy, &["commit", "-qam", "busy"]);
    git_in(&busy, &["bisect", "start"]);
    let gone = worktree(&r, others.path(), "gone", &["--detach"]);
    std::fs::remove_dir_all(&gone).unwrap();
    let catalog = Catalog::load(r.path()).unwrap();
    let by = |name: &str| catalog.worktrees.iter().find(|w| w.name() == name).unwrap();
    let main = &catalog.worktrees[0];
    assert!(main.main && main.open);
    assert_eq!(main.branch.as_deref(), Some("main"));
    assert_eq!(by("locked").locked.as_deref(), Some("on a USB stick"));
    assert!(!by("locked").main && !by("locked").open);
    assert_eq!(by("busy").in_progress, Some("a bisect"));
    assert!(by("gone").missing);
    assert_eq!(by("gone").branch, None);
}

#[test]
fn a_rebase_stopped_on_a_conflict_is_an_operation_in_progress() {
    let (mut r, others, _) = repository();
    r.git(&["branch", "side"]);
    r.write("file", b"main\n");
    r.commit_all("main change");
    let wt = worktree(&r, others.path(), "rebasing", &["side"]);
    std::fs::write(wt.join("file"), "side\n").unwrap();
    git_in(&wt, &["commit", "-qam", "side change"]);
    let out = Command::new(parterre_core::git::program())
        .current_dir(&wt)
        .args(["rebase", "main"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "the rebase should stop on its conflict"
    );
    let catalog = Catalog::load(r.path()).unwrap();
    let w = catalog
        .worktrees
        .iter()
        .find(|w| w.name() == "rebasing")
        .unwrap();
    assert_eq!(w.in_progress, Some("a rebase"));
    // The branch being rebased still counts as checked out there.
    assert!(catalog.occupied.contains_key("side"));
    let rebasing = w.rebasing.clone().expect("the stopped rebase");
    assert_eq!(rebasing.branch.as_deref(), Some("side"));
    assert_eq!(rebasing.onto, Some(oid(&r.git(&["rev-parse", "main"]))));
    assert_eq!((rebasing.done, rebasing.total), (1, 1));
}

#[test]
fn deleting_a_worktree_with_a_rebase_in_progress_warns_then_ends_it() {
    let (mut r, others, _) = repository();
    r.git(&["branch", "side"]);
    r.write("file", b"main\n");
    r.commit_all("main change");
    let wt = worktree(&r, others.path(), "rebasing", &["side"]);
    std::fs::write(wt.join("file"), "side\n").unwrap();
    git_in(&wt, &["commit", "-qam", "side change"]);
    let side = git_in(&wt, &["rev-parse", "HEAD"]);
    let out = Command::new(parterre_core::git::program())
        .current_dir(&wt)
        .args(["rebase", "main"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(!out.status.success());
    // The conflicted file is lost work, so it asks first.
    let w = warning(execute(&r, deletion(&r, "rebasing"), None));
    assert!(
        w.deletions[0].files.iter().any(|f| f == "file"),
        "{:?}",
        w.deletions[0].files
    );
    assert!(wt.join("file").exists());
    done(execute(&r, deletion(&r, "rebasing"), Some(&w)));
    assert!(!wt.exists());
    // The rebase is gone with it, and the branch is where it was, free again.
    assert_eq!(r.git(&["rev-parse", "side"]), side);
    let catalog = Catalog::load(r.path()).unwrap();
    assert!(!catalog.occupied.contains_key("side"));
    assert!(catalog.worktrees.iter().all(|w| w.in_progress.is_none()));
}

// ---------------------------------------------------------------------------------------------
// Adding a worktree.

#[test]
fn adding_a_worktree_with_a_new_branch_starts_it_at_the_commit() {
    let (mut r, others, base) = repository();
    r.commit("later");
    let path = others.path().join("topic");
    let action = add(
        base,
        path.clone(),
        Checkout::New {
            name: "topic".into(),
            track: None,
        },
    );
    done(execute(&r, action, None));
    assert_eq!(git_in(&path, &["branch", "--show-current"]), "topic");
    assert_eq!(git_in(&path, &["rev-parse", "HEAD"]), base.to_hex());
    assert_eq!(common::read_text(&path.join("file")), "base\n");
    // No upstream unless one was asked for.
    assert!(!r.git(&["config", "--list"]).contains("branch.topic."));
    // The open worktree stays on its branch.
    assert_eq!(r.git(&["branch", "--show-current"]), "main");
}

#[test]
fn a_new_branch_can_track_a_fetched_or_future_upstream() {
    let (r, others, base) = repository();
    r.git(&["remote", "add", "origin", "https://example.invalid/r"]);
    r.git(&["update-ref", "refs/remotes/origin/fetched", &base.to_hex()]);
    let fetched = others.path().join("fetched");
    done(execute(
        &r,
        add(
            base,
            fetched.clone(),
            Checkout::New {
                name: "fetched".into(),
                track: Some("origin/fetched".into()),
            },
        ),
        None,
    ));
    assert_eq!(
        git_in(&fetched, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
        "origin/fetched"
    );
    done(execute(
        &r,
        add(
            base,
            others.path().join("future"),
            Checkout::New {
                name: "future".into(),
                track: Some("origin/not-yet".into()),
            },
        ),
        None,
    ));
    assert_eq!(r.git(&["config", "branch.future.remote"]), "origin");
    assert_eq!(
        r.git(&["config", "branch.future.merge"]),
        "refs/heads/not-yet"
    );
}

#[test]
fn adding_a_worktree_for_an_existing_free_branch_checks_it_out_there() {
    let (r, others, base) = repository();
    r.git(&["branch", "free"]);
    let path = others.path().join("free");
    done(execute(
        &r,
        add(base, path.clone(), Checkout::Existing("free".into())),
        None,
    ));
    assert_eq!(git_in(&path, &["branch", "--show-current"]), "free");
}

#[test]
fn an_existing_branch_that_moved_or_is_checked_out_is_refused_before_git_runs() {
    let (mut r, others, base) = repository();
    r.git(&["branch", "moved"]);
    worktree(&r, others.path(), "holder", &["-b", "held"]);
    r.checkout("moved");
    r.commit("moved on");
    r.checkout("main");
    for (branch, path) in [("moved", "a"), ("held", "b"), ("main", "c")] {
        let path = others.path().join(path);
        let out = execute(
            &r,
            add(base, path.clone(), Checkout::Existing(branch.into())),
            None,
        );
        match out {
            Outcome::Failed { report, .. } => assert!(report.steps.is_empty(), "{branch}"),
            other => panic!("{branch}: {other:?}"),
        }
        assert!(!path.exists(), "{branch}");
    }
}

#[test]
fn adding_a_detached_worktree_checks_out_the_commit() {
    let (mut r, others, base) = repository();
    r.commit("later");
    let path = others.path().join("inspect");
    done(execute(
        &r,
        add(base, path.clone(), Checkout::Detached),
        None,
    ));
    assert_eq!(git_in(&path, &["rev-parse", "HEAD"]), base.to_hex());
    assert!(
        Command::new(parterre_core::git::program())
            .current_dir(&path)
            .args(["symbolic-ref", "-q", "HEAD"])
            .status()
            .map(|s| !s.success())
            .unwrap()
    );
}

#[test]
fn a_folder_with_something_in_it_is_never_used() {
    let (r, others, base) = repository();
    let path = others.path().join("taken");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("keep.txt"), "mine").unwrap();
    let error = failed(execute(
        &r,
        add(
            base,
            path.clone(),
            Checkout::New {
                name: "x".into(),
                track: None,
            },
        ),
        None,
    ));
    assert!(error.contains("already exists"), "{error}");
    assert_eq!(std::fs::read(path.join("keep.txt")).unwrap(), b"mine");
    assert!(r.git(&["branch", "--list", "x"]).is_empty());
}

#[test]
fn an_empty_folder_is_used() {
    let (r, others, base) = repository();
    let path = others.path().join("empty");
    std::fs::create_dir(&path).unwrap();
    done(execute(
        &r,
        add(base, path.clone(), Checkout::Detached),
        None,
    ));
    assert!(path.join("file").exists());
}

#[test]
fn an_invalid_or_taken_branch_name_is_refused() {
    let (r, others, base) = repository();
    for name in ["bad..name", "main"] {
        let path = others.path().join("x");
        let out = execute(
            &r,
            add(
                base,
                path.clone(),
                Checkout::New {
                    name: name.into(),
                    track: None,
                },
            ),
            None,
        );
        assert!(matches!(out, Outcome::Failed { .. }), "{name}: {out:?}");
        assert!(!path.exists());
    }
}

#[test]
fn the_commands_shown_are_the_ones_run() {
    let (r, others, base) = repository();
    let catalog = Catalog::load(r.path()).unwrap();
    let path = others.path().join("w");
    let p = path.to_string_lossy().into_owned();
    let commands =
        |checkout| Branches::commands(&catalog, &add(base, path.clone(), checkout)).unwrap();
    assert_eq!(
        commands(Checkout::Detached),
        vec![vec![
            "worktree",
            "add",
            "--detach",
            "--",
            &p,
            &base.to_hex()
        ]]
    );
    assert_eq!(
        commands(Checkout::Existing("main".into())),
        vec![vec!["worktree", "add", "--", &p, "main"]]
    );
    assert_eq!(
        commands(Checkout::New {
            name: "n".into(),
            track: None
        }),
        vec![vec![
            "worktree",
            "add",
            "--no-track",
            "-b",
            "n",
            "--",
            &p,
            &base.to_hex()
        ]]
    );
}

// ---------------------------------------------------------------------------------------------
// Deleting a worktree.

#[test]
fn a_worktree_that_loses_nothing_is_confirmed_then_deleted_keeping_its_branch() {
    let (r, others, _) = repository();
    let path = worktree(&r, others.path(), "clean", &["-b", "clean"]);
    let action = deletion(&r, "clean");
    let confirm = warning(execute(&r, action.clone(), None));
    assert!(confirm.is_confirmation());
    assert!(path.exists(), "asking deletes nothing");
    assert_eq!(confirm.commands[0][..2], ["worktree", "remove"]);
    assert!(!confirm.commands[0].contains(&"--force".to_owned()));
    done(execute(&r, action, Some(&confirm)));
    assert!(!path.exists());
    assert!(!registered(&r, "clean"));
    assert_eq!(r.git(&["branch", "--list", "clean"]), "clean");
}

#[test]
fn changed_staged_and_untracked_files_are_listed_and_only_then_forced() {
    let (r, others, _) = repository();
    let path = worktree(&r, others.path(), "dirty", &["-b", "dirty"]);
    std::fs::write(path.join("file"), "changed\n").unwrap();
    std::fs::write(path.join("staged"), "staged\n").unwrap();
    git_in(&path, &["add", "staged"]);
    std::fs::create_dir(path.join("dir")).unwrap();
    std::fs::write(path.join("dir/loose"), "untracked\n").unwrap();
    let action = deletion(&r, "dirty");
    let w = warning(execute(&r, action.clone(), None));
    assert_eq!(w.deletions[0].files, ["dir/loose", "file", "staged"]);
    assert!(w.commits.is_empty());
    assert!(!w.is_confirmation());
    assert!(w.commands[0].contains(&"--force".to_owned()));
    assert!(path.join("file").exists());
    done(execute(&r, action, Some(&w)));
    assert!(!path.exists());
}

#[test]
fn ignored_files_are_never_lost_work() {
    let (mut r, others, _) = repository();
    r.write(".gitignore", b"build/\n.env\n");
    r.commit_all("ignore");
    let path = worktree(&r, others.path(), "ignoring", &["-b", "ignoring"]);
    std::fs::create_dir(path.join("build")).unwrap();
    std::fs::write(path.join("build/out.o"), "x").unwrap();
    std::fs::write(path.join(".env"), "SECRET=1").unwrap();
    let action = deletion(&r, "ignoring");
    let confirm = warning(execute(&r, action.clone(), None));
    assert!(
        confirm.is_confirmation(),
        "{:?}",
        confirm.deletions[0].files
    );
    done(execute(&r, action, Some(&confirm)));
    assert!(!path.exists());
}

#[test]
fn a_detached_head_with_commits_nothing_else_reaches_is_a_warning() {
    let (r, others, _) = repository();
    let path = worktree(&r, others.path(), "detached", &["--detach"]);
    git_in(&path, &["commit", "-q", "--allow-empty", "-m", "only here"]);
    let only = oid(&git_in(&path, &["rev-parse", "HEAD"]));
    let action = deletion(&r, "detached");
    let w = warning(execute(&r, action.clone(), None));
    assert_eq!(w.commits, [only]);
    assert!(w.deletions[0].files.is_empty());
    // Git itself would delete a clean detached worktree without a word: nothing to force.
    assert!(!w.commands[0].contains(&"--force".to_owned()));
    done(execute(&r, action, Some(&w)));
    assert!(!path.exists());
}

#[test]
fn a_detached_head_that_a_branch_or_tag_reaches_loses_nothing() {
    let (mut r, others, _) = repository();
    let path = worktree(&r, others.path(), "detached", &["--detach"]);
    git_in(&path, &["commit", "-q", "--allow-empty", "-m", "tagged"]);
    git_in(&path, &["tag", "kept"]);
    assert!(warning(execute(&r, deletion(&r, "detached"), None)).is_confirmation());
    r.commit("main moves on");
    let behind = worktree(&r, others.path(), "behind", &["--detach", "HEAD~1"]);
    assert!(behind.exists());
    assert!(warning(execute(&r, deletion(&r, "behind"), None)).is_confirmation());
}

#[test]
fn another_worktree_at_the_same_detached_commit_keeps_it() {
    let (r, others, _) = repository();
    let path = worktree(&r, others.path(), "one", &["--detach"]);
    git_in(&path, &["commit", "-q", "--allow-empty", "-m", "shared"]);
    let head = git_in(&path, &["rev-parse", "HEAD"]);
    worktree(&r, others.path(), "two", &["--detach", &head]);
    assert!(warning(execute(&r, deletion(&r, "one"), None)).is_confirmation());
}

#[test]
fn an_approval_is_void_once_more_would_be_lost() {
    let (r, others, _) = repository();
    let path = worktree(&r, others.path(), "w", &["-b", "w"]);
    let action = deletion(&r, "w");
    let confirm = warning(execute(&r, action.clone(), None));
    assert!(confirm.is_confirmation());
    std::fs::write(path.join("new"), "written after the confirmation").unwrap();
    let w = warning(execute(&r, action.clone(), Some(&confirm)));
    assert_eq!(w.deletions[0].files, ["new"]);
    assert!(path.join("new").exists());
    std::fs::write(path.join("newer"), "and more").unwrap();
    let again = warning(execute(&r, action.clone(), Some(&w)));
    assert_eq!(again.deletions[0].files, ["new", "newer"]);
    assert!(path.join("newer").exists());
    done(execute(&r, action, Some(&again)));
    assert!(!path.exists());
}

#[test]
fn an_approval_is_void_once_the_detached_head_moves() {
    let (r, others, _) = repository();
    let path = worktree(&r, others.path(), "d", &["--detach"]);
    git_in(&path, &["commit", "-q", "--allow-empty", "-m", "first"]);
    let action = deletion(&r, "d");
    let w = warning(execute(&r, action.clone(), None));
    git_in(&path, &["commit", "-q", "--allow-empty", "-m", "second"]);
    let again = warning(execute(&r, action, Some(&w)));
    assert_eq!(again.commits.len(), 2);
    assert!(path.exists());
}

#[test]
fn the_main_open_and_locked_worktrees_are_never_deleted() {
    let (r, others, _) = repository();
    let linked = worktree(&r, others.path(), "linked", &["-b", "linked"]);
    // From the linked worktree, the main one isn't the open one.
    let main = Catalog::load(&linked).unwrap().main;
    let error = failed(Branches::new(&linked).execute(
        Action::DeleteWorktrees {
            paths: vec![main],
            branches: false,
        },
        None,
        &CancelTree::default(),
    ));
    assert!(error.contains("main worktree"), "{error}");
    let open = Catalog::load(&linked).unwrap().root;
    let error = failed(Branches::new(&linked).execute(
        Action::DeleteWorktrees {
            paths: vec![open],
            branches: false,
        },
        None,
        &CancelTree::default(),
    ));
    assert!(error.contains("open worktree"), "{error}");
    r.git(&["worktree", "lock", &linked.to_string_lossy()]);
    let error = failed(execute(&r, deletion(&r, "linked"), None));
    assert!(error.contains("locked"), "{error}");
    assert!(linked.exists());
}

#[test]
fn a_worktree_whose_folder_is_gone_is_confirmed_and_forgotten() {
    let (r, others, _) = repository();
    let path = worktree(&r, others.path(), "gone", &["-b", "gone"]);
    std::fs::remove_dir_all(&path).unwrap();
    let action = deletion(&r, "gone");
    let confirm = warning(execute(&r, action.clone(), None));
    assert!(confirm.is_confirmation());
    done(execute(&r, action, Some(&confirm)));
    assert!(!registered(&r, "gone"));
    assert_eq!(r.git(&["branch", "--list", "gone"]), "gone");
}

#[test]
fn a_gone_detached_worktree_still_warns_about_its_commits() {
    let (r, others, _) = repository();
    let path = worktree(&r, others.path(), "gone", &["--detach"]);
    git_in(&path, &["commit", "-q", "--allow-empty", "-m", "only here"]);
    std::fs::remove_dir_all(&path).unwrap();
    let w = warning(execute(&r, deletion(&r, "gone"), None));
    assert_eq!(w.commits.len(), 1);
    assert!(w.deletions[0].files.is_empty());
}

#[test]
fn a_worktree_that_moved_away_is_not_deleted_by_a_stale_request() {
    let (r, others, _) = repository();
    let path = worktree(&r, others.path(), "w", &["-b", "w"]);
    let action = deletion(&r, "w");
    let confirm = warning(execute(&r, action.clone(), None));
    let moved = others.path().join("moved");
    r.git(&[
        "worktree",
        "move",
        &path.to_string_lossy(),
        &moved.to_string_lossy(),
    ]);
    let error = failed(execute(&r, action, Some(&confirm)));
    assert!(error.contains("no longer there"), "{error}");
    assert!(moved.join("file").exists());
}

#[test]
fn when_git_refuses_anyway_parterre_asks_again_before_forcing() {
    let (r, others, _) = repository();
    let path = worktree(&r, others.path(), "w", &["-b", "w"]);
    // A submodule's git dir makes git refuse `worktree remove` without --force, though nothing
    // parterre counts as lost work is there.
    let sub = TestRepo::new();
    sub.git(&["commit", "-q", "--allow-empty", "-m", "sub"]);
    git_in(
        &path,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            &sub.path().to_string_lossy(),
            "sub",
        ],
    );
    git_in(&path, &["commit", "-qm", "add a submodule"]);
    let action = deletion(&r, "w");
    let fresh = warning(execute(&r, action.clone(), None));
    assert!(fresh.is_confirmation(), "{:?}", fresh.deletions[0].files);
    match execute(&r, action.clone(), Some(&fresh)) {
        Outcome::Warning(again) => {
            assert!(again.deletions[0].refusal.is_some());
            assert!(!again.is_confirmation());
            assert!(again.commands[0].contains(&"--force".to_owned()));
            assert!(path.exists());
            done(execute(&r, action, Some(&again)));
            assert!(!path.exists());
        }
        // Git versions that remove such worktrees without --force.
        Outcome::Done(_) => assert!(!path.exists()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn several_worktrees_are_asked_about_once_each_with_its_own_loss() {
    let (r, others, _) = repository();
    let clean = worktree(&r, others.path(), "clean", &["-b", "clean"]);
    let dirty = worktree(&r, others.path(), "dirty", &["-b", "dirty"]);
    std::fs::write(dirty.join("file"), "changed\n").unwrap();
    let detached = worktree(&r, others.path(), "detached", &["--detach"]);
    git_in(
        &detached,
        &["commit", "-q", "--allow-empty", "-m", "only here"],
    );
    let only = oid(&git_in(&detached, &["rev-parse", "HEAD"]));
    let action = deletions(&r, &["clean", "dirty", "detached"]);
    let w = warning(execute(&r, action.clone(), None));
    assert!(!w.is_confirmation());
    let names: Vec<_> = w.deletions.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["clean", "dirty", "detached"]);
    assert!(w.deletions[0].files.is_empty() && w.deletions[0].commits.is_empty());
    assert_eq!(w.deletions[1].files, ["file"]);
    assert_eq!(w.deletions[2].commits, [only]);
    assert_eq!(w.commits, [only]);
    let forced: Vec<bool> = w
        .commands
        .iter()
        .map(|c| c.contains(&"--force".to_owned()))
        .collect();
    assert_eq!(forced, [false, true, false]);
    assert!(clean.exists() && dirty.exists() && detached.exists());
    done(execute(&r, action, Some(&w)));
    assert!(!clean.exists() && !dirty.exists() && !detached.exists());
    assert_eq!(
        r.git(&["branch", "--list", "clean", "dirty"]),
        "clean\n  dirty"
    );
}

#[test]
fn detached_worktrees_deleted_together_lose_the_commits_they_share() {
    let (r, others, _) = repository();
    let path = worktree(&r, others.path(), "one", &["--detach"]);
    git_in(&path, &["commit", "-q", "--allow-empty", "-m", "shared"]);
    let head = git_in(&path, &["rev-parse", "HEAD"]);
    worktree(&r, others.path(), "two", &["--detach", &head]);
    let w = warning(execute(&r, deletions(&r, &["one", "two"]), None));
    assert_eq!(w.commits, [oid(&head)]);
    assert_eq!(w.deletions[0].commits, [oid(&head)]);
    assert_eq!(w.deletions[1].commits, [oid(&head)]);
}

#[test]
fn one_worktree_that_cannot_go_stops_them_all_before_anything_is_deleted() {
    let (r, others, _) = repository();
    let free = worktree(&r, others.path(), "free", &["-b", "free"]);
    let locked = worktree(&r, others.path(), "locked", &["-b", "locked"]);
    r.git(&["worktree", "lock", &locked.to_string_lossy()]);
    let error = failed(execute(&r, deletions(&r, &["free", "locked"]), None));
    assert!(error.contains("locked"), "{error}");
    let error = failed(execute(&r, deletions(&r, &["free", "free"]), None));
    assert!(error.contains("twice"), "{error}");
    assert!(free.exists() && locked.exists());
}

#[test]
fn the_worktrees_git_refuses_are_asked_about_again_and_the_rest_deleted() {
    let (r, others, _) = repository();
    let plain = worktree(&r, others.path(), "plain", &["-b", "plain"]);
    let path = worktree(&r, others.path(), "w", &["-b", "w"]);
    let sub = TestRepo::new();
    sub.git(&["commit", "-q", "--allow-empty", "-m", "sub"]);
    git_in(
        &path,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            &sub.path().to_string_lossy(),
            "sub",
        ],
    );
    git_in(&path, &["commit", "-qm", "add a submodule"]);
    let action = deletions(&r, &["w", "plain"]);
    let fresh = warning(execute(&r, action.clone(), None));
    assert!(fresh.is_confirmation());
    match execute(&r, action, Some(&fresh)) {
        Outcome::Warning(again) => {
            assert!(!plain.exists(), "the one git agreed to went");
            assert_eq!(again.action, deletion(&r, "w"));
            assert_eq!(again.deletions.len(), 1);
            assert!(again.deletions[0].refusal.is_some());
            done(execute(&r, again.action.clone(), Some(&again)));
            assert!(!path.exists());
        }
        // Git versions that remove such worktrees without --force.
        Outcome::Done(_) => assert!(!path.exists() && !plain.exists()),
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// Deleting a worktree's branch with it.

#[test]
fn a_branch_that_loses_nothing_goes_with_its_worktree_after_the_same_confirmation() {
    let (r, others, _) = repository();
    let path = worktree(&r, others.path(), "clean", &["-b", "clean"]);
    let mut confirm = warning(execute(&r, deletion(&r, "clean"), None));
    let branch = confirm.deletions[0].branch.clone().expect("its branch");
    assert_eq!((branch.name.as_str(), branch.commits.len()), ("clean", 0));
    assert!(!confirm.deletes_branches());
    confirm.set_deletes_branches(true);
    assert!(confirm.is_confirmation());
    assert_eq!(confirm.action, with_branches(deletion(&r, "clean")));
    assert_eq!(confirm.commands.len(), 2);
    assert_eq!(confirm.commands[1], ["branch", "-d", "--", "clean"]);
    done(execute(&r, confirm.action.clone(), Some(&confirm)));
    assert!(!path.exists());
    assert!(!branch_exists(&r, "clean"));
}

#[test]
fn a_branch_with_commits_only_it_has_turns_the_confirmation_into_a_warning() {
    let (r, others, _) = repository();
    let path = worktree(&r, others.path(), "w", &["-b", "topic"]);
    git_in(
        &path,
        &["commit", "-q", "--allow-empty", "-m", "only on topic"],
    );
    let only = oid(&git_in(&path, &["rev-parse", "HEAD"]));
    let mut w = warning(execute(&r, deletion(&r, "w"), None));
    // Kept, the branch keeps its commit.
    assert!(w.is_confirmation() && w.commits.is_empty());
    assert_eq!(w.deletions[0].branch.as_ref().unwrap().commits, [only]);
    w.set_deletes_branches(true);
    assert!(!w.is_confirmation());
    assert_eq!(w.commits, [only]);
    assert_eq!(w.commands[1], ["branch", "-D", "--", "topic"]);
    // Unticked again, it's the confirmation it was.
    w.set_deletes_branches(false);
    assert!(w.is_confirmation() && w.commits.is_empty());
    assert_eq!(w.commands.len(), 1);
    w.set_deletes_branches(true);
    done(execute(&r, w.action.clone(), Some(&w)));
    assert!(!path.exists());
    assert!(!branch_exists(&r, "topic"));
}

#[test]
fn approving_the_worktree_alone_never_deletes_its_branch() {
    let (r, others, _) = repository();
    let path = worktree(&r, others.path(), "w", &["-b", "topic"]);
    git_in(
        &path,
        &["commit", "-q", "--allow-empty", "-m", "only on topic"],
    );
    let confirm = warning(execute(&r, deletion(&r, "w"), None));
    let asked = warning(execute(
        &r,
        with_branches(deletion(&r, "w")),
        Some(&confirm),
    ));
    assert!(!asked.is_confirmation());
    assert!(path.exists() && branch_exists(&r, "topic"));
    // And the approval with the branch is for that, not the worktree alone.
    let mut both = confirm.clone();
    both.set_deletes_branches(true);
    let again = warning(execute(&r, deletion(&r, "w"), Some(&both)));
    assert!(again.is_confirmation());
    assert!(path.exists() && branch_exists(&r, "topic"));
}

#[test]
fn a_branch_that_moved_since_the_approval_is_asked_about_again() {
    let (r, others, _) = repository();
    let path = worktree(&r, others.path(), "w", &["-b", "topic"]);
    let mut w = warning(execute(&r, deletion(&r, "w"), None));
    w.set_deletes_branches(true);
    assert!(w.is_confirmation());
    git_in(&path, &["commit", "-q", "--allow-empty", "-m", "after"]);
    let after = oid(&git_in(&path, &["rev-parse", "HEAD"]));
    let again = warning(execute(&r, w.action.clone(), Some(&w)));
    assert_eq!(again.commits, [after]);
    assert!(again.deletes_branches());
    assert!(path.exists() && branch_exists(&r, "topic"));
}

#[test]
fn a_branch_git_d_refuses_but_that_loses_nothing_goes_without_asking_again() {
    let (r, others, _) = repository();
    let path = worktree(&r, others.path(), "w", &["-b", "topic"]);
    git_in(&path, &["commit", "-q", "--allow-empty", "-m", "tagged"]);
    // A tag keeps the commit, but `branch -d` only asks HEAD and the upstream.
    git_in(&path, &["tag", "kept"]);
    let mut w = warning(execute(&r, deletion(&r, "w"), None));
    w.set_deletes_branches(true);
    assert!(w.is_confirmation(), "{:?}", w.commits);
    done(execute(&r, w.action.clone(), Some(&w)));
    assert!(!path.exists());
    assert!(!branch_exists(&r, "topic"));
    assert!(!r.git(&["tag", "--list", "kept"]).is_empty());
}

#[test]
fn branches_deleted_together_lose_the_commits_they_share() {
    let (r, others, _) = repository();
    let one = worktree(&r, others.path(), "one", &["-b", "one"]);
    git_in(&one, &["commit", "-q", "--allow-empty", "-m", "shared"]);
    let shared = oid(&git_in(&one, &["rev-parse", "HEAD"]));
    worktree(&r, others.path(), "two", &["-b", "two", &shared.to_hex()]);
    let mut w = warning(execute(&r, deletions(&r, &["one", "two"]), None));
    // Each alone is kept by the other.
    let lost = |w: &Warning, i: usize| w.deletions[i].branch.as_ref().unwrap().commits.clone();
    assert_eq!((lost(&w, 0), lost(&w, 1)), (vec![shared], vec![shared]));
    w.set_deletes_branches(true);
    assert_eq!(w.commits, [shared]);
    assert_eq!(w.commands[2], ["branch", "-D", "--", "one", "two"]);
    done(execute(&r, w.action.clone(), Some(&w)));
    assert!(!branch_exists(&r, "one") && !branch_exists(&r, "two"));
}

#[test]
fn a_detached_worktree_has_no_branch_to_delete() {
    let (r, others, _) = repository();
    let path = worktree(&r, others.path(), "d", &["--detach"]);
    let mut w = warning(execute(&r, deletion(&r, "d"), None));
    assert!(w.deletions[0].branch.is_none());
    w.set_deletes_branches(true);
    assert_eq!(w.commands.len(), 1, "no branch step");
    done(execute(&r, w.action.clone(), Some(&w)));
    assert!(!path.exists());
    assert_eq!(r.git(&["branch", "--format=%(refname:short)"]), "main");
}

#[test]
fn the_branch_a_worktree_is_rebasing_goes_with_it() {
    let (mut r, others, _) = repository();
    r.git(&["branch", "side"]);
    r.write("file", b"main\n");
    r.commit_all("main change");
    let wt = worktree(&r, others.path(), "rebasing", &["side"]);
    std::fs::write(wt.join("file"), "side\n").unwrap();
    git_in(&wt, &["commit", "-qam", "side change"]);
    let side = oid(&git_in(&wt, &["rev-parse", "HEAD"]));
    let out = Command::new(parterre_core::git::program())
        .current_dir(&wt)
        .args(["rebase", "main"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let mut w = warning(execute(&r, deletion(&r, "rebasing"), None));
    let branch = w.deletions[0]
        .branch
        .clone()
        .expect("the branch being rebased");
    assert_eq!((branch.name.as_str(), branch.tip), ("side", side));
    assert_eq!(branch.commits, [side]);
    w.set_deletes_branches(true);
    assert!(w.commits.contains(&side));
    done(execute(&r, w.action.clone(), Some(&w)));
    assert!(!wt.exists());
    assert!(!branch_exists(&r, "side"));
}

#[test]
fn a_worktree_git_refuses_keeps_its_branch_and_the_others_go() {
    let (r, others, _) = repository();
    let plain = worktree(&r, others.path(), "plain", &["-b", "plain"]);
    let path = worktree(&r, others.path(), "w", &["-b", "w"]);
    let sub = TestRepo::new();
    sub.git(&["commit", "-q", "--allow-empty", "-m", "sub"]);
    git_in(
        &path,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            &sub.path().to_string_lossy(),
            "sub",
        ],
    );
    git_in(&path, &["commit", "-qm", "add a submodule"]);
    let mut fresh = warning(execute(&r, deletions(&r, &["w", "plain"]), None));
    fresh.set_deletes_branches(true);
    match execute(&r, fresh.action.clone(), Some(&fresh)) {
        Outcome::Warning(again) => {
            assert!(!plain.exists() && !branch_exists(&r, "plain"));
            assert!(path.exists() && branch_exists(&r, "w"));
            assert_eq!(again.action, with_branches(deletion(&r, "w")));
            assert!(again.deletions[0].refusal.is_some());
            done(execute(&r, again.action.clone(), Some(&again)));
            assert!(!path.exists() && !branch_exists(&r, "w"));
        }
        // Git versions that remove such worktrees without --force.
        Outcome::Done(_) => {
            assert!(!path.exists() && !plain.exists());
            assert!(!branch_exists(&r, "w") && !branch_exists(&r, "plain"));
        }
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// Switching to a detached HEAD.

#[test]
fn switching_to_a_commit_detaches_head_there() {
    let (mut r, _others, base) = repository();
    r.commit("later");
    done(execute(&r, Action::Detach(base), None));
    assert_eq!(r.git(&["rev-parse", "HEAD"]), base.to_hex());
    assert_eq!(r.git(&["branch", "--show-current"]), "");
    // Already there: nothing runs.
    match execute(&r, Action::Detach(base), None) {
        Outcome::Done(report) => assert!(report.steps.is_empty()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn detaching_keeps_changes_git_can_carry_and_refuses_the_rest() {
    let (mut r, _others, base) = repository();
    r.write("file", b"later\n");
    r.commit_all("later");
    r.write("file", b"uncommitted\n");
    r.write("loose", b"untracked\n");
    let out = execute(&r, Action::Detach(base), None);
    assert!(matches!(out, Outcome::Failed { .. }), "{out:?}");
    assert_eq!(r.git(&["branch", "--show-current"]), "main");
    assert_eq!(
        std::fs::read(r.path().join("file")).unwrap(),
        b"uncommitted\n"
    );
    assert_eq!(
        std::fs::read(r.path().join("loose")).unwrap(),
        b"untracked\n"
    );
}

#[test]
fn detaching_away_from_a_detached_head_with_lost_commits_warns_first() {
    let (mut r, _others, base) = repository();
    r.git(&["switch", "-q", "--detach"]);
    let only = oid(&r.commit("only on the detached HEAD"));
    let w = warning(execute(&r, Action::Detach(base), None));
    assert_eq!(w.commits, [only]);
    assert_eq!(r.git(&["rev-parse", "HEAD"]), only.to_hex());
    done(execute(&r, Action::Detach(base), Some(&w)));
    assert_eq!(r.git(&["rev-parse", "HEAD"]), base.to_hex());
}

#[test]
fn detaching_forward_from_a_detached_head_loses_nothing() {
    let (mut r, _others, base) = repository();
    let later = oid(&r.commit("later"));
    r.git(&["switch", "-q", "--detach", &base.to_hex()]);
    let _ = r.git(&["branch", "-f", "main", &base.to_hex()]);
    // `later` is now reachable only from where we're going.
    done(execute(&r, Action::Detach(later), None));
    assert_eq!(r.git(&["rev-parse", "HEAD"]), later.to_hex());
}

#[test]
fn a_commit_that_is_gone_is_refused() {
    let (r, _others, _) = repository();
    let error = failed(execute(
        &r,
        Action::Detach(oid("1234567890123456789012345678901234567890")),
        None,
    ));
    assert!(error.contains("no longer in the repository"), "{error}");
}

// ---------------------------------------------------------------------------------------------
// A worktree root inside a repository.

#[test]
fn a_root_inside_a_working_tree_is_noticed_until_excluded_there() {
    use parterre_core::worktree_folder::{exclude, inside_repository};
    let (mut r, others, _) = repository();
    // Not inside anything.
    assert_eq!(inside_repository(&others.path().join("w/fix")), None);
    // Inside this repository's working tree, before the folder exists.
    let folder = r.path().join("trees").join("fix");
    let inside = inside_repository(&folder).expect("inside the repository");
    assert_eq!(inside.pattern, "/trees/");
    exclude(&inside).unwrap();
    assert_eq!(inside_repository(&folder), None);
    // Excluding twice adds the line once.
    exclude(&inside).unwrap();
    let excludes = std::fs::read_to_string(r.path().join(".git/info/exclude")).unwrap();
    assert_eq!(excludes.matches("/trees/").count(), 1);
    // Ignored by .gitignore: nothing to say.
    r.write(".gitignore", b"/ignored/\n");
    r.commit_all("ignore");
    assert_eq!(inside_repository(&r.path().join("ignored/fix")), None);
    // Inside another repository.
    let other = TestRepo::new();
    let inside = inside_repository(&other.path().join("sub/fix")).expect("inside the other");
    assert_eq!(inside.pattern, "/sub/");
}
