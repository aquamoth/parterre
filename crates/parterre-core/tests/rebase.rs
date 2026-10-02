//! Rebasing the open worktree's branch, against real, disposable repositories: when it's
//! offered, what the confirmation says git leaves out (checked against what `git rebase` then
//! does), and every way it can stop, refuse or keep changes aside.
mod common;

use common::TestRepo;
use parterre_core::Oid;
use parterre_core::branches::{Action, Branches, Cancel, Catalog, Outcome, Report, Stuck};
use parterre_core::rebase::{self, Preview, Skipped};

fn oid(s: &str) -> Oid {
    Oid::from_hex(s).unwrap()
}

fn rev(r: &TestRepo, rev: &str) -> Oid {
    oid(&r.git(&["rev-parse", rev]))
}

fn preview(r: &TestRepo, onto: &str) -> Preview {
    Preview::load(r.path(), rev(r, onto)).unwrap()
}

fn execute(r: &TestRepo, rebase: rebase::Rebase) -> Outcome {
    Branches::new(r.path()).execute(Action::Rebase(Box::new(rebase)), None, &Cancel::default())
}

fn done(out: Outcome) -> Report {
    match out {
        Outcome::Done(report) => report,
        other => panic!("expected it done: {other:?}"),
    }
}

fn failed(out: Outcome) -> String {
    match out {
        Outcome::Failed { error, .. } => error.to_string(),
        other => panic!("expected a failure: {other:?}"),
    }
}

fn offered(r: &TestRepo, onto: &str) -> Option<String> {
    let repo = r.load();
    let catalog = Catalog::load(r.path()).unwrap();
    rebase::offered(&repo, &catalog, rev(r, onto)).map(str::to_owned)
}

/// `main` with a commit of its own, and `up` with one, from a shared base: they diverged.
fn diverged() -> TestRepo {
    let mut r = TestRepo::new();
    r.write("base", b"base\n");
    r.commit_all("base");
    r.git(&["branch", "up"]);
    r.write("mine", b"mine\n");
    r.commit_all("mine");
    r.checkout("up");
    r.write("theirs", b"theirs\n");
    r.commit_all("theirs");
    r.checkout("main");
    r
}

#[test]
fn it_is_offered_only_when_it_would_really_rebase() {
    let mut r = diverged();
    assert_eq!(offered(&r, "up").as_deref(), Some("main"));
    // Already in main: nothing to do.
    assert_eq!(offered(&r, "main~1"), None);
    // The open worktree's own branch.
    assert_eq!(offered(&r, "main"), None);
    // Only behind: a fast-forward, which isn't a rebase.
    r.git(&["branch", "ahead", "main"]);
    r.checkout("ahead");
    r.write("more", b"more\n");
    r.commit_all("more");
    r.checkout("main");
    assert_eq!(offered(&r, "ahead"), None);
    // A detached HEAD has no branch to rebase.
    r.git(&["checkout", "-q", "--detach", "main"]);
    assert_eq!(offered(&r, "up"), None);
}

#[test]
fn the_preview_greys_what_git_leaves_out_and_the_rebase_agrees() {
    let mut r = TestRepo::new();
    r.write("base", b"base\n");
    r.commit_all("base");
    r.git(&["branch", "up"]);
    // On main: a fix also made on up (dropped), a feature, and a merged side branch.
    r.write("fix", b"fixed\n");
    let fix = r.commit_all("fix");
    r.write("feature", b"feature\n");
    let feature = r.commit_all("feature");
    r.branch("side");
    r.write("side", b"side\n");
    let side = r.commit_all("side");
    r.checkout("main");
    let merge = r.merge("side", "Merge side");
    r.checkout("up");
    r.write("fix", b"fixed\n");
    r.commit_all("the same fix");
    r.write("theirs", b"theirs\n");
    r.commit_all("theirs");
    r.checkout("main");

    let p = preview(&r, "up");
    assert_eq!(p.branch, "main");
    assert!(!p.dirty);
    assert_eq!(p.skipped(oid(&fix)), Some(Skipped::AlreadyThere));
    assert_eq!(p.skipped(oid(&merge)), Some(Skipped::Merge));
    assert_eq!(p.skipped(oid(&feature)), None);
    assert_eq!(p.skipped(oid(&side)), None);

    let rebase = p.rebase("up".into(), false);
    assert_eq!(rebase::command(&rebase), ["rebase", "up"]);
    let report = done(execute(&r, rebase));
    assert!(report.attention.is_none());
    // What it replayed: the two the preview didn't grey, flattened onto up.
    let replayed = r.git(&["log", "--format=%s", "up..main"]);
    assert_eq!(replayed.lines().collect::<Vec<_>>(), ["side", "feature"]);
    assert_eq!(r.git(&["rev-list", "--merges", "--count", "up..main"]), "0");
}

#[test]
fn a_conflict_stops_it_and_leaves_the_worktree_stuck() {
    let mut r = TestRepo::new();
    r.write("file", b"base\n");
    r.commit_all("base");
    r.git(&["branch", "up"]);
    r.write("clean", b"clean\n");
    r.commit_all("clean");
    r.write("file", b"mine\n");
    let mine = r.commit_all("mine");
    r.checkout("up");
    r.write("file", b"theirs\n");
    r.commit_all("theirs");
    r.checkout("main");

    let report = done(execute(&r, preview(&r, "up").rebase("up".into(), false)));
    let attention = report.attention.expect("an orange notice");
    assert_eq!(attention.title, "Rebase stopped on conflicts in 1 file");

    let catalog = Catalog::load(r.path()).unwrap();
    assert_eq!(catalog.stuck(), Some(Stuck::InProgress("a rebase")));
    assert_eq!(catalog.conflicted, ["file"]);
    let rebasing = catalog
        .worktrees
        .iter()
        .find(|w| w.open)
        .and_then(|w| w.rebasing.clone())
        .unwrap();
    assert_eq!(rebasing.branch.as_deref(), Some("main"));
    assert_eq!(rebasing.onto, Some(rev(&r, "up")));
    assert_eq!((rebasing.done, rebasing.total), (2, 2));
    // The branch hasn't moved yet: only HEAD has, onto the clean commit.
    assert_eq!(rev(&r, "main"), oid(&mine));
    assert_eq!(r.git(&["log", "-1", "--format=%s", "HEAD"]), "clean");

    // Nothing more is offered there, and a second rebase is refused.
    assert_eq!(offered(&r, "up"), None);
    let again = rebase::Rebase {
        branch: "main".into(),
        head: oid(&mine),
        onto: rev(&r, "up"),
        target: "up".into(),
        autostash: None,
    };
    let error = failed(execute(&r, again));
    assert!(error.contains("no longer checked out"), "{error}");
}

#[test]
fn uncommitted_changes_need_the_stash_or_git_refuses() {
    let r = diverged();
    r.write("mine", b"edited\n");
    // Untracked files don't count: git rebases with them.
    r.write("untracked", b"new\n");
    let p = preview(&r, "up");
    assert!(p.dirty);
    assert!(!p.auto_stash);
    assert_eq!(
        p.blocked(false),
        Some("Commit or stash your changes first.")
    );
    assert_eq!(p.blocked(true), None);

    let error = failed(execute(&r, p.rebase("up".into(), false)));
    assert!(!error.is_empty());
    let catalog = Catalog::load(r.path()).unwrap();
    assert_eq!(catalog.stuck(), None);
    assert_eq!(common::read_text(&r.path().join("mine")), "edited\n");

    let stashed = p.rebase("up".into(), true);
    assert_eq!(rebase::command(&stashed), ["rebase", "--autostash", "up"]);
    let report = done(execute(&r, stashed));
    assert!(report.attention.is_none());
    assert_eq!(rev(&r, "main~1"), rev(&r, "up"));
    assert_eq!(common::read_text(&r.path().join("mine")), "edited\n");
    assert_eq!(r.git(&["stash", "list"]), "");
}

#[test]
fn rebase_auto_stash_ticks_the_box_and_unticking_says_no_autostash() {
    let r = diverged();
    r.git(&["config", "rebase.autoStash", "true"]);
    r.write("mine", b"edited\n");
    let p = preview(&r, "up");
    assert!(p.auto_stash);
    assert_eq!(
        rebase::command(&p.rebase("up".into(), true)),
        ["rebase", "up"]
    );
    assert_eq!(
        rebase::command(&p.rebase("up".into(), false)),
        ["rebase", "--no-autostash", "up"]
    );
}

#[test]
fn changes_the_autostash_cant_put_back_stay_in_the_stash() {
    let mut r = TestRepo::new();
    r.write("file", b"base\n");
    r.commit_all("base");
    r.git(&["branch", "up"]);
    r.write("mine", b"mine\n");
    r.commit_all("mine");
    r.checkout("up");
    r.write("file", b"theirs\n");
    r.commit_all("theirs");
    r.checkout("main");
    r.write("file", b"my edit\n");

    let report = done(execute(&r, preview(&r, "up").rebase("up".into(), true)));
    let attention = report.attention.expect("an orange notice");
    assert_eq!(attention.title, "Rebased main onto up");
    assert_eq!(
        attention.message,
        "Putting your changes back conflicted in 1 file. Resolve it with git; your changes \
         are also kept in a stash entry until you drop it."
    );
    // Rebased, and the changes are kept: in the stash, and in the file between markers.
    assert_eq!(rev(&r, "main~1"), rev(&r, "up"));
    assert_eq!(r.git(&["stash", "list"]).lines().count(), 1);
    assert!(common::read_text(&r.path().join("file")).contains("my edit"));
    // No operation in progress, but the conflicted file sticks the worktree all the same.
    let catalog = Catalog::load(r.path()).unwrap();
    assert!(catalog.worktrees.iter().all(|w| w.in_progress.is_none()));
    assert_eq!(catalog.conflicted, ["file"]);
    assert_eq!(catalog.stuck(), Some(Stuck::Conflicts));
    let refused = Preview::load(r.path(), rev(&r, "up"))
        .unwrap_err()
        .to_string();
    assert_eq!(refused, "This worktree has conflicted files.");
}

#[test]
fn a_branch_that_moved_since_the_preview_is_left_alone() {
    let mut r = diverged();
    let p = preview(&r, "up");
    r.write("later", b"later\n");
    let later = r.commit_all("later");
    let error = failed(execute(&r, p.rebase("up".into(), false)));
    assert!(error.contains("moved"), "{error}");
    assert_eq!(rev(&r, "main"), oid(&later));
}

#[test]
fn onto_a_commit_names_it_by_its_full_hash() {
    let r = diverged();
    let up = rev(&r, "up");
    let p = Preview::load(r.path(), up).unwrap();
    let rebase = p.rebase(up.to_hex(), false);
    assert_eq!(rebase::command(&rebase), ["rebase".to_owned(), up.to_hex()]);
    assert_eq!(rebase::short_target(&rebase), up.to_hex()[..7]);
    done(execute(&r, rebase));
    assert_eq!(rev(&r, "main~1"), up);
}
