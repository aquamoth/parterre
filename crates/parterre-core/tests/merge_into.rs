//! Merging the open worktree's branch into another, as a pull request does, against real,
//! disposable repositories: when it's offered, where each step runs (the other branch's
//! worktree, or none), the rebase methods, the message, and every way it can stop, refuse or
//! keep changes aside.
mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{TestRepo, read_text};
use parterre_core::Oid;
use parterre_core::branches::{Action, Branches, Cancel, Catalog, Outcome, Report, Stuck};
use parterre_core::merge::{self, Method, Preview};

/// `main`, checked out in the main worktree, with a commit of its own; `feature`, checked out
/// in a linked worktree, the open one, with two; both from `base`.
struct Pr {
    r: TestRepo,
    _dir: tempfile::TempDir,
    /// The open worktree, on `feature`.
    wt: PathBuf,
}

impl Pr {
    fn new() -> Pr {
        Pr::with(|_| {})
    }

    /// With `more` run on `feature` before it's checked out in its worktree.
    fn with(more: impl FnOnce(&mut TestRepo)) -> Pr {
        let mut r = TestRepo::new();
        r.write("base", b"base\n");
        r.commit_all("base");
        r.git(&["branch", "feature"]);
        r.write("mine", b"mine\n");
        r.commit_all("mine");
        r.checkout("feature");
        r.write("one", b"one\n");
        r.commit_all("one");
        r.write("two", b"two\n");
        r.commit_all("two");
        more(&mut r);
        r.checkout("main");
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join("feature");
        r.git(&["worktree", "add", "-q", wt.to_str().unwrap(), "feature"]);
        Pr { r, _dir: dir, wt }
    }

    fn rev(&self, rev: &str) -> Oid {
        Oid::from_hex(&self.r.git(&["rev-parse", rev])).unwrap()
    }

    /// Git in the open worktree.
    fn here(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(&self.wt)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    fn preview(&self, into: &str) -> Preview {
        Preview::load_into(&self.wt, into).unwrap()
    }

    fn execute(&self, merge: merge::Merge) -> Outcome {
        Branches::new(&self.wt).execute(Action::Merge(Box::new(merge)), None, &Cancel::default())
    }

    fn offered(&self, at: &str) -> Option<String> {
        let repo = parterre_core::git::load_repo(&self.wt).unwrap();
        let catalog = Catalog::load(&self.wt).unwrap();
        merge::offered_into(&repo, &catalog, self.rev(at)).map(str::to_owned)
    }

    /// The main worktree's uncommitted changes, as parterre's git sees them: with the system
    /// config, whose `core.autocrlf` it checked the files out with (#182).
    fn main_status(&self) -> String {
        let out = Command::new("git")
            .current_dir(self.r.path())
            .args(["status", "--porcelain"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }
}

/// The merge `p` would run with git's message, by `method`, without stashing.
fn plain(p: &Preview, method: Method) -> merge::Merge {
    p.merge("feature".into(), method, false, &p.message.clone())
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

fn same(a: &Path, b: &Path) -> bool {
    std::fs::canonicalize(a).unwrap() == std::fs::canonicalize(b).unwrap()
}

fn main_folder(pr: &Pr) -> String {
    std::fs::canonicalize(pr.r.path())
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

#[test]
fn it_is_offered_into_branches_that_lack_the_open_branchs_commits() {
    let pr = Pr::new();
    assert_eq!(pr.offered("main").as_deref(), Some("feature"));
    // Behind feature: a fast-forward is a merge too.
    pr.r.git(&["branch", "behind", "main~1"]);
    assert_eq!(pr.offered("behind").as_deref(), Some("feature"));
    // Already has feature's commits.
    pr.r.git(&["branch", "has-it", "feature"]);
    assert_eq!(pr.offered("has-it"), None);
    // A detached HEAD has no branch to merge.
    pr.here(&["checkout", "-q", "--detach"]);
    assert_eq!(pr.offered("main"), None);
}

#[test]
fn a_branch_that_already_has_the_open_branchs_commits_has_no_preview() {
    let pr = Pr::new();
    pr.r.git(&["branch", "has-it", "feature"]);
    let error = Preview::load_into(&pr.wt, "has-it")
        .unwrap_err()
        .to_string();
    assert_eq!(error, "Branch has-it already has feature's commits.");
}

#[test]
fn only_merging_into_another_branch_offers_the_rebase_methods() {
    let pr = Pr::new();
    assert_eq!(pr.preview("main").methods(), Method::ALL);
    let incoming = Preview::load(&pr.wt, pr.rev("main"), "main").unwrap();
    assert_eq!(
        incoming.methods(),
        [Method::FastForward, Method::MergeCommit]
    );
}

#[test]
fn the_preview_finds_where_the_branch_is_checked_out() {
    let pr = Pr::new();
    let p = pr.preview("main");
    assert_eq!(p.branch, "main");
    assert_eq!(p.head, pr.rev("main"));
    assert_eq!(p.theirs, pr.rev("feature"));
    assert!(!p.fast_forward);
    assert_eq!((p.replays, p.merges), (2, 0));
    let out = p.outgoing.as_ref().unwrap();
    assert_eq!(out.source, "feature");
    assert!(same(out.worktree.as_deref().unwrap(), pr.r.path()));
    assert_eq!(p.message, "Merge branch 'feature'");
    pr.r.git(&["branch", "release", "main"]);
    let p = pr.preview("release");
    assert_eq!(p.outgoing.unwrap().worktree, None);
    assert_eq!(p.message, "Merge branch 'feature' into release");
}

/// What `git merge` itself writes, merging feature into `into` where it's checked out.
fn gits_message(pr: &Pr, into: &str) -> String {
    pr.r.checkout(into);
    pr.r.git(&["merge", "-q", "--no-ff", "--no-edit", "feature"]);
    let message = pr.r.git(&["log", "-1", "--format=%B"]);
    pr.r.git(&["reset", "-q", "--hard", "HEAD~1"]);
    pr.r.checkout("main");
    message
}

#[test]
fn the_message_is_the_one_git_would_write_there() {
    for log in [false, true] {
        let pr = Pr::new();
        pr.r.git(&["branch", "release", "main"]);
        pr.r.git(&["config", "merge.log", if log { "true" } else { "false" }]);
        let (main, release) = (gits_message(&pr, "main"), gits_message(&pr, "release"));
        // Worded where main is checked out.
        assert_eq!(pr.preview("main").message, main, "merge.log {log}");
        // As if in release, checked out nowhere; git adds merge.log's list there itself.
        let p = pr.preview("release");
        assert_eq!(p.message, "Merge branch 'feature' into release");
        done(pr.execute(plain(&p, Method::MergeCommit)));
        let made = pr.r.git(&["log", "-1", "--format=%B", "release"]);
        assert_eq!(made, release, "merge.log {log}");
    }
}

#[test]
fn rebase_and_fast_forward_into_a_branch_checked_out_elsewhere() {
    let pr = Pr::new();
    let mine = pr.rev("main");
    let p = pr.preview("main");
    assert_eq!(p.default_method(), Method::MergeCommit);
    assert_eq!(
        p.blocked(Method::RebaseFastForward, false, "", "feature"),
        None
    );
    let merge = plain(&p, Method::RebaseFastForward);
    let main = pr.r.path().to_str().unwrap().to_owned();
    let main = Catalog::load(pr.r.path())
        .unwrap()
        .worktrees
        .iter()
        .find(|w| w.main)
        .map(|w| w.path.to_string_lossy().into_owned())
        .unwrap_or(main);
    assert_eq!(
        merge::commands(&merge),
        [
            vec!["rebase", "main"],
            vec!["-C", &main, "merge", "--ff-only", "feature"],
        ]
    );
    let report = done(pr.execute(merge));
    assert!(report.attention.is_none(), "{:?}", report.attention);
    // feature replayed on main, and main moved up to it, its files checked out there.
    assert_eq!(pr.rev("feature~2"), mine);
    assert_eq!(pr.rev("main"), pr.rev("feature"));
    assert_eq!(read_text(&pr.r.path().join("two")), "two\n");
    assert_eq!(pr.main_status(), "");
    assert_eq!(pr.here(&["branch", "--show-current"]), "feature");
}

#[test]
fn a_semi_linear_merge_rebases_then_makes_a_merge_commit() {
    let pr = Pr::new();
    let mine = pr.rev("main");
    let p = pr.preview("main");
    done(pr.execute(plain(&p, Method::SemiLinear)));
    assert_eq!(pr.rev("feature~2"), mine);
    assert_eq!(pr.rev("main^1"), mine);
    assert_eq!(pr.rev("main^2"), pr.rev("feature"));
    assert_eq!(
        pr.r.git(&["log", "-1", "--format=%B", "main"]),
        "Merge branch 'feature'"
    );
    assert_eq!(pr.main_status(), "");
}

#[test]
fn a_merge_commit_leaves_the_open_branch_as_it_was() {
    let pr = Pr::new();
    let (mine, feature) = (pr.rev("main"), pr.rev("feature"));
    let p = pr.preview("main");
    done(pr.execute(plain(&p, Method::MergeCommit)));
    assert_eq!(pr.rev("feature"), feature);
    assert_eq!(pr.rev("main^1"), mine);
    assert_eq!(pr.rev("main^2"), feature);
}

#[test]
fn a_fast_forward_of_a_branch_checked_out_nowhere_moves_its_ref() {
    let pr = Pr::new();
    pr.r.git(&["branch", "release", "main~1"]);
    let p = pr.preview("release");
    assert!(p.fast_forward);
    assert_eq!(p.default_method(), Method::FastForward);
    // On top already, with no merges: a rebase would replay nothing.
    assert_eq!(
        p.unavailable(Method::RebaseFastForward, "feature")
            .as_deref(),
        Some("feature is already on top of release")
    );
    let merge = plain(&p, Method::FastForward);
    assert_eq!(
        merge::commands(&merge),
        [vec!["fetch", ".", "feature:release"]]
    );
    done(pr.execute(merge));
    assert_eq!(pr.rev("release"), pr.rev("feature"));
}

#[test]
fn rebase_and_fast_forward_into_a_branch_checked_out_nowhere() {
    let pr = Pr::new();
    pr.r.git(&["branch", "release", "main"]);
    let p = pr.preview("release");
    let merge = plain(&p, Method::RebaseFastForward);
    assert_eq!(
        merge::commands(&merge),
        [
            vec!["rebase", "release"],
            vec!["fetch", ".", "feature:release"]
        ]
    );
    done(pr.execute(merge));
    assert_eq!(pr.rev("release"), pr.rev("feature"));
    assert_eq!(pr.rev("feature~2"), pr.rev("main"));
}

#[test]
fn a_merge_commit_into_a_branch_checked_out_nowhere_switches_here_and_back() {
    let pr = Pr::new();
    pr.r.git(&["branch", "release", "main"]);
    let p = pr.preview("release");
    let merge = plain(&p, Method::SemiLinear);
    assert_eq!(
        merge::commands(&merge),
        [
            vec!["rebase", "release"],
            vec!["switch", "release"],
            vec![
                "merge",
                "--no-ff",
                "-m",
                "Merge branch 'feature' into release",
                "feature"
            ],
            vec!["switch", "feature"],
        ]
    );
    done(pr.execute(merge));
    assert_eq!(pr.here(&["branch", "--show-current"]), "feature");
    assert_eq!(pr.rev("release^1"), pr.rev("main"));
    assert_eq!(pr.rev("release^2"), pr.rev("feature"));
}

/// As [`Pr::new`], with a third commit on feature that adds `mine` too: it conflicts with
/// main's.
fn conflicting() -> Pr {
    Pr::with(|r| {
        r.write("mine", b"theirs\n");
        r.commit_all("their mine");
    })
}

#[test]
fn a_rebase_that_conflicts_stops_in_the_open_worktree() {
    let pr = conflicting();
    let (mine, feature) = (pr.rev("main"), pr.rev("feature"));
    let p = pr.preview("main");
    let report = done(pr.execute(plain(&p, Method::RebaseFastForward)));
    let attention = report.attention.expect("an orange notice");
    assert_eq!(attention.title, "Rebase stopped on conflicts in 1 file");
    assert!(
        attention
            .message
            .contains("Once rebased, feature merges into main")
    );
    assert_eq!(report.steps.len(), 1, "main isn't merged into");
    assert_eq!(pr.rev("main"), mine);
    assert_eq!(
        pr.rev("feature"),
        feature,
        "the branch moves when it's done"
    );
    let here = Catalog::load(&pr.wt).unwrap();
    assert_eq!(here.stuck(), Some(Stuck::InProgress("a rebase")));
    assert_eq!(pr.main_status(), "");
}

#[test]
fn a_merge_commit_that_conflicts_stops_in_the_branchs_worktree() {
    let pr = conflicting();
    let (mine, feature) = (pr.rev("main"), pr.rev("feature"));
    let p = pr.preview("main");
    let report = done(pr.execute(plain(&p, Method::MergeCommit)));
    let attention = report.attention.expect("an orange notice");
    assert_eq!(
        attention.title,
        format!(
            "Merge stopped on conflicts in 1 file in {}",
            main_folder(&pr)
        )
    );
    assert!(attention.message.contains("main is checked out there"));
    assert_eq!(pr.rev("main"), mine);
    assert_eq!(pr.rev("feature"), feature);
    let there = Catalog::load(pr.r.path()).unwrap();
    assert_eq!(there.stuck(), Some(Stuck::InProgress("a merge")));
    assert_eq!(Catalog::load(&pr.wt).unwrap().stuck(), None);
}

#[test]
fn a_merge_commit_into_a_branch_checked_out_nowhere_that_conflicts_stays_on_it() {
    let pr = conflicting();
    pr.r.git(&["branch", "release", "main"]);
    let p = pr.preview("release");
    let report = done(pr.execute(plain(&p, Method::MergeCommit)));
    let attention = report.attention.expect("an orange notice");
    assert_eq!(attention.title, "Merge stopped on conflicts in 1 file");
    assert!(
        attention
            .message
            .contains("This worktree is on release now")
    );
    assert_eq!(pr.here(&["branch", "--show-current"]), "release");
    let here = Catalog::load(&pr.wt).unwrap();
    assert_eq!(here.stuck(), Some(Stuck::InProgress("a merge")));
    assert_eq!(pr.rev("release"), pr.rev("main"));
}

#[test]
fn uncommitted_changes_here_need_the_stash_for_what_changes_this_worktree() {
    let pr = Pr::new();
    std::fs::write(pr.wt.join("one"), "edited\n").unwrap();
    pr.r.git(&["branch", "release", "main"]);
    let p = pr.preview("main");
    assert!(p.dirty);
    let blocked = |p: &Preview, m, stash| p.blocked(m, stash, &p.message, "feature");
    for m in [Method::RebaseFastForward, Method::SemiLinear] {
        assert!(p.stashable(m));
        assert_eq!(
            blocked(&p, m, false).as_deref(),
            Some("Commit or stash your changes first.")
        );
        assert_eq!(blocked(&p, m, true), None);
    }
    // Merging in main's worktree doesn't touch this one.
    assert!(!p.stashable(Method::MergeCommit));
    assert_eq!(blocked(&p, Method::MergeCommit, false), None);
    // Switching this worktree to release can't stash them.
    let release = pr.preview("release");
    assert!(!release.stashable(Method::MergeCommit));
    assert!(!release.stashable(Method::SemiLinear));
    assert_eq!(
        blocked(&release, Method::SemiLinear, true).as_deref(),
        Some("Commit or stash your changes first.")
    );
    // A fast-forward of a ref checked out nowhere doesn't either.
    assert!(release.stashable(Method::RebaseFastForward));

    let stashed = p.merge("feature".into(), Method::RebaseFastForward, true, "");
    assert_eq!(
        merge::commands(&stashed)[0],
        ["rebase", "--autostash", "main"]
    );
    let report = done(pr.execute(stashed));
    assert!(report.attention.is_none(), "{:?}", report.attention);
    assert_eq!(pr.rev("main"), pr.rev("feature"));
    assert_eq!(read_text(&pr.wt.join("one")), "edited\n");
    assert_eq!(pr.here(&["stash", "list"]), "");
}

#[test]
fn rebase_auto_stash_ticks_the_box_and_unticking_says_no_autostash() {
    let pr = Pr::new();
    pr.r.git(&["config", "rebase.autoStash", "true"]);
    std::fs::write(pr.wt.join("one"), "edited\n").unwrap();
    let p = pr.preview("main");
    assert!(p.auto_stash);
    let rebase = |stash| {
        merge::commands(&p.merge("feature".into(), Method::RebaseFastForward, stash, ""))[0].clone()
    };
    assert_eq!(rebase(true), ["rebase", "main"]);
    assert_eq!(rebase(false), ["rebase", "--no-autostash", "main"]);
}

#[test]
fn changes_the_autostash_cant_put_back_stay_in_the_stash() {
    let pr = Pr::new();
    // main changes base, which the open worktree has changed too.
    pr.r.write("base", b"main's base\n");
    pr.r.git(&["commit", "-q", "-am", "main's base"]);
    std::fs::write(pr.wt.join("base"), "my base\n").unwrap();
    let p = pr.preview("main");
    let report = done(pr.execute(p.merge("feature".into(), Method::RebaseFastForward, true, "")));
    let attention = report.attention.expect("an orange notice");
    assert_eq!(attention.title, "Merged feature into main");
    assert!(attention.message.contains("stash entry"), "{attention:?}");
    assert_eq!(pr.rev("main"), pr.rev("feature"));
    assert_eq!(pr.here(&["stash", "list"]).lines().count(), 1);
}

#[test]
fn uncommitted_changes_in_the_branchs_worktree_block_only_a_merge_commit() {
    let pr = Pr::new();
    pr.r.write("mine", b"edited there\n");
    let p = pr.preview("main");
    let why = format!(
        "main's worktree {} has uncommitted changes.",
        main_folder(&pr)
    );
    for m in [Method::MergeCommit, Method::SemiLinear] {
        assert_eq!(
            p.blocked(m, false, &p.message, "feature"),
            Some(why.clone())
        );
    }
    // A fast-forward carries them along, or git refuses.
    assert_eq!(
        p.blocked(Method::RebaseFastForward, false, "", "feature"),
        None
    );
    done(pr.execute(plain(&p, Method::RebaseFastForward)));
    assert_eq!(pr.rev("main"), pr.rev("feature"));
    assert_eq!(read_text(&pr.r.path().join("mine")), "edited there\n");
}

#[test]
fn a_branch_whose_worktree_has_an_operation_in_progress_takes_no_merge() {
    let mut pr = Pr::new();
    pr.r.git(&["branch", "side", "main~1"]);
    pr.r.checkout("side");
    pr.r.write("mine", b"side\n");
    pr.r.commit_all("side mine");
    pr.r.checkout("main");
    let out = Command::new("git")
        .current_dir(pr.r.path())
        .args(["merge", "-q", "side"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(!out.status.success(), "the merge conflicts");
    let p = pr.preview("main");
    let why = format!(
        "main's worktree {} has a merge in progress.",
        main_folder(&pr)
    );
    for m in Method::ALL {
        if p.unavailable(m, "feature").is_none() {
            assert_eq!(
                p.blocked(m, false, &p.message, "feature"),
                Some(why.clone())
            );
        }
    }
}

#[test]
fn a_rebase_that_would_replay_nothing_is_unavailable() {
    // feature's only commit is already in main, as the same patch.
    let mut r = TestRepo::new();
    r.write("base", b"base\n");
    r.commit_all("base");
    r.git(&["branch", "feature"]);
    r.write("fix", b"fixed\n");
    r.commit_all("fix");
    r.write("more", b"more\n");
    r.commit_all("more");
    r.checkout("feature");
    r.write("fix", b"fixed\n");
    r.commit_all("fix");
    r.checkout("main");
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().join("feature");
    r.git(&["worktree", "add", "-q", wt.to_str().unwrap(), "feature"]);
    let p = Preview::load_into(&wt, "main").unwrap();
    assert_eq!(p.replays, 0);
    for m in [Method::RebaseFastForward, Method::SemiLinear] {
        assert_eq!(
            p.unavailable(m, "feature").as_deref(),
            Some("main already has feature's changes")
        );
    }
    assert_eq!(p.unavailable(Method::MergeCommit, "feature"), None);
}

#[test]
fn a_merge_inside_the_branch_makes_the_rebase_worth_it() {
    let pr = Pr::new();
    // feature2: on top of main, with a merge inside, which the rebase flattens.
    pr.here(&["switch", "-q", "-c", "feature2", "main"]);
    pr.here(&["switch", "-q", "-c", "side"]);
    std::fs::write(pr.wt.join("side"), "side\n").unwrap();
    pr.here(&["add", "side"]);
    pr.here(&["commit", "-q", "-m", "side"]);
    pr.here(&["switch", "-q", "feature2"]);
    pr.here(&["merge", "-q", "--no-ff", "-m", "Merge side", "side"]);
    let p = Preview::load_into(&pr.wt, "main").unwrap();
    assert!(p.fast_forward);
    assert_eq!((p.replays, p.merges), (1, 1));
    assert_eq!(p.unavailable(Method::RebaseFastForward, "feature2"), None);
}

#[test]
fn a_branch_that_moved_since_the_preview_is_left_alone() {
    let mut pr = Pr::new();
    let feature = pr.rev("feature");
    let p = pr.preview("main");
    pr.r.write("later", b"later\n");
    pr.r.commit_all("later");
    let later = pr.rev("main");
    let error = failed(pr.execute(plain(&p, Method::RebaseFastForward)));
    assert!(error.contains("moved"), "{error}");
    assert_eq!(pr.rev("feature"), feature, "nothing was rebased");
    assert_eq!(pr.rev("main"), later);
}

#[test]
fn a_branch_checked_out_since_the_preview_is_left_alone() {
    let pr = Pr::new();
    pr.r.git(&["branch", "release", "main"]);
    let p = pr.preview("release");
    pr.r.checkout("release");
    let error = failed(pr.execute(plain(&p, Method::MergeCommit)));
    assert!(error.contains("checked out"), "{error}");
    assert_eq!(pr.rev("release"), pr.rev("main"));
}
