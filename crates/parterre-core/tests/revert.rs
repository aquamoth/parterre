//! Reverting a commit on the open worktree's branch, against real, disposable repositories:
//! when it's offered, the message (checked against what `git revert` itself writes), merges,
//! uncommitted changes with and without stashing them, restoring the stash, and every way it
//! can stop or refuse without losing work.
mod common;

use common::{TestRepo, read_text};
use parterre_core::Oid;
use parterre_core::branches::{Action, Branches, Cancel, Catalog, Outcome, Report, Stuck};
use parterre_core::revert::{self, Preview};

fn rev(r: &TestRepo, rev: &str) -> Oid {
    Oid::from_hex(&r.git(&["rev-parse", rev])).unwrap()
}

fn preview(r: &TestRepo, commit: &str) -> Preview {
    Preview::load(r.path(), rev(r, commit)).unwrap()
}

fn run(r: &TestRepo, action: Action) -> Outcome {
    Branches::new(r.path()).execute(action, None, &Cancel::default())
}

fn execute(r: &TestRepo, revert: revert::Revert) -> Outcome {
    run(r, Action::Revert(Box::new(revert)))
}

fn done(out: Outcome) -> Report {
    match out {
        Outcome::Done(report) => report,
        other => panic!("expected it done: {other:?}"),
    }
}

fn failed(out: Outcome) -> (String, Report) {
    match out {
        Outcome::Failed { error, report } => (error.to_string(), report),
        other => panic!("expected a failure: {other:?}"),
    }
}

/// The revert `p` would run with git's message.
fn plain(p: &Preview, stash: bool) -> revert::Revert {
    p.revert(stash, &p.message.clone())
}

fn offered(r: &TestRepo, commit: &str) -> Option<String> {
    let repo = r.load();
    let catalog = Catalog::load(r.path()).unwrap();
    revert::offered(&repo, &catalog, rev(r, commit))
}

fn text(r: &TestRepo, path: &str) -> String {
    read_text(&r.path().join(path))
}

fn message(r: &TestRepo) -> String {
    r.git(&["log", "-1", "--format=%B"])
}

/// main: base (file "one") → change (file "two") → later (another file).
fn history() -> TestRepo {
    let mut r = TestRepo::new();
    r.write("file", b"one\n");
    r.write("other", b"other\n");
    r.commit_all("base");
    r.write("file", b"two\n");
    r.commit_all("change file");
    r.write("later", b"later\n");
    r.commit_all("later");
    r.git(&["tag", "change", "HEAD~1"]);
    r
}

#[test]
fn offered_for_commits_head_reaches_only() {
    let r = history();
    r.git(&["branch", "side", "HEAD~2"]);
    r.checkout("side");
    r.write("side", b"side\n");
    r.git(&["add", "side"]);
    r.git(&["commit", "-q", "-m", "side"]);
    let side = r.git(&["rev-parse", "HEAD"]);
    r.checkout("main");
    assert_eq!(offered(&r, "HEAD").as_deref(), Some("main"));
    assert_eq!(offered(&r, "change").as_deref(), Some("main"));
    assert_eq!(offered(&r, "HEAD~2").as_deref(), Some("main"));
    assert_eq!(offered(&r, &side), None, "not on main");
    r.git(&["checkout", "-q", "--detach", "HEAD"]);
    assert_eq!(offered(&r, "change").as_deref(), Some("HEAD"));
}

#[test]
fn the_message_is_what_git_writes() {
    let r = history();
    let p = preview(&r, "change");
    assert!(!p.merge && !p.reference);
    r.git(&["revert", "--no-edit", "change"]);
    assert_eq!(p.message.trim_end(), message(&r));
    assert!(
        p.message
            .starts_with("Revert \"change file\"\n\nThis reverts commit ")
    );
}

#[test]
fn a_revert_of_a_revert_reapplies() {
    let r = history();
    r.git(&["revert", "--no-edit", "change"]);
    let p = preview(&r, "HEAD");
    assert!(
        p.message.starts_with("Reapply \"change file\"\n\n"),
        "{}",
        p.message
    );
    // Git before 2.43 words it otherwise: parterre hands git the message it showed.
    let revert = plain(&p, false);
    assert!(revert.message.is_some());
    done(execute(&r, revert));
    assert_eq!(message(&r), p.message.trim_end());
    assert_eq!(text(&r, "file"), "two\n");
}

#[test]
fn reverting_makes_a_commit_at_the_tip_that_undoes_it() {
    let r = history();
    let head = rev(&r, "HEAD");
    let p = preview(&r, "change");
    let revert = plain(&p, false);
    assert_eq!(
        revert::commands(&revert),
        [vec![
            "revert".to_owned(),
            "--no-edit".into(),
            rev(&r, "change").to_hex()
        ]]
    );
    let report = done(execute(&r, revert));
    assert_eq!(report.created, Some(rev(&r, "HEAD")));
    assert!(report.attention.is_none() && report.stash.is_none());
    assert_eq!(rev(&r, "HEAD~1"), head);
    assert_eq!(text(&r, "file"), "one\n");
    assert_eq!(text(&r, "later"), "later\n");
    assert_eq!(message(&r), p.message.trim_end());
    assert!(message(&r).contains(&format!(
        "This reverts commit {}.",
        rev(&r, "change").to_hex()
    )));
}

#[test]
fn an_edited_message_is_used() {
    let r = history();
    let p = preview(&r, "change");
    let revert = p.revert(false, "Undo the change\n\nIt broke things.");
    assert!(revert::command(&revert).contains(&"--edit".to_owned()));
    done(execute(&r, revert));
    assert_eq!(message(&r), "Undo the change\n\nIt broke things.");
}

#[test]
fn an_empty_message_or_title_is_blocked() {
    let r = history();
    let p = preview(&r, "change");
    assert!(p.blocked(false, &p.message).is_none());
    assert_eq!(
        p.blocked(false, "  \n").as_deref(),
        Some("Enter a message for the revert.")
    );
    assert_eq!(
        p.blocked(false, "\n\nThis reverts commit x.").as_deref(),
        Some("Say why you're reverting on the title line.")
    );
}

#[test]
fn with_revert_reference_the_title_is_the_users() {
    let r = history();
    r.git(&["config", "revert.reference", "true"]);
    let p = preview(&r, "change");
    assert!(p.reference);
    let short = r.git(&["show", "-s", "--pretty=reference", "change"]);
    assert_eq!(p.message, format!("\n\nThis reverts commit {short}.\n"));
    assert!(p.blocked(false, &p.message).is_some());
    let edited = format!("It broke the build{}", p.message);
    done(execute(&r, p.revert(false, &edited)));
    assert_eq!(message(&r), edited.trim_end());
}

#[test]
fn a_merge_is_reverted_against_its_first_parent() {
    let mut r = history();
    r.git(&["branch", "feature"]);
    r.checkout("feature");
    r.write("feature", b"feature\n");
    r.commit_all("feature");
    r.checkout("main");
    r.write("main-only", b"main\n");
    r.commit_all("main only");
    r.merge("feature", "Merge branch 'feature'");
    let p = preview(&r, "HEAD");
    assert!(p.merge);
    let revert = plain(&p, false);
    assert!(
        revert::command(&revert)
            .windows(2)
            .any(|w| w == ["-m", "1"])
    );
    // Compare with git's own wording first, in a throwaway branch.
    r.git(&["switch", "-q", "-c", "check"]);
    r.git(&["revert", "--no-edit", "-m", "1", "HEAD"]);
    assert_eq!(p.message.trim_end(), message(&r));
    assert!(p.message.contains(", reversing\nchanges made to "));
    r.git(&["switch", "-q", "main"]);
    done(execute(&r, revert));
    assert!(!r.path().join("feature").exists());
    assert_eq!(text(&r, "main-only"), "main\n");
}

#[test]
fn local_changes_elsewhere_stay_unstashed() {
    let r = history();
    r.write("other", b"mine\n");
    let p = preview(&r, "change");
    assert!(p.stashable() && p.refusal(false).is_none());
    assert_eq!(
        p.caution(false).as_deref(),
        Some("1 file with local changes stays in the worktree")
    );
    assert!(p.caution(true).is_none());
    let report = done(execute(&r, plain(&p, false)));
    assert!(report.stash.is_none());
    assert_eq!(text(&r, "other"), "mine\n");
    assert_eq!(text(&r, "file"), "one\n");
}

#[test]
fn local_changes_to_a_file_the_revert_changes_need_a_stash() {
    let r = history();
    r.write("file", b"mine\n");
    let p = preview(&r, "change");
    assert_eq!(p.overlap, ["file"]);
    assert_eq!(p.refused(false), ["file"]);
    let short = &rev(&r, "change").to_hex()[..7];
    assert_eq!(
        p.refusal(false),
        Some(format!("{short} changes 1 file with local changes"))
    );
    assert!(p.refusal(true).is_none());
    assert!(p.blocked(false, &p.message).is_some());
    // Run anyway, as a stale dialog might: refused before git touches anything.
    let head = rev(&r, "HEAD");
    let (error, _) = failed(execute(&r, plain(&p, false)));
    assert!(error.contains("local changes changed"), "{error}");
    assert_eq!(rev(&r, "HEAD"), head);
    assert_eq!(text(&r, "file"), "mine\n");
}

#[test]
fn any_staged_change_needs_a_stash() {
    let r = history();
    r.stage("other", b"staged\n");
    let p = preview(&r, "change");
    assert_eq!(p.staged, ["other"]);
    assert_eq!(
        p.refusal(false).as_deref(),
        Some("1 file has staged changes")
    );
}

#[test]
fn stashing_reverts_and_leaves_the_changes_to_restore() {
    let r = history();
    r.write("other", b"also mine\n");
    r.stage("staged", b"staged\n");
    r.write("untracked", b"untracked\n");
    let p = preview(&r, "change");
    let revert = plain(&p, true);
    let commands = revert::commands(&revert);
    assert_eq!(commands[0][..3], ["stash", "push", "-m"]);
    let report = done(execute(&r, revert));
    let stash = report.stash.expect("stashed");
    assert_eq!(stash.files, ["other", "staged"]);
    assert_eq!(stash.oid, rev(&r, "stash@{0}"));
    assert_eq!(report.created, Some(rev(&r, "HEAD")));
    assert_eq!(text(&r, "file"), "one\n");
    assert_eq!(text(&r, "other"), "other\n");
    assert!(!r.path().join("staged").exists());
    assert_eq!(text(&r, "untracked"), "untracked\n", "untracked files stay");
    // Another entry on top since: it's found by its commit.
    r.write("later", b"someone else's\n");
    r.git(&["stash", "push", "-q", "-m", "theirs"]);
    let report = done(run(&r, Action::RestoreStash(stash.oid)));
    assert!(report.attention.is_none());
    assert_eq!(text(&r, "other"), "also mine\n");
    assert_eq!(text(&r, "staged"), "staged\n");
    assert_eq!(r.git(&["stash", "list", "--format=%s"]), "On main: theirs");
}

#[test]
fn a_restore_that_conflicts_keeps_the_entry() {
    let r = history();
    r.write("file", b"mine\n");
    let p = preview(&r, "change");
    let stash = done(execute(&r, plain(&p, true))).stash.unwrap();
    let report = done(run(&r, Action::RestoreStash(stash.oid)));
    let attention = report.attention.expect("conflicted");
    assert_eq!(
        attention.title,
        "Restoring stashed changes conflicted in 1 file"
    );
    assert!(attention.message.contains("stash@{0}"));
    assert_eq!(rev(&r, "stash@{0}"), stash.oid);
    assert!(text(&r, "file").contains("mine"));
}

#[test]
fn restoring_an_entry_gone_since_says_so() {
    let r = history();
    r.write("file", b"mine\n");
    let p = preview(&r, "change");
    let stash = done(execute(&r, plain(&p, true))).stash.unwrap();
    r.git(&["stash", "drop", "-q"]);
    let (error, _) = failed(run(&r, Action::RestoreStash(stash.oid)));
    assert!(error.contains("gone from the stash list"), "{error}");
}

/// main, where `file` changed again after `change`: reverting `change` conflicts.
fn conflicting() -> TestRepo {
    let mut r = history();
    r.write("file", b"three\n");
    r.commit_all("change file again");
    r
}

#[test]
fn a_conflict_leaves_the_revert_in_progress() {
    let r = conflicting();
    let p = preview(&r, "change");
    let report = done(execute(&r, plain(&p, false)));
    let attention = report.attention.expect("stopped");
    assert_eq!(attention.title, "Revert stopped on conflicts in 1 file");
    assert!(report.created.is_none() && report.stash.is_none());
    let catalog = Catalog::load(r.path()).unwrap();
    assert_eq!(catalog.stuck(), Some(Stuck::InProgress("a revert")));
    let open = catalog.worktrees.iter().find(|w| w.open).unwrap();
    assert_eq!(open.reverting, Some(rev(&r, "change")));
    assert_eq!(catalog.stashed_for_revert, None);
    // Nothing more is offered there until it's finished.
    assert!(Preview::load(r.path(), rev(&r, "HEAD")).is_err());
}

#[test]
fn a_conflict_after_stashing_names_the_entry_instead_of_restoring() {
    let r = conflicting();
    r.write("other", b"mine\n");
    let p = preview(&r, "change");
    let report = done(execute(&r, plain(&p, true)));
    assert!(report.stash.is_none(), "not restored over the conflict");
    let attention = report.attention.expect("stopped");
    assert!(
        attention
            .message
            .contains("Your changes are stashed in stash@{0}."),
        "{}",
        attention.message
    );
    let catalog = Catalog::load(r.path()).unwrap();
    assert_eq!(catalog.stashed_for_revert.as_deref(), Some("stash@{0}"));
    assert_eq!(text(&r, "other"), "other\n");
    r.git(&["revert", "--abort"]);
    // The test's git, without the system's `core.autocrlf`, as parterre's git checked out.
    r.git(&["reset", "-q", "--hard"]);
    r.git(&["stash", "pop", "-q"]);
    assert_eq!(text(&r, "other"), "mine\n");
}

/// main, where a later commit undid `change` by hand.
fn undone() -> TestRepo {
    let mut r = history();
    r.write("file", b"one\n");
    r.commit_all("undo by hand");
    r
}

#[test]
fn nothing_to_revert_leaves_no_operation_in_progress() {
    let r = undone();
    let head = rev(&r, "HEAD");
    let p = preview(&r, "change");
    let report = done(execute(&r, plain(&p, false)));
    let short = &rev(&r, "change").to_hex()[..7];
    assert_eq!(
        report.attention.expect("said so").title,
        format!("Nothing to revert: {short}'s changes are already undone")
    );
    assert_eq!(rev(&r, "HEAD"), head);
    assert!(report.created.is_none());
    assert_eq!(Catalog::load(r.path()).unwrap().stuck(), None);
}

#[test]
fn nothing_to_revert_after_stashing_still_offers_the_stash() {
    let r = undone();
    r.write("other", b"mine\n");
    let p = preview(&r, "change");
    let report = done(execute(&r, plain(&p, true)));
    assert!(report.attention.is_some());
    let stash = report.stash.expect("stashed");
    done(run(&r, Action::RestoreStash(stash.oid)));
    assert_eq!(text(&r, "other"), "mine\n");
}

#[test]
fn an_untracked_file_in_the_way_is_a_refusal_not_nothing_to_revert() {
    let mut r = history();
    r.git(&["rm", "-q", "other"]);
    r.commit("remove other");
    r.write("other", b"untracked\n");
    let p = preview(&r, "HEAD");
    let (error, _) = failed(execute(&r, plain(&p, false)));
    assert!(!error.is_empty());
    assert_eq!(text(&r, "other"), "untracked\n");
    assert_eq!(Catalog::load(r.path()).unwrap().stuck(), None);
}

#[test]
fn a_branch_that_moved_is_not_reverted() {
    let mut r = history();
    let p = preview(&r, "change");
    r.write("new", b"new\n");
    r.commit_all("meanwhile");
    let head = rev(&r, "HEAD");
    let (error, _) = failed(execute(&r, plain(&p, false)));
    assert!(error.contains("moved"), "{error}");
    assert_eq!(rev(&r, "HEAD"), head);
}

#[test]
fn a_detached_head_is_reverted_on() {
    let r = history();
    r.git(&["checkout", "-q", "--detach", "HEAD"]);
    let p = preview(&r, "change");
    assert_eq!(p.name(), "HEAD");
    let revert = plain(&p, false);
    assert_eq!(
        Action::Revert(Box::new(revert.clone())).label(),
        format!("Revert {} in HEAD", &rev(&r, "change").to_hex()[..7])
    );
    done(execute(&r, revert));
    assert_eq!(text(&r, "file"), "one\n");
    assert_eq!(rev(&r, "main"), rev(&r, "HEAD~1"));
}
