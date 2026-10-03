//! Merging into the open worktree's branch, against real, disposable repositories: when it's
//! offered, the merge methods, the message (checked against what `git merge` itself writes),
//! and every way it can stop, refuse or keep changes aside.
mod common;

use common::TestRepo;
use parterre_core::Oid;
use parterre_core::branches::{Action, Branches, Cancel, Catalog, Outcome, Report, Stuck};
use parterre_core::merge::{self, Method, Preview, Rebased};

fn oid(s: &str) -> Oid {
    Oid::from_hex(s).unwrap()
}

fn rev(r: &TestRepo, rev: &str) -> Oid {
    oid(&r.git(&["rev-parse", rev]))
}

fn preview(r: &TestRepo, target: &str) -> Preview {
    Preview::load(r.path(), rev(r, target), target).unwrap()
}

fn execute(r: &TestRepo, merge: merge::Merge) -> Outcome {
    Branches::new(r.path()).execute(Action::Merge(Box::new(merge)), None, &Cancel::default())
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

fn offered(r: &TestRepo, theirs: &str) -> Option<String> {
    let repo = r.load();
    let catalog = Catalog::load(r.path()).unwrap();
    merge::offered(&repo, &catalog, rev(r, theirs)).map(str::to_owned)
}

/// The merge `p` would run with git's message, by `method`, without stashing.
fn plain(p: &Preview, target: &str, method: Method) -> merge::Merge {
    p.merge(
        target.into(),
        method,
        Rebased::Copy,
        false,
        &p.message.clone(),
    )
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

/// `up` two commits ahead of `main`: a fast-forward would do.
fn behind() -> TestRepo {
    let mut r = TestRepo::new();
    r.write("base", b"base\n");
    r.commit_all("base");
    r.git(&["branch", "up"]);
    r.checkout("up");
    r.write("one", b"one\n");
    r.commit_all("one");
    r.write("two", b"two\n");
    r.commit_all("two");
    r.checkout("main");
    r
}

#[test]
fn it_is_offered_only_when_there_is_something_to_merge() {
    let mut r = diverged();
    assert_eq!(offered(&r, "up").as_deref(), Some("main"));
    // Already in main: nothing to do.
    assert_eq!(offered(&r, "main~1"), None);
    // The open worktree's own branch.
    assert_eq!(offered(&r, "main"), None);
    // Only behind: a fast-forward is a merge too.
    r.git(&["branch", "ahead", "main"]);
    r.checkout("ahead");
    r.write("more", b"more\n");
    r.commit_all("more");
    r.checkout("main");
    assert_eq!(offered(&r, "ahead").as_deref(), Some("main"));
    // A detached HEAD has no branch to merge into.
    r.git(&["checkout", "-q", "--detach", "main"]);
    assert_eq!(offered(&r, "up"), None);
}

#[test]
fn a_fast_forward_moves_the_branch_up() {
    let r = behind();
    let p = preview(&r, "up");
    assert!(p.fast_forward);
    assert_eq!(p.default_method(), Method::FastForward);
    assert_eq!(p.unavailable(Method::FastForward, "up"), None);
    let merge = plain(&p, "up", Method::FastForward);
    assert_eq!(merge::command(&merge), ["merge", "--ff-only", "up"]);
    let report = done(execute(&r, merge));
    assert!(report.attention.is_none());
    assert_eq!(rev(&r, "main"), rev(&r, "up"));
}

#[test]
fn a_merge_commit_where_a_fast_forward_would_do() {
    let r = behind();
    let p = preview(&r, "up");
    let merge = plain(&p, "up", Method::MergeCommit);
    assert_eq!(
        merge::command(&merge),
        ["merge", "--no-ff", "-m", "Merge branch 'up'", "up"]
    );
    done(execute(&r, merge));
    assert_eq!(rev(&r, "main^2"), rev(&r, "up"));
    assert_eq!(
        r.git(&["log", "-1", "--format=%B", "main"]),
        "Merge branch 'up'"
    );
}

#[test]
fn diverged_branches_cant_fast_forward() {
    let r = diverged();
    let p = preview(&r, "up");
    assert!(!p.fast_forward);
    assert_eq!(p.default_method(), Method::MergeCommit);
    assert_eq!(
        p.unavailable(Method::FastForward, "up").as_deref(),
        Some("main has commits up doesn't")
    );
    assert!(
        p.blocked(Method::FastForward, Rebased::Copy, false, &p.message, "up")
            .is_some()
    );
    assert_eq!(
        p.blocked(Method::MergeCommit, Rebased::Copy, false, &p.message, "up"),
        None
    );
    let mine = rev(&r, "main");
    done(execute(&r, plain(&p, "up", Method::MergeCommit)));
    assert_eq!(rev(&r, "main^1"), mine);
    assert_eq!(rev(&r, "main^2"), rev(&r, "up"));
}

#[test]
fn merge_ff_false_picks_a_merge_commit() {
    let r = behind();
    r.git(&["config", "merge.ff", "false"]);
    assert_eq!(preview(&r, "up").default_method(), Method::MergeCommit);
    r.git(&["config", "merge.ff", "only"]);
    assert_eq!(preview(&r, "up").default_method(), Method::FastForward);
}

/// What `git merge --no-ff <target>` writes on its own, undone afterwards.
fn gits_message(r: &TestRepo, target: &str) -> String {
    let head = r.git(&["rev-parse", "HEAD"]);
    r.git(&["merge", "-q", "--no-ff", "--no-edit", target]);
    let message = r.git(&["log", "-1", "--format=%B"]);
    r.git(&["reset", "-q", "--hard", &head]);
    message
}

#[test]
fn the_message_is_the_one_git_would_write() {
    let r = diverged();
    r.git(&["update-ref", "refs/remotes/origin/up", "up"]);
    assert_eq!(preview(&r, "up").message, gits_message(&r, "up"));
    assert_eq!(
        preview(&r, "origin/up").message,
        gits_message(&r, "origin/up")
    );
    assert_eq!(
        preview(&r, "origin/up").message,
        "Merge remote-tracking branch 'origin/up'"
    );
    // Into a branch other than main, git names it.
    r.git(&["switch", "-q", "-c", "topic"]);
    assert_eq!(preview(&r, "up").message, "Merge branch 'up' into topic");
    r.checkout("main");
    // With merge.log, git adds the commits merged, without fmt-merge-msg's comment lines.
    r.git(&["config", "merge.log", "true"]);
    let p = preview(&r, "up");
    assert!(p.log);
    assert_eq!(p.message, gits_message(&r, "up"));
    assert!(p.message.contains("* up:"), "{}", p.message);
    assert!(!p.message.contains('#'), "{}", p.message);
    // Not added twice when it runs.
    let merge = plain(&p, "up", Method::MergeCommit);
    assert!(merge::command(&merge).contains(&"--no-log".to_owned()));
    done(execute(&r, merge));
    assert_eq!(r.git(&["log", "-1", "--format=%B"]), p.message);
}

#[test]
fn the_message_is_what_the_user_wrote() {
    let r = diverged();
    let p = preview(&r, "up");
    assert_eq!(
        p.blocked(Method::MergeCommit, Rebased::Copy, false, "  \n", "up")
            .as_deref(),
        Some("Enter a message for the merge commit.")
    );
    // A fast-forward makes no commit.
    assert_eq!(
        behind_preview().blocked(Method::FastForward, Rebased::Copy, false, "", "up"),
        None
    );
    let message = "Bring in up\n\n#123 and its fix";
    done(execute(
        &r,
        p.merge(
            "up".into(),
            Method::MergeCommit,
            Rebased::Copy,
            false,
            message,
        ),
    ));
    assert_eq!(r.git(&["log", "-1", "--format=%B"]), message);
}

fn behind_preview() -> Preview {
    preview(&behind(), "up")
}

#[test]
fn a_commit_is_named_by_its_full_hash_and_short_in_the_message() {
    let r = diverged();
    let up = rev(&r, "up");
    let p = Preview::load(r.path(), up, &up.to_hex()).unwrap();
    let short = r.git(&["rev-parse", "--short", "up"]);
    assert_eq!(p.message, format!("Merge commit '{short}'"));
    r.git(&["config", "merge.log", "true"]);
    let logged = Preview::load(r.path(), up, &up.to_hex()).unwrap();
    assert_eq!(logged.message, gits_message(&r, &short));
    r.git(&["config", "--unset", "merge.log"]);
    let merge = plain(&p, &up.to_hex(), Method::MergeCommit);
    assert_eq!(merge::short_target(&merge), up.to_hex()[..7]);
    assert_eq!(merge::command(&merge).last(), Some(&up.to_hex()));
    done(execute(&r, merge));
    assert_eq!(rev(&r, "main^2"), up);
}

#[test]
fn a_conflict_stops_it_and_leaves_the_worktree_stuck() {
    let mut r = TestRepo::new();
    r.write("file", b"base\n");
    r.commit_all("base");
    r.git(&["branch", "up"]);
    r.write("file", b"mine\n");
    let mine = r.commit_all("mine");
    r.checkout("up");
    r.write("file", b"theirs\n");
    r.commit_all("theirs");
    r.checkout("main");

    let report = done(execute(
        &r,
        plain(&preview(&r, "up"), "up", Method::MergeCommit),
    ));
    let attention = report.attention.expect("an orange notice");
    assert_eq!(attention.title, "Merge stopped on conflicts in 1 file");
    assert_eq!(
        attention.message,
        "Finish or abort it with git, or go to another worktree."
    );

    let catalog = Catalog::load(r.path()).unwrap();
    assert_eq!(catalog.stuck(), Some(Stuck::InProgress("a merge")));
    assert_eq!(catalog.conflicted, ["file"]);
    let open = catalog.worktrees.iter().find(|w| w.open).unwrap();
    assert_eq!(open.merging, Some(rev(&r, "up")));
    assert_eq!(rev(&r, "main"), oid(&mine));
    // Git keeps the message for the commit that finishes it.
    let msg = std::fs::read_to_string(r.path().join(".git/MERGE_MSG")).unwrap();
    assert!(msg.starts_with("Merge branch 'up'"), "{msg}");

    // Nothing more is offered there, and a second merge is refused.
    assert_eq!(offered(&r, "up"), None);
    let refused = Preview::load(r.path(), rev(&r, "up"), "up")
        .unwrap_err()
        .to_string();
    assert_eq!(refused, "A merge is in progress in this worktree.");
}

#[test]
fn a_hook_that_refuses_leaves_the_merge_uncommitted() {
    let r = diverged();
    let hooks = r.path().join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-merge-commit");
    std::fs::write(&hook, "#!/bin/sh\necho refused by the hook >&2\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mine = rev(&r, "main");
    let report = done(execute(
        &r,
        plain(&preview(&r, "up"), "up", Method::MergeCommit),
    ));
    let attention = report.attention.expect("an orange notice");
    assert_eq!(attention.title, "Merge not committed");
    assert!(report.steps[0].output.contains("refused by the hook"));
    let catalog = Catalog::load(r.path()).unwrap();
    assert_eq!(catalog.stuck(), Some(Stuck::InProgress("a merge")));
    assert!(catalog.conflicted.is_empty());
    assert_eq!(rev(&r, "main"), mine);
}

#[test]
fn uncommitted_changes_need_the_stash_for_a_merge_commit() {
    let r = diverged();
    r.write("mine", b"edited\n");
    // Untracked files don't count.
    r.write("untracked", b"new\n");
    let p = preview(&r, "up");
    assert!(p.dirty);
    assert!(!p.auto_stash);
    assert_eq!(
        p.blocked(Method::MergeCommit, Rebased::Copy, false, &p.message, "up")
            .as_deref(),
        Some("Commit or stash your changes first.")
    );
    assert_eq!(
        p.blocked(Method::MergeCommit, Rebased::Copy, true, &p.message, "up"),
        None
    );

    let stashed = p.merge(
        "up".into(),
        Method::MergeCommit,
        Rebased::Copy,
        true,
        &p.message,
    );
    assert_eq!(
        merge::command(&stashed),
        [
            "merge",
            "--no-ff",
            "--autostash",
            "-m",
            "Merge branch 'up'",
            "up"
        ]
    );
    let report = done(execute(&r, stashed));
    assert!(report.attention.is_none());
    assert_eq!(rev(&r, "main^2"), rev(&r, "up"));
    assert_eq!(common::read_text(&r.path().join("mine")), "edited\n");
    assert_eq!(r.git(&["stash", "list"]), "");
}

#[test]
fn a_fast_forward_carries_uncommitted_changes_or_git_refuses() {
    let r = behind();
    r.write("base", b"edited\n");
    let p = preview(&r, "up");
    assert!(p.dirty);
    assert_eq!(
        p.blocked(Method::FastForward, Rebased::Copy, false, "", "up"),
        None
    );
    done(execute(&r, plain(&p, "up", Method::FastForward)));
    assert_eq!(rev(&r, "main"), rev(&r, "up"));
    assert_eq!(common::read_text(&r.path().join("base")), "edited\n");

    // A change to a file the fast-forward would overwrite: git refuses, and keeps it.
    let mut r = behind();
    r.checkout("up");
    r.write("base", b"theirs\n");
    r.commit_all("their base");
    r.checkout("main");
    r.write("base", b"edited\n");
    let tip = rev(&r, "main");
    let error = failed(execute(
        &r,
        plain(&preview(&r, "up"), "up", Method::FastForward),
    ));
    assert!(!error.is_empty());
    assert_eq!(rev(&r, "main"), tip);
    assert_eq!(common::read_text(&r.path().join("base")), "edited\n");
    assert_eq!(Catalog::load(r.path()).unwrap().stuck(), None);
}

#[test]
fn a_stopped_merge_keeps_the_stashed_changes_aside() {
    let mut r = TestRepo::new();
    r.write("file", b"base\n");
    r.write("other", b"other\n");
    r.commit_all("base");
    r.git(&["branch", "up"]);
    r.write("file", b"mine\n");
    r.commit_all("mine");
    r.checkout("up");
    r.write("file", b"theirs\n");
    r.commit_all("theirs");
    r.checkout("main");
    r.write("other", b"my edit\n");

    let p = preview(&r, "up");
    let report = done(execute(
        &r,
        p.merge(
            "up".into(),
            Method::MergeCommit,
            Rebased::Copy,
            true,
            &p.message,
        ),
    ));
    let attention = report.attention.expect("an orange notice");
    assert_eq!(attention.title, "Merge stopped on conflicts in 1 file");
    assert!(
        attention
            .message
            .ends_with("Your changes are set aside until then."),
        "{}",
        attention.message
    );
    // Git holds them until the merge is committed or aborted. (Not aborted here: the helpers'
    // git doesn't read the system config that parterre's does, see CLAUDE.md.)
    assert_eq!(r.git(&["show", "MERGE_AUTOSTASH:other"]), "my edit");
    assert_eq!(common::read_text(&r.path().join("other")), "other\n");
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

    let p = preview(&r, "up");
    let report = done(execute(
        &r,
        p.merge(
            "up".into(),
            Method::MergeCommit,
            Rebased::Copy,
            true,
            &p.message,
        ),
    ));
    let attention = report.attention.expect("an orange notice");
    assert_eq!(attention.title, "Merged up into main");
    assert_eq!(
        attention.message,
        "Putting your changes back conflicted in 1 file. Resolve it with git; your changes \
         are also kept in a stash entry until you drop it."
    );
    assert_eq!(rev(&r, "main^2"), rev(&r, "up"));
    assert_eq!(r.git(&["stash", "list"]).lines().count(), 1);
    assert!(common::read_text(&r.path().join("file")).contains("my edit"));
    let catalog = Catalog::load(r.path()).unwrap();
    assert_eq!(catalog.stuck(), Some(Stuck::Conflicts));
}

#[test]
fn merge_auto_stash_ticks_the_box_and_unticking_says_no_autostash() {
    let r = diverged();
    r.git(&["config", "merge.autoStash", "true"]);
    r.write("mine", b"edited\n");
    let p = preview(&r, "up");
    assert!(p.auto_stash);
    let command = |stash| {
        merge::command(&p.merge("up".into(), Method::FastForward, Rebased::Copy, stash, ""))
    };
    assert_eq!(command(true), ["merge", "--ff-only", "up"]);
    assert_eq!(
        command(false),
        ["merge", "--ff-only", "--no-autostash", "up"]
    );
}

#[test]
fn a_branch_that_moved_since_the_preview_is_left_alone() {
    let mut r = diverged();
    let p = preview(&r, "up");
    r.write("later", b"later\n");
    let later = r.commit_all("later");
    let error = failed(execute(&r, plain(&p, "up", Method::MergeCommit)));
    assert!(error.contains("moved"), "{error}");
    assert_eq!(rev(&r, "main"), oid(&later));
}
