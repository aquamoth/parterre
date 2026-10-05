//! Conflicted files, against real, disposable repositories stopped by a merge, rebase,
//! cherry-pick, revert or stash pop: every kind git leaves, what can finish each (checked
//! against the index after running it), the sides' names as git writes them, and the
//! comparison with the working tree listing them all.
mod common;

use std::process::Command;

use common::TestRepo;
use parterre_core::branches::{Action, Branches, Outcome};
use parterre_core::compare::Comparison;
use parterre_core::conflicts::{self, Answer, Conflict, Resolve, Sides};
use parterre_core::git::Git;
use parterre_core::{Oid, changed_files::FileStatus};
use parterre_util::CancelTree;

/// Runs git, which may fail (a merge that stops on conflicts).
fn try_git(r: &TestRepo, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(r.path())
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_EDITOR", "true")
        .output()
        .expect("run git")
        .status
        .success()
}

/// Commits a symlink (or a gitlink) straight into the index, so no file system support is
/// needed: `target` is the link's text (or the submodule's commit).
fn stage_mode(r: &TestRepo, mode: &str, path: &str, target: &str) {
    let oid = if mode == "160000" {
        target.to_owned()
    } else {
        r.git_with_input(&["hash-object", "-w", "--stdin"], target.as_bytes())
    };
    r.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("{mode},{oid},{path}"),
    ]);
}

fn listed(r: &TestRepo) -> Vec<Conflict> {
    conflicts::list(&Git::new(r.path())).unwrap().1
}

fn conflict(r: &TestRepo, path: &str) -> Conflict {
    listed(r)
        .into_iter()
        .find(|c| c.path == path)
        .unwrap_or_else(|| panic!("{path} not conflicted"))
}

fn sides(r: &TestRepo) -> Sides {
    let git = Git::new(r.path());
    let (root, list) = conflicts::list(&git).unwrap();
    conflicts::sides(&git, &root, &list)
}

fn answer(r: &TestRepo, path: &str, answer: Answer) -> Outcome {
    let resolve = Resolve {
        conflict: conflict(r, path),
        answer,
        item: String::new(),
    };
    Branches::new(r.path()).execute(
        Action::Resolve(Box::new(resolve)),
        None,
        &CancelTree::default(),
    )
}

fn done(out: Outcome) {
    assert!(matches!(out, Outcome::Done(_)), "expected it done: {out:?}");
}

fn unmerged(r: &TestRepo) -> Vec<String> {
    listed(r).into_iter().map(|c| c.path).collect()
}

/// The blob the index holds for `path` at stage 0, by its text.
fn staged(r: &TestRepo, path: &str) -> String {
    r.git(&["show", &format!(":0:{path}")])
}

fn staged_mode(r: &TestRepo, path: &str) -> String {
    r.git(&["ls-files", "--stage", "--", path])
        .split(' ')
        .next()
        .unwrap()
        .to_owned()
}

/// A merge of `feature` into `main` that stops with every kind of conflict.
fn merge_of_every_kind() -> TestRepo {
    let mut r = TestRepo::new();
    r.write("text.txt", b"one\ntwo\nthree\n");
    r.write("deleted-by-them.txt", b"base\n");
    r.write("deleted-by-us.txt", b"base\n");
    r.write("image.bin", b"PNG\0base");
    r.git(&["add", "-A"]);
    stage_mode(&r, "120000", "link", "target-base");
    stage_mode(&r, "160000", "sub", &"1".repeat(40));
    r.commit("Base");
    r.branch("feature");
    r.write("text.txt", b"one\nTWO (feature)\nthree\n");
    r.git(&["rm", "-q", "deleted-by-them.txt"]);
    r.write("deleted-by-us.txt", b"feature\n");
    r.write("image.bin", b"PNG\0feature");
    r.write("both-added.txt", b"feature's\n");
    r.git(&["add", "-A"]);
    stage_mode(&r, "120000", "link", "target-feature");
    stage_mode(&r, "160000", "sub", &"2".repeat(40));
    r.commit("Feature");
    r.checkout("main");
    r.write("text.txt", b"one\n2 (main)\nthree\n");
    r.write("deleted-by-them.txt", b"main\n");
    r.git(&["rm", "-q", "deleted-by-us.txt"]);
    r.write("image.bin", b"PNG\0main");
    r.write("both-added.txt", b"main's\n");
    r.git(&["add", "-A"]);
    stage_mode(&r, "120000", "link", "target-main");
    stage_mode(&r, "160000", "sub", &"3".repeat(40));
    r.commit("Main");
    assert!(!try_git(&r, &["merge", "-q", "feature"]), "the merge stops");
    r
}

#[test]
fn a_merge_lists_every_kind_with_git_s_codes() {
    let r = merge_of_every_kind();
    let codes: Vec<(String, &str)> = listed(&r)
        .iter()
        .map(|c| (c.path.clone(), c.code()))
        .collect();
    assert_eq!(
        codes,
        [
            ("both-added.txt".to_owned(), "AA"),
            ("deleted-by-them.txt".to_owned(), "UD"),
            ("deleted-by-us.txt".to_owned(), "DU"),
            ("image.bin".to_owned(), "UU"),
            ("link".to_owned(), "UU"),
            ("sub".to_owned(), "UU"),
            ("text.txt".to_owned(), "UU"),
        ]
    );
}

#[test]
fn only_text_on_both_sides_goes_to_the_merge_tool() {
    let r = merge_of_every_kind();
    let tool: Vec<(String, bool)> = listed(&r)
        .iter()
        .map(|c| (c.path.clone(), c.merge_tool().is_ok()))
        .collect();
    assert_eq!(
        tool,
        [
            ("both-added.txt".to_owned(), true),
            ("deleted-by-them.txt".to_owned(), false),
            ("deleted-by-us.txt".to_owned(), false),
            ("image.bin".to_owned(), false),
            ("link".to_owned(), false),
            ("sub".to_owned(), false),
            ("text.txt".to_owned(), true),
        ]
    );
    assert!(conflict(&r, "image.bin").binary);
    assert!(!conflict(&r, "text.txt").binary);
}

#[test]
fn what_finishes_each_kind_in_parterre() {
    let r = merge_of_every_kind();
    let answers = |p: &str| conflict(&r, p).answers();
    for p in [
        "text.txt",
        "both-added.txt",
        "deleted-by-them.txt",
        "deleted-by-us.txt",
        "image.bin",
        "link",
    ] {
        assert_eq!(answers(p), [Answer::Ours, Answer::Theirs], "{p}");
    }
    // Submodules are left to the terminal.
    assert_eq!(answers("sub"), []);
}

#[test]
fn taking_the_side_that_kept_a_file_stages_it() {
    let r = merge_of_every_kind();
    done(answer(&r, "deleted-by-them.txt", Answer::Ours));
    assert!(!unmerged(&r).contains(&"deleted-by-them.txt".to_owned()));
    assert_eq!(staged(&r, "deleted-by-them.txt"), "main");
    done(answer(&r, "deleted-by-us.txt", Answer::Theirs));
    assert_eq!(staged(&r, "deleted-by-us.txt"), "feature");
    assert!(r.path().join("deleted-by-us.txt").is_file());
}

#[test]
fn taking_the_side_that_deleted_a_file_deletes_it() {
    let r = merge_of_every_kind();
    let c = conflict(&r, "deleted-by-them.txt");
    assert!(c.deletes(Answer::Theirs));
    done(answer(&r, "deleted-by-them.txt", Answer::Theirs));
    assert!(!unmerged(&r).contains(&"deleted-by-them.txt".to_owned()));
    assert_eq!(r.git(&["ls-files", "--", "deleted-by-them.txt"]), "");
    assert!(!r.path().join("deleted-by-them.txt").exists());
}

#[test]
fn a_side_of_a_content_conflict_is_taken_whole() {
    let r = merge_of_every_kind();
    done(answer(&r, "text.txt", Answer::Theirs));
    assert!(!unmerged(&r).contains(&"text.txt".to_owned()));
    assert_eq!(staged(&r, "text.txt"), "one\nTWO (feature)\nthree");
}

#[test]
fn a_side_of_a_binary_file_is_taken_whole() {
    let r = merge_of_every_kind();
    done(answer(&r, "image.bin", Answer::Theirs));
    assert!(!unmerged(&r).contains(&"image.bin".to_owned()));
    assert_eq!(
        std::fs::read(r.path().join("image.bin")).unwrap(),
        b"PNG\0feature"
    );
    assert_eq!(staged(&r, "image.bin"), "PNG\0feature");
}

#[test]
fn a_side_of_a_symlink_keeps_it_a_symlink() {
    let r = merge_of_every_kind();
    done(answer(&r, "link", Answer::Ours));
    assert!(!unmerged(&r).contains(&"link".to_owned()));
    assert_eq!(staged_mode(&r, "link"), "120000");
    assert_eq!(staged(&r, "link"), "target-main");
}

#[test]
fn a_file_resolved_meanwhile_is_left_alone() {
    let r = merge_of_every_kind();
    let stale = conflict(&r, "deleted-by-them.txt");
    r.git(&["rm", "-q", "deleted-by-them.txt"]);
    let resolve = Resolve {
        conflict: stale,
        answer: Answer::Ours,
        item: String::new(),
    };
    let out = Branches::new(r.path()).execute(
        Action::Resolve(Box::new(resolve)),
        None,
        &CancelTree::default(),
    );
    match out {
        Outcome::Failed { error, report } => {
            assert!(
                error.to_string().contains("no longer conflicted"),
                "{error}"
            );
            assert!(report.steps.is_empty(), "nothing ran");
        }
        other => panic!("expected a refusal: {other:?}"),
    }
}

#[test]
fn an_answer_the_kind_doesn_t_have_is_refused() {
    let r = merge_of_every_kind();
    let out = answer(&r, "sub", Answer::Ours);
    assert!(matches!(out, Outcome::Failed { .. }), "{out:?}");
    assert!(unmerged(&r).contains(&"sub".to_owned()));
}

/// Sides with names, mine being stage `mine`.
fn named(ours: &str, theirs: &str, mine: u8) -> Sides {
    Sides {
        ours: Some(ours.to_owned()),
        theirs: Some(theirs.to_owned()),
        mine,
    }
}

#[test]
fn a_merge_s_sides_are_my_branch_and_the_branch_merged() {
    let r = merge_of_every_kind();
    let sides = sides(&r);
    assert_eq!(sides, named("main", "feature", 2));
    let gone = conflict(&r, "deleted-by-them.txt");
    assert_eq!(gone.item(&sides, Answer::Ours), "Use mine (main)");
    assert_eq!(
        gone.item(&sides, Answer::Theirs),
        "Use theirs (feature, deleted)"
    );
}

#[test]
fn a_merge_of_a_deletion_names_the_branch_merged() {
    let mut r = TestRepo::new();
    r.write("f", b"base\n");
    r.commit_all("Base");
    r.branch("topic");
    r.git(&["rm", "-q", "f"]);
    r.commit("Drop f");
    r.checkout("main");
    r.write("f", b"main\n");
    r.commit_all("Edit f");
    assert!(!try_git(&r, &["merge", "-q", "topic"]));
    assert_eq!(conflict(&r, "f").code(), "UD");
    assert_eq!(sides(&r), named("main", "topic", 2));
}

/// A file both sides of a history change, for a stop in each operation.
fn diverged() -> (TestRepo, String) {
    let mut r = TestRepo::new();
    r.write("list.txt", b"one\ntwo\nthree\n");
    r.commit_all("Base");
    r.branch("topic");
    r.write("list.txt", b"one\nTWO (topic)\nthree\n");
    let topic = r.commit_all("Topic: edit list");
    r.checkout("main");
    r.write("list.txt", b"one\n2 (main)\nthree\n");
    r.commit_all("Main: edit list");
    (r, topic)
}

fn short(r: &TestRepo, rev: &str) -> String {
    r.git(&["rev-parse", "--short", rev])
}

#[test]
fn a_rebase_s_mine_is_the_branch_replayed_onto_theirs() {
    let (r, _) = diverged();
    r.checkout("topic");
    assert!(!try_git(&r, &["rebase", "-q", "main"]));
    let sides = sides(&r);
    assert_eq!(sides, named("main", "topic", 3));
    let c = conflict(&r, "list.txt");
    assert_eq!(
        sides.mine_first(c.answers()),
        [Answer::Theirs, Answer::Ours]
    );
    assert_eq!(c.item(&sides, Answer::Theirs), "Use mine (topic)");
    assert_eq!(c.item(&sides, Answer::Ours), "Use theirs (main)");
}

#[test]
fn a_cherry_pick_s_theirs_is_the_commit_picked() {
    let (r, topic) = diverged();
    assert!(!try_git(&r, &["cherry-pick", &topic]));
    assert_eq!(sides(&r), named("main", &short(&r, &topic), 2));
}

#[test]
fn a_revert_s_theirs_is_the_revert() {
    let (mut r, _) = diverged();
    let edit = r.git(&["rev-parse", "HEAD"]);
    r.write("list.txt", b"one\n2 (main), again\nthree\n");
    r.commit_all("Main: edit again");
    assert!(!try_git(&r, &["revert", "--no-edit", &edit]));
    assert_eq!(
        sides(&r),
        named("main", &format!("revert of {}", short(&r, &edit)), 2)
    );
}

#[test]
fn a_stash_pop_s_theirs_is_named_as_the_markers_name_it() {
    let mut r = TestRepo::new();
    r.write("settings.ini", b"colour = blue\n");
    r.commit_all("Base");
    r.write("settings.ini", b"colour = green\n");
    r.git(&["stash", "-q"]);
    r.write("settings.ini", b"colour = red\n");
    r.commit_all("Red");
    assert!(!try_git(&r, &["stash", "pop", "-q"]));
    assert_eq!(sides(&r), named("main", "Stashed changes", 2));
    let c = conflict(&r, "settings.ini");
    assert!(c.merge_tool().is_ok());
    assert_eq!(c.answers(), [Answer::Ours, Answer::Theirs]);
}

#[test]
fn comparing_with_the_working_tree_lists_every_conflicted_file() {
    let r = merge_of_every_kind();
    let head = Oid::from_hex(&r.git(&["rev-parse", "HEAD"])).unwrap();
    let compared = Comparison::with_working_tree(head, false)
        .run(&Git::new(r.path()))
        .unwrap();
    let tree = compared.working_tree.expect("the working tree's side");
    assert_eq!(tree.conflicts.len(), 7);
    for c in &tree.conflicts {
        assert!(
            compared.files.iter().any(|f| f.path == c.path),
            "{} listed",
            c.path
        );
    }
    // `git diff HEAD` leaves out a file equal to HEAD's: the merge kept main's.
    let kept = compared
        .files
        .iter()
        .find(|f| f.path == "deleted-by-them.txt")
        .unwrap();
    assert_eq!(kept.status, FileStatus::Unmerged);
    assert_eq!(tree.sides.theirs.as_deref(), Some("feature"));
    // Not binary, so its diff is read as text.
    assert_eq!(kept.added, Some(0));
}

#[test]
fn comparing_commits_lists_no_conflicts() {
    let r = merge_of_every_kind();
    let head = Oid::from_hex(&r.git(&["rev-parse", "HEAD"])).unwrap();
    let base = Oid::from_hex(&r.git(&["rev-parse", "HEAD~"])).unwrap();
    let compared = Comparison {
        old: parterre_core::file_diff::Rev::Commit(base),
        new: parterre_core::file_diff::Rev::Commit(head),
        since_ancestor: false,
    }
    .run(&Git::new(r.path()))
    .unwrap();
    assert!(compared.working_tree.is_none());
}

#[test]
fn a_file_against_a_directory_is_finished_by_its_moved_aside_name() {
    let mut r = TestRepo::new();
    r.write("guide", b"the guide\n");
    r.commit_all("Base");
    r.branch("feature");
    r.git(&["rm", "-q", "guide"]);
    r.write("guide/index.md", b"a directory now\n");
    r.commit_all("Guide as a directory");
    r.checkout("main");
    r.write("guide", b"the guide, longer\n");
    r.commit_all("Longer guide");
    assert!(!try_git(&r, &["merge", "-q", "feature"]));
    let aside = listed(&r)
        .into_iter()
        .find(|c| c.path.starts_with("guide~"))
        .expect("git moves the file aside");
    assert_eq!(aside.code(), "UD");
    assert_eq!(aside.answers(), [Answer::Ours, Answer::Theirs]);
    // Their side is the folder: taking it deletes the file moved aside.
    done(answer(&r, &aside.path, Answer::Theirs));
    assert!(unmerged(&r).is_empty());
    assert!(r.path().join("guide/index.md").is_file());
}
