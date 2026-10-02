mod common;

use std::path::Path;
use std::process::Command;

use common::TestRepo;
use parterre_core::revgraph::{self, GraphOptions, Simplification};
use parterre_core::upstream::{Side, Upstream};
use parterre_core::{CommitIx, Repo};
use tempfile::TempDir;

/// Runs git in `dir`, as [`TestRepo::git`] does in the repository, at `minutes` past its base
/// date.
fn git_in(dir: &Path, minutes: u32, args: &[&str]) -> String {
    let date = format!("{} +0000", 1_700_000_000 + minutes * 60);
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A repository with a bare `origin`, and a colleague's clone of it that can push.
struct Setup {
    repo: TestRepo,
    /// Holds `origin.git` and the colleague's `other`.
    remote: TempDir,
}

impl Setup {
    /// `main` with one file, pushed to `origin` and tracking `origin/main`.
    fn new() -> Setup {
        let mut repo = TestRepo::new();
        let remote = tempfile::tempdir().expect("tempdir");
        let origin = remote.path().join("origin.git");
        let origin = origin.to_str().expect("utf-8 path");
        git_in(
            remote.path(),
            0,
            &["init", "-q", "--bare", "-b", "main", origin],
        );
        repo.git(&["remote", "add", "origin", origin]);
        repo.write("main.txt", b"1\n");
        repo.commit_all("Start");
        repo.git(&["push", "-q", "-u", "origin", "main"]);
        Setup { repo, remote }
    }

    /// Changes `file` and commits it as `message`; returns its hash.
    fn change(&mut self, file: &str, message: &str) -> String {
        let path = self.repo.path().join(file);
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        text.push_str(message);
        text.push('\n');
        self.repo.write(file, text.as_bytes());
        self.repo.commit_all(message)
    }

    /// The colleague clones `origin`, commits `message` to `branch` and pushes it; then this
    /// repository fetches. Returns the colleague's commit.
    fn colleague_pushes(&self, branch: &str, file: &str, message: &str) -> String {
        let other = self.remote.path().join("other");
        if !other.exists() {
            let origin = self.remote.path().join("origin.git");
            let args = ["clone", "-q", origin.to_str().unwrap(), "other"];
            git_in(self.remote.path(), 500, &args);
            git_in(&other, 500, &["config", "user.name", "Colleague"]);
            git_in(&other, 500, &["config", "user.email", "c@example.com"]);
        }
        git_in(&other, 500, &["fetch", "-q", "origin"]);
        git_in(
            &other,
            500,
            &["checkout", "-q", "-B", branch, &format!("origin/{branch}")],
        );
        std::fs::write(other.join(file), format!("{message}\n")).expect("write file");
        git_in(&other, 500, &["add", "-A"]);
        git_in(&other, 500, &["commit", "-q", "-m", message]);
        git_in(&other, 500, &["push", "-q", "origin", branch]);
        self.repo.git(&["fetch", "-q", "origin"]);
        git_in(&other, 500, &["rev-parse", "HEAD"])
    }

    /// A branch `name` off `main`, with commits `messages` (each to `name`.txt), pushed and
    /// tracking `origin/name`. Ends back on `main`.
    fn pushed_branch(&mut self, name: &str, messages: &[&str]) -> Vec<String> {
        self.repo.git(&["switch", "-q", "-c", name, "main"]);
        let file = format!("{}.txt", name.replace('/', "-"));
        let commits = messages.iter().map(|m| self.change(&file, m)).collect();
        self.repo.git(&["push", "-q", "-u", "origin", name]);
        self.repo.git(&["switch", "-q", "main"]);
        commits
    }

    /// A new commit on `main`, which branches can then be rebased onto.
    fn main_moves_on(&mut self) -> String {
        self.repo.git(&["switch", "-q", "main"]);
        self.change("main.txt", "Tidy up main")
    }

    fn rebase_onto_main(&self, branch: &str) {
        self.repo.git(&["switch", "-q", branch]);
        self.repo.git(&["rebase", "-q", "main"]);
        self.repo.git(&["switch", "-q", "main"]);
    }
}

fn upstream<'a>(repo: &'a Repo, branch: &str) -> &'a Upstream {
    let i = repo
        .refs
        .iter()
        .position(|r| r.full_name == format!("refs/heads/{branch}"))
        .unwrap_or_else(|| panic!("no branch {branch}"));
    repo.upstream_of(i)
        .unwrap_or_else(|| panic!("{branch} has no upstream"))
}

/// The commits on one side only, as (subject, side), newest first.
fn sides(repo: &Repo, u: &Upstream) -> Vec<(String, Side)> {
    u.commits
        .iter()
        .map(|&(c, s)| (repo.commit(c).subject.clone(), s))
        .collect()
}

fn named(list: &[(&str, Side)]) -> Vec<(String, Side)> {
    list.iter().map(|&(s, side)| (s.to_owned(), side)).collect()
}

fn subject(repo: &Repo, c: CommitIx) -> &str {
    &repo.commit(c).subject
}

/// git's own counts, as `for-each-ref` prints them: `ahead 2, behind 1`.
fn git_track(r: &TestRepo, branch: &str) -> String {
    r.git(&[
        "for-each-ref",
        "--format=%(upstream:track,nobracket)",
        &format!("refs/heads/{branch}"),
    ])
}

fn counts(u: &Upstream) -> String {
    match (u.ahead(), u.behind()) {
        (0, 0) => String::new(),
        (a, 0) => format!("ahead {a}"),
        (0, b) => format!("behind {b}"),
        (a, b) => format!("ahead {a}, behind {b}"),
    }
}

#[test]
fn level_branch_has_nothing_between() {
    let s = Setup::new();
    let repo = s.repo.load();
    let u = upstream(&repo, "main");
    assert_eq!(u.short_name(), "origin/main");
    assert!(!u.is_gone());
    assert!(u.commits.is_empty());
    assert_eq!(u.rebased_from, None);
}

#[test]
fn unpushed_commits_are_ahead() {
    let mut s = Setup::new();
    s.change("main.txt", "One");
    s.change("main.txt", "Two");
    let repo = s.repo.load();
    let u = upstream(&repo, "main");
    let want = named(&[("Two", Side::Ahead), ("One", Side::Ahead)]);
    assert_eq!(sides(&repo, u), want);
    assert_eq!((u.ahead(), u.behind()), (2, 0));
    assert_eq!(counts(u), git_track(&s.repo, "main"));
    assert!(!u.is_rebased());
    assert!(!u.loses_commits());
}

#[test]
fn commits_pushed_by_others_are_behind() {
    let s = Setup::new();
    s.colleague_pushes("main", "theirs.txt", "Theirs");
    let repo = s.repo.load();
    let u = upstream(&repo, "main");
    assert_eq!(sides(&repo, u), named(&[("Theirs", Side::Behind)]));
    assert_eq!(counts(u), git_track(&s.repo, "main"));
}

#[test]
fn a_branch_moved_back_is_only_behind() {
    // Pushed, then reset: the commit was on the branch, but nothing replaced it, so a pull
    // brings it back.
    let mut s = Setup::new();
    s.change("main.txt", "Undone");
    s.repo.git(&["push", "-q"]);
    s.repo.git(&["reset", "-q", "--hard", "HEAD~1"]);
    let repo = s.repo.load();
    let u = upstream(&repo, "main");
    assert_eq!(sides(&repo, u), named(&[("Undone", Side::Behind)]));
    assert_eq!(u.rebased_from, None);
}

#[test]
fn diverged_without_a_rebase_is_only_behind() {
    // Both sides committed, nothing rewritten: a pull brings the colleague's commit, and
    // nobody force-pushes a branch that wasn't rewritten.
    let mut s = Setup::new();
    s.colleague_pushes("main", "theirs.txt", "Theirs");
    s.change("main.txt", "Mine");
    let repo = s.repo.load();
    let u = upstream(&repo, "main");
    let want = named(&[("Mine", Side::Ahead), ("Theirs", Side::Behind)]);
    assert_eq!(sides(&repo, u), want);
    assert_eq!(counts(u), git_track(&s.repo, "main"));
    assert!(!u.is_rebased());
    assert!(!u.loses_commits());
    assert_eq!(u.rebased_from, None);
}

#[test]
fn a_rebase_replaces_what_was_pushed() {
    // Pushed, then rebased onto main: every commit on the remote has a rebased copy.
    let mut s = Setup::new();
    s.pushed_branch("export", &["Export to CSV", "Export to Excel"]);
    s.main_moves_on();
    s.rebase_onto_main("export");
    let repo = s.repo.load();
    let u = upstream(&repo, "export");
    let want = named(&[
        ("Export to Excel", Side::Ahead),
        ("Export to CSV", Side::Ahead),
        ("Tidy up main", Side::Ahead),
        ("Export to Excel", Side::Replaced),
        ("Export to CSV", Side::Replaced),
    ]);
    assert_eq!(sides(&repo, u), want);
    assert_eq!(counts(u), git_track(&s.repo, "export"));
    assert!(u.is_rebased());
    assert!(!u.loses_commits());
    // The upstream still points at the commit it was rebased from: no extra node.
    assert_eq!(u.rebased_from, None);
}

#[test]
fn a_rebase_after_a_colleague_pushed_would_lose_theirs() {
    // Pushed, a colleague pushed a fix on top, then rebased onto main without pulling.
    let mut s = Setup::new();
    s.pushed_branch("import", &["Import from CSV", "Import from JSON"]);
    s.colleague_pushes("import", "fix.txt", "Fix encoding");
    s.main_moves_on();
    s.rebase_onto_main("import");
    let repo = s.repo.load();
    let u = upstream(&repo, "import");
    let want = named(&[
        ("Import from JSON", Side::Ahead),
        ("Import from CSV", Side::Ahead),
        ("Tidy up main", Side::Ahead),
        ("Fix encoding", Side::Lost),
        ("Import from JSON", Side::Replaced),
        ("Import from CSV", Side::Replaced),
    ]);
    assert_eq!(sides(&repo, u), want);
    assert_eq!(counts(u), git_track(&s.repo, "import"));
    assert!(u.is_rebased());
    assert!(u.loses_commits());
    // The upstream moved on past the commit the branch was rebased from.
    let from = u.rebased_from.expect("rebased from");
    assert_eq!(subject(&repo, from), "Import from JSON");
    assert_ne!(Some(from), u.target.map(|t| repo.refs[t].target));
}

#[test]
fn a_cherry_picked_copy_is_not_lost() {
    // As above, then the colleague's fix cherry-picked onto the rebased branch: nothing is
    // lost, though the branch never had the fix itself.
    let mut s = Setup::new();
    s.pushed_branch("sync", &["Sync engine", "Sync conflicts"]);
    s.colleague_pushes("sync", "retry.txt", "Retry on timeout");
    s.main_moves_on();
    s.rebase_onto_main("sync");
    s.repo.git(&["switch", "-q", "sync"]);
    s.repo.git(&["cherry-pick", "origin/sync"]);
    s.repo.git(&["switch", "-q", "main"]);
    let repo = s.repo.load();
    let u = upstream(&repo, "sync");
    let replaced: Vec<String> = sides(&repo, u)
        .into_iter()
        .filter(|&(_, side)| side == Side::Replaced)
        .map(|(subject, _)| subject)
        .collect();
    assert_eq!(
        replaced,
        ["Retry on timeout", "Sync conflicts", "Sync engine"]
    );
    assert!(u.is_rebased());
    assert!(!u.loses_commits());
    // The newest replaced commit is the upstream's own: it is a node already.
    assert_eq!(u.rebased_from, None);
}

#[test]
fn a_rebase_that_drops_a_commit_loses_it() {
    // Rebased with one commit dropped: the branch had it (its reflog says so, and git's
    // --force-if-includes would allow the push), but nothing replaced it.
    let mut s = Setup::new();
    let pushed = s.pushed_branch("drop", &["Keep me", "Drop me"]);
    s.main_moves_on();
    s.repo.git(&["switch", "-q", "drop"]);
    s.repo.git(&["reset", "-q", "--hard", "main"]);
    s.repo.git(&["cherry-pick", &pushed[0]]);
    s.repo.git(&["switch", "-q", "main"]);
    let repo = s.repo.load();
    let u = upstream(&repo, "drop");
    let behind: Vec<(String, Side)> = sides(&repo, u)
        .into_iter()
        .filter(|&(_, side)| side != Side::Ahead)
        .collect();
    let want = named(&[("Drop me", Side::Lost), ("Keep me", Side::Replaced)]);
    assert_eq!(behind, want);
    assert!(u.is_rebased());
    assert!(u.loses_commits());
}

#[test]
fn a_rebase_that_only_drops_is_only_behind() {
    // `git rebase --onto main` with the upstream level: nothing to replay, so the branch's
    // own commit is dropped and it moves to main. A pull brings the commit back.
    let mut s = Setup::new();
    s.pushed_branch("research", &["Research"]);
    s.main_moves_on();
    s.repo.git(&["switch", "-q", "research"]);
    s.repo.git(&["rebase", "-q", "--onto", "main"]);
    s.repo.git(&["switch", "-q", "main"]);
    let repo = s.repo.load();
    let u = upstream(&repo, "research");
    let want = named(&[("Tidy up main", Side::Ahead), ("Research", Side::Behind)]);
    assert_eq!(sides(&repo, u), want);
    assert!(!u.is_rebased());
}

#[test]
fn commits_in_the_same_second_are_told_apart() {
    // A dropped commit made in the same second as the commit it was rebased onto, by the same
    // author, as scripts and agents do: still not replaced.
    let mut s = Setup::new();
    s.repo.set_clock(50);
    s.pushed_branch("quick", &["Quick"]);
    s.repo.set_clock(50);
    s.main_moves_on();
    s.repo.git(&["switch", "-q", "quick"]);
    s.repo.git(&["rebase", "-q", "--onto", "main"]);
    s.repo.git(&["switch", "-q", "main"]);
    let repo = s.repo.load();
    let u = upstream(&repo, "quick");
    let (quick, tidy) = (&u.commits[1].0, &u.commits[0].0);
    assert_eq!(
        repo.commit(*quick).author_time,
        repo.commit(*tidy).author_time
    );
    let want = named(&[("Tidy up main", Side::Ahead), ("Quick", Side::Behind)]);
    assert_eq!(sides(&repo, u), want);
}

#[test]
fn a_conflict_resolved_in_a_rebase_still_replaces() {
    // Resolving a conflict changes the patch, but rebase keeps the author date.
    let mut s = Setup::new();
    s.pushed_branch("clash", &["Clash"]);
    s.repo.git(&["switch", "-q", "main"]);
    s.repo.write("clash.txt", b"main's version\n");
    s.repo.commit_all("Main clashes");
    s.repo.git(&["switch", "-q", "clash"]);
    let rebase = Command::new("git")
        .current_dir(s.repo.path())
        .args(["rebase", "-q", "main"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(!rebase.status.success(), "the rebase should conflict");
    s.repo.write("clash.txt", b"both\n");
    s.repo.git(&["add", "clash.txt"]);
    s.repo
        .git(&["-c", "core.editor=true", "rebase", "--continue"]);
    s.repo.git(&["switch", "-q", "main"]);
    let repo = s.repo.load();
    let u = upstream(&repo, "clash");
    let behind: Vec<(String, Side)> = sides(&repo, u)
        .into_iter()
        .filter(|&(_, side)| side != Side::Ahead)
        .collect();
    assert_eq!(behind, named(&[("Clash", Side::Replaced)]));
}

#[test]
fn a_squash_shows_the_squashed_away_commits_lost() {
    // The known false alarm: a squash keeps only the first commit's author date.
    let mut s = Setup::new();
    s.pushed_branch("squash", &["First", "Second"]);
    s.repo.git(&["switch", "-q", "squash"]);
    s.repo.git(&["reset", "-q", "--soft", "HEAD~2"]);
    s.repo.git(&["commit", "-q", "-C", "origin/squash~1"]);
    s.repo.git(&["switch", "-q", "main"]);
    let repo = s.repo.load();
    let u = upstream(&repo, "squash");
    let behind: Vec<(String, Side)> = sides(&repo, u)
        .into_iter()
        .filter(|&(_, side)| side != Side::Ahead)
        .collect();
    assert_eq!(
        behind,
        named(&[("Second", Side::Lost), ("First", Side::Replaced)])
    );
}

#[test]
fn a_deleted_upstream_is_gone() {
    let mut s = Setup::new();
    s.pushed_branch("login", &["Login form"]);
    s.repo.git(&["push", "-q", "origin", "--delete", "login"]);
    s.repo.git(&["fetch", "-q", "--prune", "origin"]);
    let repo = s.repo.load();
    let u = upstream(&repo, "login");
    assert!(u.is_gone());
    assert_eq!(u.short_name(), "origin/login");
    assert!(u.commits.is_empty());
}

#[test]
fn branches_without_an_upstream_have_none() {
    let mut s = Setup::new();
    s.repo.branch("local");
    s.change("local.txt", "Local only");
    let repo = s.repo.load();
    let local = repo
        .refs
        .iter()
        .position(|r| r.name == "local")
        .expect("local");
    assert!(repo.upstream_of(local).is_none());
    assert_eq!(repo.upstreams.len(), 1, "only main has one");
}

#[test]
fn a_local_upstream_counts_too() {
    // `git branch -u main`: the upstream is another local branch.
    let mut s = Setup::new();
    s.repo
        .git(&["switch", "-q", "-c", "topic", "--track", "main"]);
    s.change("topic.txt", "Topic");
    s.repo.git(&["switch", "-q", "main"]);
    s.change("main.txt", "Main");
    let repo = s.repo.load();
    let u = upstream(&repo, "topic");
    assert_eq!(u.short_name(), "main");
    let want = named(&[("Topic", Side::Ahead), ("Main", Side::Behind)]);
    assert_eq!(sides(&repo, u), want);
    assert_eq!(counts(u), git_track(&s.repo, "topic"));
}

#[test]
fn counts_hold_with_merges_and_skewed_clocks() {
    // Merges on both sides, and commits dated before their parents: the counts stay git's.
    let mut s = Setup::new();
    s.repo.set_clock(100);
    s.change("main.txt", "Late");
    s.repo.git(&["push", "-q"]);
    s.repo.git(&["switch", "-q", "-c", "side", "main~1"]);
    s.repo.set_clock(10);
    s.change("side.txt", "Early side");
    s.repo.git(&["switch", "-q", "main"]);
    s.repo.set_clock(20);
    s.repo.merge("side", "Merge side");
    s.repo.set_clock(5);
    s.change("main.txt", "Skewed");
    s.colleague_pushes("main", "theirs.txt", "Theirs");
    let repo = s.repo.load();
    let u = upstream(&repo, "main");
    assert_eq!(counts(u), git_track(&s.repo, "main"));
    let ahead: Vec<String> = sides(&repo, u)
        .into_iter()
        .filter(|&(_, side)| side == Side::Ahead)
        .map(|(subject, _)| subject)
        .collect();
    // Parents after their children, whatever the dates.
    assert_eq!(ahead, ["Skewed", "Merge side", "Early side"]);
}

#[test]
fn reloading_notices_a_new_upstream() {
    let mut s = Setup::new();
    s.repo.branch("topic");
    s.change("topic.txt", "Topic");
    s.repo.git(&["push", "-q", "origin", "topic"]);
    let before = s.repo.load();
    s.repo.git(&["branch", "-q", "-u", "origin/topic"]);
    let after = s.repo.load();
    assert!(!before.same_refs(&after));
}

#[test]
fn edges_hold_the_commits_between() {
    // In the labelled-commits graph, the rebased branch's commits collapse into its edge to
    // main, and the upstream's into the edge from origin/import down to its fork point.
    let mut s = Setup::new();
    s.pushed_branch("import", &["Import from CSV", "Import from JSON"]);
    s.colleague_pushes("import", "fix.txt", "Fix encoding");
    s.main_moves_on();
    s.rebase_onto_main("import");
    let repo = s.repo.load();
    let u = upstream(&repo, "import").clone();
    let options = GraphOptions {
        simplification: Simplification::Decorated,
        ..GraphOptions::default()
    };
    let graph = revgraph::build(&repo, &options);
    // The commit it was rebased from is a node of its own.
    let from = u.rebased_from.expect("rebased from");
    assert!(graph.node_of(from).is_some());
    let hidden = GraphOptions {
        show_upstreams: false,
        ..options.clone()
    };
    assert!(revgraph::build(&repo, &hidden).node_of(from).is_none());

    let label = |node: u32| {
        let n = &graph.nodes[node as usize];
        n.refs
            .first()
            .map(|&r| repo.refs[r].name.clone())
            .unwrap_or_else(|| repo.commit(n.commit).subject.clone())
    };
    let mut edges: Vec<(String, String, Vec<Option<Side>>)> = u
        .edge_sides(&graph, &repo)
        .into_iter()
        .map(|(e, sides)| {
            let edge = graph.edges[e];
            (label(edge.child), label(edge.parent), sides)
        })
        .collect();
    edges.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    use Side::*;
    assert_eq!(
        edges,
        [
            // The rebased-from node down to the fork point (origin/main, as main moved on): JSON's
            // original and CSV's.
            (
                "Import from JSON".to_owned(),
                "origin/main".to_owned(),
                vec![Some(Replaced), Some(Replaced)]
            ),
            // The rebased branch down to main: its two commits.
            (
                "import".to_owned(),
                "main".to_owned(),
                vec![Some(Ahead), Some(Ahead)]
            ),
            // main's new commit, on the branch only.
            (
                "main".to_owned(),
                "origin/main".to_owned(),
                vec![Some(Ahead)]
            ),
            // The colleague's fix, down to the rebased-from node.
            (
                "origin/import".to_owned(),
                "Import from JSON".to_owned(),
                vec![Some(Lost)]
            ),
        ]
    );
}
