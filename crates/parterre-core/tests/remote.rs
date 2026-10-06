//! Fetching, pulling and pushing against a local bare remote (#181), with a second clone that
//! pushes behind parterre's back. Every path that can lose work is covered: the force push and
//! its lease, a pull that stops on conflicts, and a push the remote rejects.
mod common;

use common::TestRepo;
use parterre_core::Oid;
use parterre_core::branches::{Action, Branches, Catalog, Outcome, Report, Stuck, Warning};
use parterre_core::remote::{
    self, Diverged, Live, Pull, Push, PushState, Reconcile, RemoteBranchTip, SetUpstream,
};
use parterre_util::CancelTree;
use tempfile::TempDir;

/// The bare remote `origin`, the repository parterre has open (`work`), and another clone
/// (`other`), both on `main` tracking `origin/main`, at one shared commit.
struct Setup {
    origin: TempDir,
    work: TestRepo,
    other: TestRepo,
}

fn bare(dir: &TempDir, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn setup() -> Setup {
    let origin = tempfile::tempdir().unwrap();
    bare(&origin, &["init", "-q", "--bare", "-b", "main"]);
    let url = origin.path().to_str().unwrap().to_owned();
    let mut work = TestRepo::new();
    work.write("base", b"base\n");
    work.commit_all("base");
    work.git(&["remote", "add", "origin", &url]);
    work.git(&["push", "-q", "-u", "origin", "main"]);
    let other = TestRepo::new();
    other.git(&["remote", "add", "origin", &url]);
    other.git(&["fetch", "-q", "origin"]);
    other.git(&["checkout", "-q", "-B", "main", "--track", "origin/main"]);
    Setup {
        origin,
        work,
        other,
    }
}

fn oid(s: &str) -> Oid {
    Oid::from_hex(s).unwrap()
}

fn rev(r: &TestRepo, rev: &str) -> Oid {
    oid(&r.git(&["rev-parse", rev]))
}

fn on_origin(s: &Setup, branch: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(s.origin.path())
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .unwrap();
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn execute(r: &TestRepo, action: Action, approval: Option<&Warning>) -> Outcome {
    Branches::new(r.path()).execute(action, approval, &CancelTree::default())
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

fn warning(out: Outcome) -> Warning {
    match out {
        Outcome::Warning(w) => w,
        other => panic!("expected a warning: {other:?}"),
    }
}

fn commands(report: &Report) -> Vec<String> {
    report
        .steps
        .iter()
        .map(|s| parterre_core::branches::command_text(&s.args))
        .collect()
}

fn pull(r: &TestRepo, how: Option<Reconcile>) -> Action {
    Action::Pull(Box::new(Pull {
        branch: "main".into(),
        head: rev(r, "main"),
        how,
    }))
}

fn push(r: &TestRepo, branch: &str) -> Action {
    Action::Push(Box::new(Push {
        branch: branch.into(),
        tip: rev(r, branch),
        remote: "origin".into(),
    }))
}

/// Whether git's config, as parterre's git reads it (the system's and the user's included),
/// says how `git pull` reconciles a diverged `main`.
fn told_how_to_pull(r: &TestRepo) -> bool {
    let git = parterre_core::git::Git::new(r.path());
    ["pull.rebase", "pull.ff", "branch.main.rebase"]
        .iter()
        .any(|key| git.query(&["config", "--get", key]).unwrap().is_some())
}

fn upstream(r: &TestRepo, branch: &str) -> Option<String> {
    let out = r.git(&[
        "for-each-ref",
        "--format=%(upstream:short)",
        &format!("refs/heads/{branch}"),
    ]);
    (!out.is_empty()).then_some(out)
}

/// `other` commits `file` with `text` on main and pushes it; returns the commit.
fn push_from_other(s: &mut Setup, file: &str, text: &str) -> String {
    s.other.write(file, text.as_bytes());
    let c = s.other.commit_all(&format!("other {file}"));
    s.other.git(&["push", "-q", "origin", "HEAD"]);
    c
}

#[test]
fn fetching_brings_every_remotes_commits_and_prunes_deleted_branches() {
    let mut s = setup();
    s.other.git(&["push", "-q", "origin", "main:doomed"]);
    s.work.git(&["fetch", "-q", "origin"]);
    s.other.git(&["push", "-q", "origin", "--delete", "doomed"]);
    let theirs = push_from_other(&mut s, "theirs", "theirs\n");
    let report = done(execute(&s.work, Action::Fetch, None));
    assert_eq!(commands(&report), ["git fetch --progress --all --prune"]);
    assert_eq!(rev(&s.work, "origin/main").to_hex(), theirs);
    assert!(
        s.work
            .git(&["branch", "-r"])
            .lines()
            .all(|l| !l.contains("doomed"))
    );
    // Only remote-tracking branches moved.
    assert_ne!(rev(&s.work, "main").to_hex(), theirs);
}

#[test]
fn fetching_without_a_remote_is_refused() {
    let mut r = TestRepo::new();
    r.commit("only");
    let (error, report) = failed(execute(&r, Action::Fetch, None));
    assert!(error.contains("no remote"), "{error}");
    assert!(report.steps.is_empty());
}

#[test]
fn a_failed_fetch_says_parterre_cannot_ask_for_credentials() {
    let s = setup();
    s.work.git(&[
        "remote",
        "set-url",
        "origin",
        "/nonexistent/parterre-remote",
    ]);
    let (error, _) = failed(execute(&s.work, Action::Fetch, None));
    assert!(error.contains("credential helper"), "{error}");
}

#[test]
fn fetching_streams_gits_output_to_the_live_view() {
    let mut s = setup();
    push_from_other(&mut s, "theirs", "theirs\n");
    let live = Live::default();
    let out = Branches::new(s.work.path())
        .with_live(live.clone())
        .execute(Action::Fetch, None, &CancelTree::default());
    done(out);
    let steps = live.steps();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].0, "git fetch --progress --all --prune");
    assert!(steps[0].1.contains("main"), "{:?}", steps[0].1);
}

#[test]
fn live_output_overwrites_a_line_ended_by_a_carriage_return() {
    let live = Live::default();
    live.start(&["fetch".to_owned()]);
    live.push(b"Counting: 1%\rCounting: 50%\rCounting: 100%, done.\r\nnext\n");
    assert_eq!(live.steps()[0].1, "Counting: 100%, done.\nnext\n");
}

#[test]
fn pulling_fetches_first_and_fast_forwards() {
    let mut s = setup();
    let theirs = push_from_other(&mut s, "theirs", "theirs\n");
    let report = done(execute(&s.work, pull(&s.work, None), None));
    assert_eq!(
        commands(&report),
        ["git fetch --progress --prune origin", "git pull --progress"]
    );
    assert_eq!(rev(&s.work, "main").to_hex(), theirs);
}

#[test]
fn pulling_with_nothing_new_only_fetches() {
    let s = setup();
    let report = done(execute(&s.work, pull(&s.work, None), None));
    assert_eq!(commands(&report), ["git fetch --progress --prune origin"]);
}

#[test]
fn pulling_a_diverged_branch_asks_how_when_git_isnt_told() {
    let mut s = setup();
    let theirs = push_from_other(&mut s, "theirs", "theirs\n");
    s.work.write("mine", b"mine\n");
    let mine = s.work.commit_all("mine");
    if told_how_to_pull(&s.work) {
        // Git for Windows' installer sets `pull.rebase` in the system config.
        done(execute(&s.work, pull(&s.work, None), None));
        return;
    }
    let diverged: Diverged = match execute(&s.work, pull(&s.work, None), None) {
        Outcome::Diverged(d) => *d,
        other => panic!("expected the question: {other:?}"),
    };
    assert_eq!((diverged.ahead, diverged.behind), (1, 1));
    assert_eq!(diverged.upstream, "origin/main");
    assert_eq!(
        parterre_core::branches::command_text(&diverged.command(Reconcile::Rebase)),
        "git pull --progress --rebase"
    );
    // Fetched, and nothing else.
    assert_eq!(rev(&s.work, "main").to_hex(), mine);
    assert_eq!(rev(&s.work, "origin/main").to_hex(), theirs);

    let report = done(execute(
        &s.work,
        pull(&s.work, Some(Reconcile::Rebase)),
        None,
    ));
    assert_eq!(
        commands(&report),
        [
            "git fetch --progress --prune origin",
            "git pull --progress --rebase"
        ]
    );
    assert_eq!(rev(&s.work, "main~1").to_hex(), theirs);
}

#[test]
fn pulling_a_diverged_branch_by_merge_makes_a_merge_commit() {
    let mut s = setup();
    let theirs = push_from_other(&mut s, "theirs", "theirs\n");
    s.work.write("mine", b"mine\n");
    let mine = s.work.commit_all("mine");
    let report = done(execute(
        &s.work,
        pull(&s.work, Some(Reconcile::Merge)),
        None,
    ));
    assert_eq!(
        commands(&report),
        [
            "git fetch --progress --prune origin",
            "git pull --progress --no-rebase"
        ]
    );
    assert_eq!(rev(&s.work, "main^1").to_hex(), mine);
    assert_eq!(rev(&s.work, "main^2").to_hex(), theirs);
}

#[test]
fn pulling_a_diverged_branch_asks_nothing_once_git_is_told() {
    for (key, value) in [
        ("pull.rebase", "true"),
        ("pull.ff", "false"),
        ("branch.main.rebase", "false"),
    ] {
        let mut s = setup();
        s.work.git(&["config", key, value]);
        let theirs = push_from_other(&mut s, "theirs", "theirs\n");
        s.work.write("mine", b"mine\n");
        s.work.commit_all("mine");
        let report = done(execute(&s.work, pull(&s.work, None), None));
        assert_eq!(
            commands(&report),
            ["git fetch --progress --prune origin", "git pull --progress"],
            "{key}"
        );
        let base = s.work.git(&["merge-base", "main", &theirs]);
        assert_eq!(base, theirs, "{key}: the pull took their commit in");
    }
}

#[test]
fn a_pull_that_conflicts_leaves_the_merge_in_progress_and_says_so() {
    let mut s = setup();
    push_from_other(&mut s, "base", "theirs\n");
    s.work.write("base", b"mine\n");
    s.work.commit_all("mine");
    let report = done(execute(
        &s.work,
        pull(&s.work, Some(Reconcile::Merge)),
        None,
    ));
    let attention = report.attention.expect("a stop needs attention");
    assert_eq!(attention.title, "Pull stopped on conflicts in 1 file");
    let catalog = Catalog::load(s.work.path()).unwrap();
    assert_eq!(catalog.stuck(), Some(Stuck::InProgress("a merge")));
}

#[test]
fn a_pull_that_conflicts_while_rebasing_leaves_the_rebase_in_progress() {
    let mut s = setup();
    push_from_other(&mut s, "base", "theirs\n");
    s.work.write("base", b"mine\n");
    s.work.commit_all("mine");
    let report = done(execute(
        &s.work,
        pull(&s.work, Some(Reconcile::Rebase)),
        None,
    ));
    assert!(report.attention.is_some());
    let catalog = Catalog::load(s.work.path()).unwrap();
    assert_eq!(catalog.stuck(), Some(Stuck::InProgress("a rebase")));
}

#[test]
fn a_pull_git_refuses_over_uncommitted_changes_fails_and_changes_nothing() {
    let mut s = setup();
    push_from_other(&mut s, "base", "theirs\n");
    s.work.write("base", b"uncommitted\n");
    let head = rev(&s.work, "main");
    let (error, _) = failed(execute(&s.work, pull(&s.work, None), None));
    assert!(!error.is_empty());
    assert_eq!(rev(&s.work, "main"), head);
    assert_eq!(
        common::read_text(&s.work.path().join("base")),
        "uncommitted\n"
    );
}

#[test]
fn pulling_a_moved_or_stuck_branch_is_refused() {
    let mut s = setup();
    let stale = pull(&s.work, None);
    s.work.commit("moved");
    let (error, report) = failed(execute(&s.work, stale, None));
    assert!(error.contains("moved"), "{error}");
    assert!(report.steps.is_empty());

    s.work.git(&["checkout", "-q", "-b", "side"]);
    s.work.write("base", b"side\n");
    s.work.commit_all("side");
    s.work.checkout("main");
    s.work.write("base", b"main\n");
    s.work.commit_all("main");
    let _ = std::process::Command::new("git")
        .current_dir(s.work.path())
        .args(["merge", "side"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output();
    let (error, report) = failed(execute(&s.work, pull(&s.work, None), None));
    assert!(error.contains("in progress"), "{error}");
    assert!(report.steps.is_empty());
}

#[test]
fn the_first_push_of_a_branch_with_no_upstream_sets_it() {
    let mut s = setup();
    s.work.branch("feature");
    s.work.commit("feature");
    let report = done(execute(&s.work, push(&s.work, "feature"), None));
    assert_eq!(commands(&report), ["git push --progress -u origin feature"]);
    assert_eq!(
        on_origin(&s, "feature"),
        Some(rev(&s.work, "feature").to_hex())
    );
    assert_eq!(
        upstream(&s.work, "feature").as_deref(),
        Some("origin/feature")
    );
}

#[test]
fn pushing_leaves_an_upstream_of_another_name_alone() {
    let mut s = setup();
    s.work
        .git(&["checkout", "-q", "-b", "feature", "--track", "origin/main"]);
    s.work.commit("feature");
    let report = done(execute(&s.work, push(&s.work, "feature"), None));
    assert_eq!(commands(&report), ["git push --progress origin feature"]);
    assert_eq!(
        on_origin(&s, "feature"),
        Some(rev(&s.work, "feature").to_hex())
    );
    assert_eq!(on_origin(&s, "main"), Some(rev(&s.work, "main").to_hex()));
    assert_eq!(upstream(&s.work, "feature").as_deref(), Some("origin/main"));
}

#[test]
fn pushing_to_an_upstream_never_pushed_creates_it_and_keeps_the_config() {
    let mut s = setup();
    s.work.branch("feature");
    s.work.commit("feature");
    s.work.git(&["config", "branch.feature.remote", "origin"]);
    s.work
        .git(&["config", "branch.feature.merge", "refs/heads/feature"]);
    let report = done(execute(&s.work, push(&s.work, "feature"), None));
    assert_eq!(commands(&report), ["git push --progress origin feature"]);
    assert_eq!(
        on_origin(&s, "feature"),
        Some(rev(&s.work, "feature").to_hex())
    );
    assert_eq!(
        upstream(&s.work, "feature").as_deref(),
        Some("origin/feature")
    );
}

#[test]
fn a_push_ahead_of_the_remote_goes_in_one_click() {
    let mut s = setup();
    let mine = s.work.commit("mine");
    let report = done(execute(&s.work, push(&s.work, "main"), None));
    assert_eq!(commands(&report), ["git push --progress origin main"]);
    assert_eq!(on_origin(&s, "main"), Some(mine));
}

#[test]
fn a_rejected_push_fetches_and_says_the_remote_has_commits_you_dont() {
    let mut s = setup();
    // origin/main is stale here: by the last fetch, main is just ahead.
    let theirs = push_from_other(&mut s, "theirs", "theirs\n");
    s.work.commit("mine");
    let (error, report) = failed(execute(&s.work, push(&s.work, "main"), None));
    assert_eq!(error, "origin/main has commits you don't have.");
    assert_eq!(
        commands(&report),
        [
            "git push --progress origin main",
            "git fetch --progress --prune origin"
        ]
    );
    assert_eq!(on_origin(&s, "main"), Some(theirs.clone()));
    assert_eq!(rev(&s.work, "origin/main").to_hex(), theirs);
}

/// `feature` pushed with two commits, then `work` rewrites both onto a new `main` commit.
fn rebased_feature() -> Setup {
    let mut s = setup();
    s.work.branch("feature");
    s.work.write("one", b"one\n");
    s.work.commit_all("one");
    s.work.write("two", b"two\n");
    s.work.commit_all("two");
    s.work.git(&["push", "-q", "-u", "origin", "feature"]);
    s.work.checkout("main");
    s.work.write("main", b"main\n");
    s.work.commit_all("main moved");
    s.work.checkout("feature");
    s.work.git(&["rebase", "-q", "main"]);
    s
}

#[test]
fn a_force_push_that_only_replaces_rebased_commits_is_a_confirmation() {
    let s = rebased_feature();
    let w = warning(execute(&s.work, push(&s.work, "feature"), None));
    assert!(w.is_confirmation(), "{w:?}");
    assert!(w.commits.is_empty());
    assert_eq!(w.replaced.len(), 2);
    assert_eq!(
        w.commands
            .iter()
            .map(|c| parterre_core::branches::command_text(c))
            .collect::<Vec<_>>(),
        [
            "git push --progress --force-with-lease=refs/heads/feature --force-if-includes origin feature"
        ]
    );
    let report = done(execute(&s.work, w.action.clone(), Some(&w)));
    assert_eq!(report.steps.len(), 1);
    assert_eq!(
        on_origin(&s, "feature"),
        Some(rev(&s.work, "feature").to_hex())
    );
}

#[test]
fn a_force_push_that_loses_someone_elses_commit_warns_with_it() {
    let mut s = rebased_feature();
    // `other` adds to feature, and `work` fetches it but never takes it in.
    s.other.git(&["fetch", "-q", "origin"]);
    s.other.git(&[
        "checkout",
        "-q",
        "-b",
        "feature",
        "--track",
        "origin/feature",
    ]);
    s.other.write("three", b"three\n");
    let three = s.other.commit_all("three");
    s.other.git(&["push", "-q", "origin", "feature"]);
    s.work.git(&["fetch", "-q", "origin"]);
    let w = warning(execute(&s.work, push(&s.work, "feature"), None));
    assert!(!w.is_confirmation());
    assert_eq!(w.commits, vec![oid(&three)]);
    assert_eq!(w.replaced.len(), 2);
    // Nothing pushed before the answer.
    assert_eq!(on_origin(&s, "feature"), Some(three.clone()));
}

#[test]
fn an_approved_force_push_whose_remote_moved_unseen_is_stopped_by_the_lease() {
    let mut s = rebased_feature();
    let w = warning(execute(&s.work, push(&s.work, "feature"), None));
    // After the answer, `other` pushes to feature; `work` hasn't fetched it.
    s.other.git(&["fetch", "-q", "origin"]);
    s.other.git(&[
        "checkout",
        "-q",
        "-b",
        "feature",
        "--track",
        "origin/feature",
    ]);
    s.other.write("three", b"three\n");
    let three = s.other.commit_all("three");
    s.other.git(&["push", "-q", "origin", "feature"]);
    let (error, report) = failed(execute(&s.work, w.action.clone(), Some(&w)));
    assert_eq!(error, "origin/feature has commits you don't have.");
    assert_eq!(report.steps.len(), 2, "the push, then a fetch");
    assert_eq!(on_origin(&s, "feature"), Some(three));
}

#[test]
fn an_approval_from_before_a_fetch_asks_again_with_whats_lost_now() {
    let mut s = rebased_feature();
    let w = warning(execute(&s.work, push(&s.work, "feature"), None));
    s.other.git(&["fetch", "-q", "origin"]);
    s.other.git(&[
        "checkout",
        "-q",
        "-b",
        "feature",
        "--track",
        "origin/feature",
    ]);
    s.other.write("three", b"three\n");
    let three = s.other.commit_all("three");
    s.other.git(&["push", "-q", "origin", "feature"]);
    s.work.git(&["fetch", "-q", "origin"]);
    let again = warning(execute(&s.work, w.action.clone(), Some(&w)));
    assert_eq!(again.commits, vec![oid(&three)]);
    assert_eq!(on_origin(&s, "feature"), Some(three));
}

#[test]
fn a_force_push_with_no_upstream_sets_it_too() {
    let s = rebased_feature();
    s.work.git(&["branch", "--unset-upstream", "feature"]);
    let w = warning(execute(&s.work, push(&s.work, "feature"), None));
    let report = done(execute(&s.work, w.action.clone(), Some(&w)));
    assert_eq!(
        commands(&report),
        [
            "git push --progress -u --force-with-lease=refs/heads/feature --force-if-includes origin feature"
        ]
    );
    assert_eq!(
        upstream(&s.work, "feature").as_deref(),
        Some("origin/feature")
    );
}

#[test]
fn a_moved_branch_is_not_pushed() {
    let mut s = setup();
    let stale = push(&s.work, "main");
    s.work.commit("moved");
    let (error, report) = failed(execute(&s.work, stale, None));
    assert!(error.contains("moved"), "{error}");
    assert!(report.steps.is_empty());
}

#[test]
fn each_remote_says_whether_a_push_would_send_anything() {
    let mut s = setup();
    let url = s.origin.path().to_str().unwrap().to_owned();
    s.work.git(&["remote", "add", "backup", &url]);
    s.work.git(&["fetch", "-q", "backup"]);
    let states = |r: &TestRepo, branch: &str| {
        let repo = r.load();
        let catalog = Catalog::load(r.path()).unwrap();
        remote::push_targets(&repo, &catalog, branch)
    };
    let both = |state| vec![("backup".to_owned(), state), ("origin".to_owned(), state)];
    assert_eq!(states(&s.work, "main"), both(PushState::UpToDate));
    s.work.commit("mine");
    assert_eq!(states(&s.work, "main"), both(PushState::Ahead));
    s.work.git(&["reset", "-q", "--hard", "HEAD~1"]);
    push_from_other(&mut s, "theirs", "theirs\n");
    s.work.git(&["fetch", "-q", "origin"]);
    // Behind origin: nothing to push there; backup wasn't fetched.
    assert_eq!(
        states(&s.work, "main"),
        vec![
            ("backup".to_owned(), PushState::UpToDate),
            ("origin".to_owned(), PushState::UpToDate)
        ]
    );
    s.work.write("base", b"diverged\n");
    s.work.commit_all("diverged");
    assert_eq!(
        states(&s.work, "main"),
        vec![
            ("backup".to_owned(), PushState::Ahead),
            ("origin".to_owned(), PushState::Force)
        ]
    );
    s.work.branch("feature");
    assert_eq!(states(&s.work, "feature"), both(PushState::New));
}

#[test]
fn pull_is_offered_on_the_open_worktrees_branch_with_an_upstream_only() {
    let s = setup();
    let catalog = Catalog::load(s.work.path()).unwrap();
    let head = rev(&s.work, "main");
    assert_eq!(
        remote::pull_offered(&catalog, head).map(|b| b.name.as_str()),
        Some("main")
    );
    s.work.git(&["branch", "--unset-upstream"]);
    let catalog = Catalog::load(s.work.path()).unwrap();
    assert!(remote::pull_offered(&catalog, head).is_none());
}

#[test]
fn setting_an_upstream() {
    let s = setup();
    s.work.git(&["branch", "feature"]);
    let set = Action::SetUpstream(Box::new(SetUpstream {
        branch: "feature".into(),
        upstream: "origin/main".into(),
    }));
    let report = done(execute(&s.work, set, None));
    assert_eq!(
        commands(&report),
        ["git branch --set-upstream-to=refs/remotes/origin/main -- feature"]
    );
    assert_eq!(upstream(&s.work, "feature").as_deref(), Some("origin/main"));
    let missing = Action::SetUpstream(Box::new(SetUpstream {
        branch: "feature".into(),
        upstream: "origin/nowhere".into(),
    }));
    let (error, report) = failed(execute(&s.work, missing, None));
    assert!(error.contains("no remote-tracking branch"), "{error}");
    assert!(report.steps.is_empty());
}

#[test]
fn a_branch_behind_its_remote_has_nothing_to_push() {
    let mut s = setup();
    push_from_other(&mut s, "theirs", "theirs\n");
    s.work.git(&["fetch", "-q", "origin"]);
    let (error, report) = failed(execute(&s.work, push(&s.work, "main"), None));
    assert!(error.contains("nothing to push"), "{error}");
    assert!(report.steps.is_empty());
}

#[test]
fn forcing_past_a_commit_the_branch_dropped_loses_it_once_approved() {
    let mut s = setup();
    s.work.branch("feature");
    s.work.write("one", b"one\n");
    s.work.commit_all("one");
    s.work.write("two", b"two\n");
    let two = s.work.commit_all("two");
    s.work.git(&["push", "-q", "-u", "origin", "feature"]);
    // `two` is dropped: the branch had it, so git's include check lets the push through.
    s.work.git(&["reset", "-q", "--hard", "HEAD~1"]);
    s.work.write("three", b"three\n");
    s.work.commit_all("three");
    let w = warning(execute(&s.work, push(&s.work, "feature"), None));
    assert_eq!(w.commits, vec![oid(&two)]);
    assert!(w.replaced.is_empty());
    done(execute(&s.work, w.action.clone(), Some(&w)));
    assert_eq!(
        on_origin(&s, "feature"),
        Some(rev(&s.work, "feature").to_hex())
    );
}

#[test]
fn git_wont_force_past_commits_the_branch_never_had_and_says_why() {
    let mut s = rebased_feature();
    s.other.git(&["fetch", "-q", "origin"]);
    s.other.git(&[
        "checkout",
        "-q",
        "-b",
        "feature",
        "--track",
        "origin/feature",
    ]);
    s.other.write("three", b"three\n");
    let three = s.other.commit_all("three");
    s.other.git(&["push", "-q", "origin", "feature"]);
    s.work.git(&["fetch", "-q", "origin"]);
    let w = warning(execute(&s.work, push(&s.work, "feature"), None));
    let (error, report) = failed(execute(&s.work, w.action.clone(), Some(&w)));
    assert!(
        error.starts_with("origin/feature has commits feature never had"),
        "{error}"
    );
    assert_eq!(report.steps.len(), 2, "the push, then a fetch");
    assert_eq!(on_origin(&s, "feature"), Some(three));
}

/// A pre-push hook that waits a minute, to cancel the push under.
#[cfg(unix)]
#[test]
fn a_cancelled_push_stops_git_and_pushes_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let mut s = setup();
    let hook = s.work.path().join(".git/hooks/pre-push");
    std::fs::write(&hook, "#!/bin/sh\necho checking\nsleep 60\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let before = on_origin(&s, "main");
    s.work.commit("mine");
    let cancel = CancelTree::default();
    let live = Live::default();
    let started = std::time::Instant::now();
    let (canceller, watched) = (cancel.clone(), live.clone());
    std::thread::spawn(move || {
        while !watched
            .steps()
            .first()
            .is_some_and(|s| s.1.contains("checking"))
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        canceller.cancel();
    });
    let out =
        Branches::new(s.work.path())
            .with_live(live)
            .execute(push(&s.work, "main"), None, &cancel);
    let (error, _) = failed(out);
    assert_eq!(error, "Operation cancelled.");
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
    assert_eq!(on_origin(&s, "main"), before);
}

fn delete_remote(r: &TestRepo, names: &[&str]) -> Action {
    Action::DeleteRemoteBranches(
        names
            .iter()
            .map(|name| RemoteBranchTip {
                remote: "origin".into(),
                branch: (*name).into(),
                tip: rev(r, &format!("origin/{name}")),
            })
            .collect(),
    )
}

/// `feature`, pushed with one commit of its own.
fn pushed_feature() -> Setup {
    let mut s = setup();
    s.work.branch("feature");
    s.work.commit("feature");
    s.work.git(&["push", "-q", "-u", "origin", "feature"]);
    s.work.checkout("main");
    s
}

#[test]
fn deleting_a_remote_branch_always_asks_first_then_deletes_it_for_everyone() {
    let s = pushed_feature();
    let action = delete_remote(&s.work, &["feature"]);
    let w = warning(execute(&s.work, action, None));
    // The local branch still has its commit: nothing is lost, but it asks.
    assert!(w.is_confirmation());
    assert!(w.commits.is_empty());
    let tip = rev(&s.work, "origin/feature").to_hex();
    assert_eq!(
        w.commands
            .iter()
            .map(|c| parterre_core::branches::command_text(c))
            .collect::<Vec<_>>(),
        [format!(
            "git push --progress --force-with-lease=refs/heads/feature:{tip} origin --delete feature"
        )]
    );
    let report = done(execute(&s.work, w.action.clone(), Some(&w)));
    assert_eq!(report.steps.len(), 1);
    assert_eq!(on_origin(&s, "feature"), None);
    assert!(!s.work.git(&["branch", "-r"]).contains("origin/feature"));
    // The local branch is left, its upstream gone.
    assert_eq!(rev(&s.work, "feature").to_hex(), tip);
}

#[test]
fn deleting_a_remote_branch_lists_the_commits_only_it_has() {
    let mut s = pushed_feature();
    s.work.git(&["branch", "-D", "feature"]);
    let only = rev(&s.work, "origin/feature");
    let w = warning(execute(&s.work, delete_remote(&s.work, &["feature"]), None));
    assert!(!w.is_confirmation());
    assert_eq!(w.commits, vec![only]);
    assert_eq!(w.deletions.len(), 1);
    assert_eq!(w.deletions[0].name, "origin/feature");
    done(execute(&s.work, w.action.clone(), Some(&w)));
    assert_eq!(on_origin(&s, "feature"), None);
    let _ = &mut s;
}

#[test]
fn deleting_several_remote_branches_counts_shared_commits_once() {
    let s = pushed_feature();
    s.work.git(&["push", "-q", "origin", "feature:copy"]);
    s.work.git(&["fetch", "-q", "origin"]);
    s.work.git(&["branch", "-D", "feature"]);
    let w = warning(execute(
        &s.work,
        delete_remote(&s.work, &["copy", "feature"]),
        None,
    ));
    assert_eq!(w.commits.len(), 1, "the commit both have, once");
    assert_eq!(w.commands.len(), 2);
    done(execute(&s.work, w.action.clone(), Some(&w)));
    assert_eq!(on_origin(&s, "feature"), None);
    assert_eq!(on_origin(&s, "copy"), None);
}

#[test]
fn a_remote_branch_that_moved_since_it_was_offered_is_not_deleted() {
    let mut s = pushed_feature();
    let stale = delete_remote(&s.work, &["feature"]);
    s.work.checkout("feature");
    s.work.commit("more");
    s.work.git(&["push", "-q", "origin", "feature"]);
    let (error, report) = failed(execute(&s.work, stale, None));
    assert!(error.contains("moved"), "{error}");
    assert!(report.steps.is_empty());
    assert!(on_origin(&s, "feature").is_some());
}

#[test]
fn a_remote_branch_that_moved_on_the_remote_unseen_is_kept_and_a_fetch_suggested() {
    let mut s = pushed_feature();
    let w = warning(execute(&s.work, delete_remote(&s.work, &["feature"]), None));
    s.other.git(&["fetch", "-q", "origin"]);
    s.other.git(&[
        "checkout",
        "-q",
        "-b",
        "feature",
        "--track",
        "origin/feature",
    ]);
    let theirs = s.other.commit("theirs");
    s.other.git(&["push", "-q", "origin", "feature"]);
    let (error, report) = failed(execute(&s.work, w.action.clone(), Some(&w)));
    assert_eq!(
        error,
        "origin/feature moved on origin since the last fetch. Fetch, and look again."
    );
    assert!(report.suggest_fetch);
    assert_eq!(on_origin(&s, "feature"), Some(theirs));
}

#[test]
fn an_approval_from_before_the_commits_changed_asks_again() {
    let s = pushed_feature();
    let w = warning(execute(&s.work, delete_remote(&s.work, &["feature"]), None));
    s.work.git(&["branch", "-D", "feature"]);
    let again = warning(execute(&s.work, w.action.clone(), Some(&w)));
    assert_eq!(again.commits.len(), 1);
    assert!(on_origin(&s, "feature").is_some());
}

#[test]
fn each_remotes_default_branch_is_known() {
    let s = setup();
    assert!(
        Catalog::load(s.work.path())
            .unwrap()
            .remote_defaults
            .is_empty()
    );
    s.work.git(&["remote", "set-head", "origin", "main"]);
    let catalog = Catalog::load(s.work.path()).unwrap();
    assert_eq!(catalog.remote_defaults, ["origin/main"]);
    assert!(catalog.remotes.iter().all(|r| !r.name.ends_with("/HEAD")));
}

#[test]
fn a_deletion_that_stops_partway_says_which_went() {
    let mut s = pushed_feature();
    s.work.git(&["push", "-q", "origin", "feature:copy"]);
    s.work.git(&["fetch", "-q", "origin"]);
    let w = warning(execute(
        &s.work,
        delete_remote(&s.work, &["copy", "feature"]),
        None,
    ));
    s.other.git(&["fetch", "-q", "origin"]);
    s.other.git(&[
        "checkout",
        "-q",
        "-b",
        "feature",
        "--track",
        "origin/feature",
    ]);
    s.other.commit("theirs");
    s.other.git(&["push", "-q", "origin", "feature"]);
    let (error, _) = failed(execute(&s.work, w.action.clone(), Some(&w)));
    assert_eq!(
        error,
        "origin/feature moved on origin since the last fetch. Fetch, and look again. \
         origin/copy was deleted."
    );
    assert_eq!(on_origin(&s, "copy"), None);
    assert!(on_origin(&s, "feature").is_some());
}

#[test]
fn a_remotes_head_naming_the_branch_doesnt_keep_its_commits() {
    let s = pushed_feature();
    s.work.git(&["remote", "set-head", "origin", "feature"]);
    s.work.git(&["branch", "-D", "feature"]);
    let w = warning(execute(&s.work, delete_remote(&s.work, &["feature"]), None));
    assert_eq!(w.commits, vec![rev(&s.work, "origin/feature")]);
}
