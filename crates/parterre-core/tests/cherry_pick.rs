//! Cherry-picking onto the open worktree's branch, against real, disposable repositories: when
//! it's offered, what the confirmation leaves out (checked against what `git cherry-pick` then
//! does), the order the commits go on in, and every way it can stop, refuse or keep changes
//! aside.
mod common;

use common::TestRepo;
use parterre_core::Oid;
use parterre_core::branches::{Action, Branches, Catalog, Outcome, Report, Stuck};
use parterre_core::cherry_pick::{self, CherryPick, Picks, Preview, Skipped};
use parterre_util::CancelTree;

fn oid(s: &str) -> Oid {
    Oid::from_hex(s).unwrap()
}

fn rev(r: &TestRepo, rev: &str) -> Oid {
    oid(&r.git(&["rev-parse", rev]))
}

fn lacking(r: &TestRepo, of: &str) -> Preview {
    Preview::load(r.path(), &Picks::Lacking(rev(r, of)), Some(of.into())).unwrap()
}

fn chosen(r: &TestRepo, commits: &[&str]) -> Preview {
    let commits = commits.iter().map(|c| oid(c)).collect();
    Preview::load(r.path(), &Picks::Chosen(commits), None).unwrap()
}

fn execute(r: &TestRepo, pick: CherryPick) -> Outcome {
    Branches::new(r.path()).execute(
        Action::CherryPick(Box::new(pick)),
        None,
        &CancelTree::default(),
    )
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

/// The subjects of `range`, newest first.
fn subjects(r: &TestRepo, range: &str) -> Vec<String> {
    r.git(&["log", "--format=%s", range])
        .lines()
        .map(str::to_owned)
        .collect()
}

/// `main` with a commit of its own, and `up` with three from a shared base: `one` and `three`
/// touch files of their own, `two` too.
fn diverged() -> (TestRepo, [String; 3]) {
    let mut r = TestRepo::new();
    r.write("file", b"base\n");
    r.commit_all("base");
    r.git(&["branch", "up"]);
    r.write("mine", b"mine\n");
    r.commit_all("mine");
    r.checkout("up");
    let commits = ["one", "two", "three"].map(|name| {
        r.write(name, format!("{name}\n").as_bytes());
        r.commit_all(name)
    });
    r.checkout("main");
    (r, commits)
}

#[test]
fn it_is_offered_where_a_commit_isnt_on_the_branch_yet() {
    let (r, [one, ..]) = diverged();
    let repo = r.load();
    let catalog = Catalog::load(r.path()).unwrap();
    let offered = |c: &str| cherry_pick::offered(&repo, &catalog, rev(&r, c));
    assert_eq!(offered("up"), Some("main"));
    // On main already, or main itself.
    assert_eq!(offered("main~1"), None);
    assert_eq!(offered("main"), None);
    // The log's selection: one commit not on main is enough.
    let some = [rev(&r, "main~1"), oid(&one)];
    assert_eq!(
        cherry_pick::offered_chosen(&repo, &catalog, &some),
        Some("main")
    );
    let none = [rev(&r, "main~1"), rev(&r, "main")];
    assert_eq!(cherry_pick::offered_chosen(&repo, &catalog, &none), None);
    // Also when main could fast-forward to it: the user may want to leave some out.
    r.git(&["branch", "ahead", "main"]);
    r.checkout("ahead");
    r.write("more", b"more\n");
    r.git(&["add", "more"]);
    r.git(&["commit", "-q", "-m", "more"]);
    r.checkout("main");
    let repo = r.load();
    let catalog = Catalog::load(r.path()).unwrap();
    assert_eq!(
        cherry_pick::offered(&repo, &catalog, rev(&r, "ahead")),
        Some("main")
    );
    // A detached HEAD has no branch.
    r.git(&["checkout", "-q", "--detach", "main"]);
    let catalog = Catalog::load(r.path()).unwrap();
    assert_eq!(cherry_pick::offered(&repo, &catalog, rev(&r, "up")), None);
}

#[test]
fn the_graphs_picks_are_what_main_lacks_and_git_agrees() {
    let mut r = TestRepo::new();
    r.write("base", b"base\n");
    r.commit_all("base");
    r.git(&["branch", "up"]);
    r.write("fix", b"fixed\n");
    r.commit_all("fix");
    r.checkout("up");
    // On up: the same fix (left out), a feature, and a merged side branch (the merge left out,
    // its commit picked).
    r.write("fix", b"fixed\n");
    let same = r.commit_all("the same fix");
    r.write("feature", b"feature\n");
    let feature = r.commit_all("feature");
    r.branch("side");
    r.write("side", b"side\n");
    let side = r.commit_all("side");
    r.checkout("up");
    let merge = r.merge("side", "Merge side");
    r.checkout("main");

    let p = lacking(&r, "up");
    assert_eq!(p.branch, "main");
    assert!(!p.dirty);
    assert_eq!(p.listed.len(), 4);
    assert_eq!(p.listed[0], oid(&merge), "newest first");
    assert_eq!(p.skipped(oid(&same)), Some(Skipped::AlreadyThere));
    assert_eq!(p.skipped(oid(&merge)), Some(Skipped::Merge));
    assert_eq!(p.skipped(oid(&feature)), None);
    assert_eq!(p.picked(oid(&same)), None);
    assert_eq!(p.picked(oid(&feature)), Some(true));
    // Oldest first, explicitly.
    assert_eq!(p.commits(), [oid(&feature), oid(&side)]);

    let pick = p.cherry_pick(false, false);
    assert_eq!(pick.name, "up");
    assert_eq!(
        cherry_pick::command(&pick),
        ["cherry-pick".to_owned(), feature.clone(), side.clone()]
    );
    let report = done(execute(&r, pick));
    assert!(report.attention.is_none());
    assert_eq!(subjects(&r, "main~2..main"), ["side", "feature"]);
    assert_eq!(subjects(&r, "main~3..main~2"), ["fix"]);
}

#[test]
fn chosen_commits_skip_those_between_and_go_on_oldest_first() {
    let (r, [one, two, three]) = diverged();
    // As the log lists them: newest first.
    let p = chosen(&r, &[&three, &one]);
    assert_eq!(p.listed, [oid(&three), oid(&one)]);
    assert_eq!(p.commits(), [oid(&one), oid(&three)]);
    let pick = p.cherry_pick(false, false);
    assert_eq!(pick.name, "2 commits");
    done(execute(&r, pick));
    assert_eq!(subjects(&r, "main~3..main"), ["three", "one", "mine"]);
    assert!(!r.path().join("two").exists());

    // Dropping one leaves the other; dropping both leaves nothing to do.
    let mut p = chosen(&r, &[&two]);
    assert_eq!(p.cherry_pick(false, false).name, &two[..7]);
    p.set_picked(oid(&two), false);
    assert_eq!(p.blocked(), Some("Pick at least one commit."));
    p.set_picked(oid(&two), true);
    assert_eq!(p.blocked(), None);
    // Commits already on main are left out, not picked again.
    let mut p = chosen(&r, &[&two, &r.git(&["rev-parse", "main~1"])]);
    assert_eq!(p.skipped(rev(&r, "main~1")), Some(Skipped::OnBranch));
    p.set_picked(rev(&r, "main~1"), true);
    assert_eq!(p.commits(), [oid(&two)]);
}

#[test]
fn commits_on_separate_lines_go_on_bottom_to_top_as_listed() {
    let mut r = TestRepo::new();
    r.write("file", b"base\n");
    r.commit_all("base");
    r.git(&["branch", "x"]);
    r.git(&["branch", "y"]);
    r.write("mine", b"mine\n");
    r.commit_all("mine");
    r.checkout("y");
    r.write("y", b"y\n");
    let y = r.commit_all("on y");
    r.checkout("x");
    r.write("x", b"x\n");
    let x = r.commit_all("on x");
    r.checkout("main");
    // x is newer, so the log lists it first: y goes on first.
    let p = chosen(&r, &[&x, &y]);
    done(execute(&r, p.cherry_pick(false, false)));
    assert_eq!(subjects(&r, "main~2..main"), ["on x", "on y"]);
}

#[test]
fn x_records_where_each_commit_came_from() {
    let (r, [one, ..]) = diverged();
    let pick = chosen(&r, &[&one]).cherry_pick(true, false);
    assert_eq!(cherry_pick::command(&pick), ["cherry-pick", "-x", &one]);
    done(execute(&r, pick));
    let message = r.git(&["log", "-1", "--format=%B", "main"]);
    assert!(
        message.contains(&format!("(cherry picked from commit {one})")),
        "{message}"
    );
}

/// `main` and `up` both change `file`; `up` has a clean commit before and after its change.
fn conflicting() -> (TestRepo, [String; 3]) {
    let mut r = TestRepo::new();
    r.write("file", b"base\n");
    r.commit_all("base");
    r.git(&["branch", "up"]);
    r.write("file", b"mine\n");
    r.commit_all("mine");
    r.checkout("up");
    r.write("before", b"before\n");
    let before = r.commit_all("before");
    r.write("file", b"theirs\n");
    let theirs = r.commit_all("theirs");
    r.write("after", b"after\n");
    let after = r.commit_all("after");
    r.checkout("main");
    (r, [before, theirs, after])
}

fn picking(r: &TestRepo) -> parterre_core::branches::Picking {
    let catalog = Catalog::load(r.path()).unwrap();
    catalog
        .worktrees
        .iter()
        .find(|w| w.open)
        .and_then(|w| w.picking.clone())
        .expect("a cherry-pick stopped")
}

#[test]
fn a_conflict_midway_stops_it_and_says_where() {
    let (r, [before, theirs, _]) = conflicting();
    let mine = rev(&r, "main");
    let report = done(execute(&r, lacking(&r, "up").cherry_pick(false, false)));
    let attention = report.attention.expect("an orange notice");
    assert_eq!(
        attention.title,
        "Cherry-pick stopped on conflicts in 1 file"
    );
    assert_eq!(
        attention.message,
        "Finish or abort it with git, or go to another worktree."
    );
    let catalog = Catalog::load(r.path()).unwrap();
    assert_eq!(catalog.stuck(), Some(Stuck::InProgress("a cherry-pick")));
    assert_eq!(catalog.conflicted, ["file"]);
    let p = picking(&r);
    assert_eq!(p.commit, oid(&theirs));
    assert_eq!((p.done, p.total), (2, 3));
    // The first went on.
    assert_eq!(rev(&r, "main~1"), mine);
    assert_eq!(subjects(&r, "main~1..main"), ["before"]);
    assert_ne!(rev(&r, "main"), oid(&before));

    // Nothing more starts there.
    let refused = Preview::load(r.path(), &Picks::Lacking(rev(&r, "up")), None)
        .unwrap_err()
        .to_string();
    assert_eq!(refused, "A cherry-pick is in progress in this worktree.");
}

#[test]
fn a_single_commit_that_conflicts_is_one_of_one() {
    let (r, [_, theirs, _]) = conflicting();
    done(execute(
        &r,
        chosen(&r, &[&theirs]).cherry_pick(false, false),
    ));
    let p = picking(&r);
    assert_eq!((p.commit, p.done, p.total), (oid(&theirs), 1, 1));
}

#[test]
fn a_pick_that_turns_out_empty_stops_as_git_does() {
    let mut r = TestRepo::new();
    r.write("file", b"base\n");
    r.commit_all("base");
    r.git(&["branch", "up"]);
    r.write("file", b"done\n");
    r.commit_all("straight there");
    r.checkout("up");
    r.write("file", b"halfway\n");
    r.commit_all("halfway");
    r.write("file", b"done\n");
    let rest = r.commit_all("the rest of the way");
    r.checkout("main");
    // A different change, so not left out: it only turns out empty on main.
    let p = chosen(&r, &[&rest]);
    assert_eq!(p.skipped(oid(&rest)), None);
    let report = done(execute(&r, p.cherry_pick(false, false)));
    assert_eq!(
        report.attention.expect("an orange notice").title,
        "Cherry-pick stopped"
    );
    let catalog = Catalog::load(r.path()).unwrap();
    assert_eq!(catalog.stuck(), Some(Stuck::InProgress("a cherry-pick")));
}

#[test]
fn uncommitted_changes_git_refuses_to_pick_over_are_left_alone() {
    let (r, [_, theirs, _]) = conflicting();
    r.write("file", b"my edit\n");
    let p = chosen(&r, &[&theirs]);
    assert!(p.dirty);
    let tip = rev(&r, "main");
    let error = failed(execute(&r, p.cherry_pick(false, false)));
    assert!(!error.is_empty());
    assert_eq!(rev(&r, "main"), tip);
    assert_eq!(common::read_text(&r.path().join("file")), "my edit\n");
    let catalog = Catalog::load(r.path()).unwrap();
    assert_eq!(catalog.stuck(), None);
}

#[test]
fn stash_changes_sets_them_aside_and_puts_them_back() {
    let (r, [one, ..]) = diverged();
    r.write("mine", b"edited\n");
    // Untracked files stay where they are.
    r.write("untracked", b"new\n");
    let p = chosen(&r, &[&one]);
    let pick = p.cherry_pick(false, true);
    assert_eq!(
        cherry_pick::commands(&pick),
        [
            vec!["stash", "push", "-m", "parterre: before cherry-pick"],
            vec!["cherry-pick", &one],
            vec!["stash", "pop"],
        ]
    );
    let report = done(execute(&r, pick));
    assert!(report.attention.is_none());
    assert_eq!(subjects(&r, "main~1..main"), ["one"]);
    assert_eq!(common::read_text(&r.path().join("mine")), "edited\n");
    assert_eq!(common::read_text(&r.path().join("untracked")), "new\n");
    assert_eq!(r.git(&["stash", "list"]), "");
}

#[test]
fn stash_changes_with_nothing_to_stash_pops_nothing() {
    let (r, [one, two, _]) = diverged();
    // Someone else's stash entry, which must stay.
    r.write("mine", b"theirs to keep\n");
    r.git(&["stash", "push", "-q", "-m", "keep me"]);
    let p = chosen(&r, &[&one]);
    assert!(!p.dirty);
    // Not dirty: the box isn't offered, and asking for it anyway doesn't stash.
    assert_eq!(cherry_pick::commands(&p.cherry_pick(false, true)).len(), 1);
    let mut pick = chosen(&r, &[&two]).cherry_pick(false, false);
    pick.stash = true;
    done(execute(&r, pick));
    assert_eq!(r.git(&["stash", "list"]).lines().count(), 1);
    assert_eq!(common::read_text(&r.path().join("mine")), "mine\n");
}

#[test]
fn a_stop_keeps_the_stash_and_says_so() {
    let (r, [_, theirs, _]) = conflicting();
    r.write("before", b"my own\n");
    r.git(&["add", "before"]);
    let report = done(execute(&r, chosen(&r, &[&theirs]).cherry_pick(false, true)));
    let attention = report.attention.expect("an orange notice");
    assert_eq!(
        attention.title,
        "Cherry-pick stopped on conflicts in 1 file"
    );
    assert!(
        attention.message.contains("stash@{0}"),
        "{}",
        attention.message
    );
    assert_eq!(r.git(&["stash", "list"]).lines().count(), 1);
}

#[test]
fn changes_the_stash_cant_put_back_stay_in_it() {
    let (r, [one, ..]) = diverged();
    // up's one adds the file the uncommitted change is to.
    r.write("one", b"my one\n");
    r.git(&["add", "one"]);
    let p = chosen(&r, &[&one]);
    assert!(p.dirty);
    let report = done(execute(&r, p.cherry_pick(false, true)));
    let attention = report.attention.expect("an orange notice");
    assert_eq!(
        attention.title,
        format!("Cherry-picked {} onto main", &one[..7])
    );
    assert_eq!(
        attention.message,
        "Putting your changes back conflicted in 1 file. Resolve it with git; your changes \
         are also kept in a stash entry until you drop it."
    );
    assert_eq!(subjects(&r, "main~1..main"), ["one"]);
    assert_eq!(r.git(&["stash", "list"]).lines().count(), 1);
    let catalog = Catalog::load(r.path()).unwrap();
    assert_eq!(catalog.stuck(), Some(Stuck::Conflicts));
}

#[test]
fn a_branch_that_moved_since_the_preview_is_left_alone() {
    let (mut r, [one, ..]) = diverged();
    let pick = chosen(&r, &[&one]).cherry_pick(false, false);
    r.write("later", b"later\n");
    r.commit_all("later");
    let error = failed(execute(&r, pick));
    assert!(error.contains("moved"), "{error}");
    assert_eq!(subjects(&r, "main~1..main"), ["later"]);
}
