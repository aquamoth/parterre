//! Resetting the open worktree's branch, against real, disposable repositories: what each mode
//! is predicted to do is checked against what `git reset` then does, and every way a reset can
//! lose work is covered.
mod common;

use std::collections::BTreeSet;

use common::TestRepo;
use parterre_core::Oid;
use parterre_core::branches::{Action, Branches, Cancel, Catalog, Outcome};
use parterre_core::reset::{self, Lost, Mode, Preview};

fn oid(s: &str) -> Oid {
    Oid::from_hex(s).unwrap()
}

fn rev(r: &TestRepo, rev: &str) -> Oid {
    oid(&r.git(&["rev-parse", rev]))
}

fn preview(r: &TestRepo, target: &str) -> Preview {
    Preview::load(r.path(), rev(r, target)).unwrap()
}

fn execute(r: &TestRepo, reset: reset::Reset) -> Outcome {
    Branches::new(r.path()).execute(Action::Reset(Box::new(reset)), None, &Cancel::default())
}

fn done(out: Outcome) {
    assert!(matches!(out, Outcome::Done(_)), "{out:?}");
}

fn failed(out: Outcome) -> String {
    match out {
        Outcome::Failed { error, .. } => error.to_string(),
        other => panic!("expected a failure: {other:?}"),
    }
}

fn read(r: &TestRepo, path: &str) -> String {
    std::fs::read_to_string(r.path().join(path)).unwrap()
}

/// `git status --short` for the paths the preview lists, as `XY path` lines.
fn real_status(r: &TestRepo, paths: &BTreeSet<String>) -> BTreeSet<String> {
    // Not trimmed, as `TestRepo::git` has it: the first line may start with a space.
    let out = std::process::Command::new("git")
        .current_dir(r.path())
        .args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
        ])
        .output()
        .unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .split('\0')
        .filter(|e| e.len() > 3 && paths.contains(&e[3..]))
        .map(str::to_owned)
        .collect()
}

/// The status the preview predicts for `mode`, in the same form.
fn predicted_status(p: &Preview, mode: Mode) -> BTreeSet<String> {
    let letter = |c: Option<reset::Change>| c.map_or(' ', |c| c.letter());
    let mut lines = BTreeSet::new();
    for f in p.files(mode) {
        if f.staged.is_some() || f.unstaged.is_some() {
            lines.insert(format!(
                "{}{} {}",
                letter(f.staged),
                letter(f.unstaged),
                f.path
            ));
        }
        if f.untracked {
            lines.insert(format!("?? {}", f.path));
        }
    }
    lines
}

/// For every mode git runs, in a fresh copy of the scenario: the files' status afterwards is
/// what the preview predicted.
fn predictions_match_git(scenario: fn() -> TestRepo, target: &str) {
    for mode in Mode::ALL {
        let r = scenario();
        let p = preview(&r, target);
        if p.refusal(mode).is_some() {
            continue;
        }
        let paths: BTreeSet<String> = p.files(mode).into_iter().map(|f| f.path).collect();
        let predicted = predicted_status(&p, mode);
        done(execute(&r, p.reset(mode)));
        assert_eq!(real_status(&r, &paths), predicted, "{mode:?}");
        assert_eq!(rev(&r, "HEAD"), p.target, "{mode:?}");
        assert_eq!(r.git(&["symbolic-ref", "HEAD"]), "refs/heads/main");
    }
}

/// main: base (a.txt, lib.txt) → tip (a.txt changed, notes.txt added), with tip on no other
/// branch.
fn two_commits() -> TestRepo {
    let mut r = TestRepo::new();
    r.write("a.txt", b"one\n");
    r.write("lib.txt", b"lib\n");
    r.commit_all("base");
    r.write("a.txt", b"two\n");
    r.write("notes.txt", b"notes\n");
    r.commit_all("tip");
    r
}

#[test]
fn commits_left_behind_are_lost_in_every_mode() {
    let r = two_commits();
    let p = preview(&r, "HEAD~1");
    let tip = rev(&r, "HEAD");
    assert_eq!(p.commits, vec![tip]);
    assert_eq!((p.behind, p.ahead), (1, 0));
    for mode in Mode::ALL {
        assert!(p.loses(mode), "{mode:?}");
    }
    // Nothing to choose between: Keep, the safe one.
    assert_eq!(p.default_mode(), Mode::Keep);
    done(execute(&r, p.reset(Mode::Keep)));
    assert_eq!(r.git(&["branch", "--contains", &tip.to_hex()]), "");
    assert_eq!(read(&r, "a.txt"), "one\n");
    assert!(!r.path().join("notes.txt").exists());
}

#[test]
fn commits_another_branch_has_are_not_lost() {
    let r = two_commits();
    r.git(&["branch", "pushed"]);
    let p = preview(&r, "HEAD~1");
    assert!(p.commits.is_empty());
    assert!(!p.loses(Mode::Keep));
    assert_eq!(p.default_mode(), Mode::Keep);
    assert_eq!(
        p.movement(7).split(" from").next(),
        Some("Moves main back 1 commit")
    );
}

#[test]
fn soft_and_mixed_keep_the_changes_of_the_commits_left_behind() {
    predictions_match_git(two_commits, "HEAD~1");
    let r = two_commits();
    let p = preview(&r, "HEAD~1");
    let soft: Vec<_> = p
        .files(Mode::Soft)
        .into_iter()
        .map(|f| (f.path, f.staged.map(|c| c.letter())))
        .collect();
    assert_eq!(
        soft,
        [("a.txt".into(), Some('M')), ("notes.txt".into(), Some('A'))]
    );
}

/// Forward, to a commit on another branch the branch is behind.
#[test]
fn moving_forward_loses_nothing_and_updates_the_files() {
    let mut r = TestRepo::new();
    r.write("a.txt", b"one\n");
    r.commit_all("base");
    r.branch("ahead");
    r.write("a.txt", b"two\n");
    r.commit_all("more");
    r.checkout("main");
    let p = preview(&r, "ahead");
    assert_eq!((p.behind, p.ahead), (0, 1));
    assert!(p.commits.is_empty());
    let keep = p.files(Mode::Keep);
    assert!(keep[0].updated);
    assert_eq!(keep[0].lines, Some((1, 1)));
    done(execute(&r, p.reset(Mode::Keep)));
    assert_eq!(read(&r, "a.txt"), "two\n");
}

/// a.txt staged as S, then edited to W; new.txt staged. Mixed and Keep unstage everything,
/// losing S; Soft keeps the index; Hard loses all of it.
fn partially_staged() -> TestRepo {
    let r = two_commits();
    r.git(&["branch", "pushed"]);
    r.write("lib.txt", b"S\n");
    r.git(&["add", "lib.txt"]);
    r.write("lib.txt", b"W\n");
    r.write("new.txt", b"new\n");
    r.git(&["add", "new.txt"]);
    r
}

#[test]
fn a_partially_staged_file_loses_its_staged_version() {
    predictions_match_git(partially_staged, "HEAD~1");
    let r = partially_staged();
    let p = preview(&r, "HEAD~1");
    let lost = |mode| p.lost_files(mode);
    assert!(lost(Mode::Soft).is_empty());
    assert_eq!(lost(Mode::Mixed), ["lib.txt"]);
    assert_eq!(lost(Mode::Keep), ["lib.txt"]);
    assert_eq!(lost(Mode::Hard), ["lib.txt", "new.txt"]);
    assert_eq!(p.default_mode(), Mode::Soft);
    let lib = |mode| {
        p.files(mode)
            .into_iter()
            .find(|f| f.path == "lib.txt")
            .unwrap()
    };
    assert_eq!(lib(Mode::Mixed).lost, Some(Lost::StagedVersion));
    assert_eq!(lib(Mode::Hard).lost, Some(Lost::StagedVersionAndChanges));

    done(execute(&r, p.reset(Mode::Mixed)));
    // Only W remains, and nothing is staged: S is gone.
    assert_eq!(read(&r, "lib.txt"), "W\n");
    assert_eq!(r.git(&["diff", "--cached", "--name-only"]), "");
}

#[test]
fn hard_loses_every_uncommitted_change() {
    let r = partially_staged();
    let p = preview(&r, "HEAD~1");
    done(execute(&r, p.reset(Mode::Hard)));
    assert_eq!(read(&r, "lib.txt"), "lib\n");
    assert!(!r.path().join("new.txt").exists());
    assert_eq!(r.git(&["status", "--porcelain"]), "");
}

/// An edit to lib.txt, which the commits left behind didn't touch, and an untracked file.
fn dirty() -> TestRepo {
    let r = two_commits();
    r.git(&["branch", "pushed"]);
    r.write("lib.txt", b"edited\n");
    r.write("scratch.txt", b"mine\n");
    r
}

#[test]
fn keep_keeps_uncommitted_changes_and_hard_drops_them() {
    predictions_match_git(dirty, "HEAD~1");
    let r = dirty();
    let p = preview(&r, "HEAD~1");
    assert_eq!(p.default_mode(), Mode::Keep);
    assert_eq!(p.lost_files(Mode::Hard), ["lib.txt"]);
    // An untracked file out of the target's way isn't the reset's concern.
    assert!(p.files(Mode::Hard).iter().all(|f| f.path != "scratch.txt"));
    done(execute(&r, p.reset(Mode::Keep)));
    assert_eq!(read(&r, "lib.txt"), "edited\n");
    assert_eq!(read(&r, "a.txt"), "one\n");
    assert_eq!(read(&r, "scratch.txt"), "mine\n");
}

/// Back to a commit that has lib.txt, from one that removed it, with an untracked lib.txt on
/// disk: Keep refuses, Hard writes over it.
fn in_the_way() -> TestRepo {
    let mut r = TestRepo::new();
    r.write("a.txt", b"one\n");
    r.write("lib.txt", b"lib\none\n");
    r.commit_all("base");
    r.git(&["rm", "-q", "lib.txt"]);
    r.commit_all("remove lib");
    r.git(&["branch", "pushed"]);
    r.write("lib.txt", b"lib\nmine\n");
    r
}

#[test]
fn hard_writes_over_an_untracked_file_in_the_way() {
    predictions_match_git(in_the_way, "HEAD~1");
    let r = in_the_way();
    let p = preview(&r, "HEAD~1");
    let refusal = p.refusal(Mode::Keep).expect("git refuses --keep");
    assert!(refusal.contains("'lib.txt'"), "{refusal}");
    let keep = p.files(Mode::Keep);
    assert!(keep.iter().find(|f| f.path == "lib.txt").unwrap().refused);
    let hard = p.files(Mode::Hard);
    let lib = hard.iter().find(|f| f.path == "lib.txt").unwrap();
    assert_eq!(lib.lost, Some(Lost::Untracked));
    assert_eq!(lib.lines, Some((1, 1)));
    assert!(p.loses(Mode::Hard));
    // Mixed leaves it where it is, and loses nothing.
    assert_eq!(p.default_mode(), Mode::Mixed);

    done(execute(&r, p.reset(Mode::Hard)));
    assert_eq!(read(&r, "lib.txt"), "lib\none\n");
}

#[test]
fn an_untracked_file_where_the_target_has_a_folder_is_in_the_way() {
    let mut r = TestRepo::new();
    r.write("a.txt", b"one\n");
    r.write("docs/x.txt", b"x\n");
    r.commit_all("base");
    r.git(&["rm", "-q", "-r", "docs"]);
    r.commit_all("remove docs");
    r.git(&["branch", "pushed"]);
    r.write("docs", b"a file now\n");
    let p = preview(&r, "HEAD~1");
    assert_eq!(p.lost_files(Mode::Hard), ["docs"]);
    assert!(p.refusal(Mode::Keep).is_some());
}

#[test]
fn an_ignored_file_in_the_way_is_never_lost_work() {
    let mut r = in_the_way();
    std::fs::remove_file(r.path().join("lib.txt")).unwrap();
    r.write(".gitignore", b"lib.txt\n");
    r.commit_all("ignore lib");
    r.git(&["branch", "-f", "pushed"]);
    r.write("lib.txt", b"ignored\n");
    let p = preview(&r, "HEAD~2");
    assert!(p.lost_files(Mode::Hard).is_empty());
}

/// A change to the file the commits left behind changed: git refuses Keep and names it.
#[test]
fn git_refuses_keep_over_a_change_to_a_file_the_commits_changed() {
    let r = two_commits();
    r.git(&["branch", "pushed"]);
    r.write("a.txt", b"mine\n");
    let p = preview(&r, "HEAD~1");
    let refusal = p
        .refusal(Mode::Keep)
        .expect("git refuses --keep")
        .to_owned();
    assert!(refusal.contains("'a.txt'"), "{refusal}");
    let a = p
        .files(Mode::Keep)
        .into_iter()
        .find(|f| f.path == "a.txt")
        .unwrap();
    assert!(a.refused && !a.updated && a.lost.is_none());
    assert!(a.words().ends_with("git refuses"), "{}", a.words());
    // Running it anyway: git refuses, and nothing changes.
    let tip = rev(&r, "HEAD");
    failed(execute(&r, p.reset(Mode::Keep)));
    assert_eq!(rev(&r, "HEAD"), tip);
    assert_eq!(read(&r, "a.txt"), "mine\n");
}

/// A file only touched since git last looked is no change: git's dry run would refuse, and so
/// would `reset --keep`, without the index refreshed first.
#[test]
fn a_touched_file_is_no_reason_to_refuse() {
    let r = two_commits();
    r.git(&["branch", "pushed"]);
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    let file = std::fs::File::options()
        .write(true)
        .open(r.path().join("a.txt"))
        .unwrap();
    file.set_modified(old).unwrap();
    let p = preview(&r, "HEAD~1");
    assert_eq!(p.refusal(Mode::Keep), None);
    file.set_modified(old + std::time::Duration::from_secs(60))
        .unwrap();
    done(execute(&r, p.reset(Mode::Keep)));
}

#[test]
fn a_branch_checked_out_in_another_worktree_too_is_refused() {
    let r = two_commits();
    r.git(&["branch", "pushed"]);
    let other = tempfile::tempdir().unwrap();
    let place = other.path().join("again");
    r.git(&[
        "worktree",
        "add",
        "-q",
        "--force",
        place.to_str().unwrap(),
        "main",
    ]);
    let p = preview(&r, "HEAD~1");
    for mode in Mode::ALL {
        let refusal = p.refusal(mode).expect("refused");
        assert!(refusal.contains("in use by"), "{refusal}");
    }
    let tip = rev(&r, "HEAD");
    failed(execute(&r, p.reset(Mode::Soft)));
    assert_eq!(rev(&r, "HEAD"), tip);
}

#[test]
fn a_reset_that_would_lose_more_than_agreed_to_does_not_run() {
    let r = two_commits();
    r.git(&["branch", "pushed"]);
    r.write("lib.txt", b"edited\n");
    let p = preview(&r, "HEAD~1");
    assert!(p.lost_files(Mode::Hard) == ["lib.txt"]);
    let agreed = p.reset(Mode::Hard);
    // Another edit since the dialog was read.
    r.write("notes.txt", b"more notes\n");
    let error = failed(execute(&r, agreed));
    assert!(error.contains("changed"), "{error}");
    assert_eq!(read(&r, "notes.txt"), "more notes\n");
    assert_eq!(read(&r, "lib.txt"), "edited\n");
}

#[test]
fn a_branch_moved_since_is_not_reset() {
    let mut r = two_commits();
    let p = preview(&r, "HEAD~1");
    let agreed = p.reset(Mode::Keep);
    let moved = r.commit("meanwhile");
    let error = failed(execute(&r, agreed));
    assert!(error.contains("moved"), "{error}");
    assert_eq!(r.git(&["rev-parse", "HEAD"]), moved);
}

#[test]
fn no_reset_is_offered_detached_mid_merge_or_where_the_branch_is() {
    let mut r = two_commits();
    let tip = rev(&r, "HEAD");
    let base = rev(&r, "HEAD~1");
    let catalog = Catalog::load(r.path()).unwrap();
    assert_eq!(reset::branch(&catalog, base), Ok("main"));
    assert!(reset::branch(&catalog, tip).is_err());

    r.git(&["switch", "-q", "-c", "other", "HEAD~1"]);
    r.write("a.txt", b"theirs\n");
    r.commit_all("theirs");
    r.checkout("main");
    let out = std::process::Command::new("git")
        .current_dir(r.path())
        .args(["merge", "-q", "other"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(!out.status.success(), "the merge stops on a conflict");
    let catalog = Catalog::load(r.path()).unwrap();
    let why = reset::branch(&catalog, base).unwrap_err();
    assert!(why.contains("merge is in progress"), "{why}");
    r.git(&["merge", "--abort"]);

    r.git(&["switch", "-q", "--detach", "HEAD"]);
    let catalog = Catalog::load(r.path()).unwrap();
    assert!(reset::branch(&catalog, base).is_err());
}

#[test]
fn diffs_show_what_changes_on_disk_or_what_stays_uncommitted() {
    let r = dirty();
    let p = preview(&r, "HEAD~1");
    let keep = p.files(Mode::Keep);
    let a = keep.iter().find(|f| f.path == "a.txt").unwrap();
    let spec = p.diff(a);
    // Keep rewrites a.txt: the file now against the target's.
    assert_eq!(
        spec.old.unwrap().rev,
        parterre_core::file_diff::Rev::WorkingTree
    );
    let lib = keep.iter().find(|f| f.path == "lib.txt").unwrap();
    let spec = p.diff(lib);
    // lib.txt stays modified: the target's against the file.
    assert_eq!(
        spec.old.unwrap().rev,
        parterre_core::file_diff::Rev::Commit(p.target)
    );
}
