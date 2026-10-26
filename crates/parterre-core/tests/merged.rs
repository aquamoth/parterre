mod common;

use common::{TestRepo, git_at, oid};
use parterre_core::git::Git;
use parterre_core::merged::{self, How, Merged};
use parterre_core::revgraph::{self, GraphOptions};
use parterre_core::{CommitIx, Repo};
use tempfile::TempDir;

/// A repository with a bare `origin` whose HEAD is `main`.
struct Setup {
    repo: TestRepo,
    _remote: TempDir,
}

impl Setup {
    fn new() -> Setup {
        let mut repo = TestRepo::new();
        let remote = tempfile::tempdir().expect("tempdir");
        let origin = remote.path().join("origin.git");
        let origin = origin.to_str().expect("utf-8 path");
        git_at(
            remote.path(),
            0,
            &["init", "-q", "--bare", "-b", "main", origin],
        );
        repo.git(&["remote", "add", "origin", origin]);
        repo.write("main.txt", b"1\n2\n3\n");
        repo.commit_all("Start");
        repo.git(&["push", "-q", "-u", "origin", "main"]);
        repo.git(&["remote", "set-head", "origin", "main"]);
        Setup {
            repo,
            _remote: remote,
        }
    }

    /// Appends `message` to `file` and commits it; returns its hash.
    fn change(&mut self, file: &str, message: &str) -> String {
        let path = self.repo.path().join(file);
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        text.push_str(message);
        text.push('\n');
        self.repo.write(file, text.as_bytes());
        self.repo.commit_all(message)
    }

    /// A branch `name` off `main`, with commits `messages` to `name`.txt, pushed and tracking
    /// `origin/name`. Ends back on `main`; returns the commits.
    fn pushed_branch(&mut self, name: &str, messages: &[&str]) -> Vec<String> {
        self.repo.git(&["switch", "-q", "-c", name, "main"]);
        let file = format!("{name}.txt");
        let commits = messages.iter().map(|m| self.change(&file, m)).collect();
        self.repo.git(&["push", "-q", "-u", "origin", name]);
        self.repo.git(&["switch", "-q", "main"]);
        commits
    }

    fn main_moves_on(&mut self, message: &str) -> String {
        self.repo.git(&["switch", "-q", "main"]);
        self.change("other.txt", message)
    }

    /// As a forge squash-merges a pull request of `branch`: one commit on `main`, pushed.
    fn squash_merge(&mut self, branch: &str) -> String {
        self.repo.git(&["switch", "-q", "main"]);
        self.repo.git(&["merge", "-q", "--squash", branch]);
        let commit = self.repo.commit(&format!("{branch} (#1)"));
        self.repo.git(&["push", "-q", "origin", "main"]);
        commit
    }

    /// As a forge rebase-merges one: copies of its commits on top of `main`, pushed. Returns
    /// the copy of its tip.
    fn rebase_merge(&mut self, branch: &str) -> String {
        self.repo
            .git(&["switch", "-q", "--detach", &format!("refs/heads/{branch}")]);
        self.repo.git(&["rebase", "-q", "main"]);
        let tip = self.repo.git(&["rev-parse", "HEAD"]);
        self.repo.git(&["switch", "-q", "main"]);
        self.repo.git(&["merge", "-q", "--ff-only", &tip]);
        self.repo.git(&["push", "-q", "origin", "main"]);
        tip
    }

    /// The forge deletes the branch once merged, and a fetch prunes it here.
    fn delete_on_origin(&self, branch: &str) {
        self.repo.git(&["push", "-q", "origin", "--delete", branch]);
        self.repo.git(&["fetch", "-q", "--prune", "origin"]);
    }

    fn load(&self) -> Repo {
        self.repo.load()
    }
}

fn branch(repo: &Repo, name: &str) -> usize {
    repo.refs
        .iter()
        .position(|r| r.full_name == format!("refs/heads/{name}"))
        .unwrap_or_else(|| panic!("no branch {name}"))
}

fn names(repo: &Repo, branches: &[usize]) -> Vec<String> {
    branches
        .iter()
        .map(|&b| repo.refs[b].name.clone())
        .collect()
}

/// What the local check finds for each candidate, by name.
fn found(s: &Setup) -> Vec<(String, Option<Merged>)> {
    let repo = s.load();
    let candidates = merged::candidates(&repo);
    let answers = merged::find(&Git::new(s.repo.path()), &repo, &candidates).expect("find");
    names(&repo, &candidates).into_iter().zip(answers).collect()
}

fn squash(landed: &str) -> Option<Merged> {
    Some(Merged {
        how: Some(How::Squash),
        landed: Some(oid(landed)),
        pull_request: None,
    })
}

#[test]
fn a_squash_merged_branch_is_found() {
    let mut s = Setup::new();
    s.pushed_branch("topic", &["One", "Two"]);
    // The default branch moves on meanwhile, elsewhere.
    s.main_moves_on("Elsewhere");
    let landed = s.squash_merge("topic");
    s.delete_on_origin("topic");
    assert_eq!(found(&s), [("topic".to_owned(), squash(&landed))]);
}

#[test]
fn a_rebase_merged_branch_is_found() {
    let mut s = Setup::new();
    s.pushed_branch("topic", &["One", "Two", "Three"]);
    s.main_moves_on("Elsewhere");
    let tip = s.rebase_merge("topic");
    s.delete_on_origin("topic");
    let expected = Merged {
        how: Some(How::Rebase),
        landed: Some(oid(&tip)),
        pull_request: None,
    };
    assert_eq!(found(&s), [("topic".to_owned(), Some(expected))]);
}

/// A rebase of one commit looks like a squash of it, and it names the commit.
#[test]
fn one_commit_rebased_is_squash_merged() {
    let mut s = Setup::new();
    s.pushed_branch("topic", &["One"]);
    s.main_moves_on("Elsewhere");
    let tip = s.rebase_merge("topic");
    s.delete_on_origin("topic");
    assert_eq!(found(&s), [("topic".to_owned(), squash(&tip))]);
}

/// Only the whole branch counts.
#[test]
fn a_branch_partly_upstream_is_not_merged() {
    let mut s = Setup::new();
    let commits = s.pushed_branch("topic", &["One", "Two"]);
    s.main_moves_on("Elsewhere");
    s.repo.git(&["cherry-pick", &commits[0]]);
    s.repo.git(&["push", "-q", "origin", "main"]);
    s.delete_on_origin("topic");
    assert_eq!(found(&s), [("topic".to_owned(), None)]);
}

/// A branch that adds `lines` to `main.txt`, pushed and gone; meanwhile `main` adds `Main`
/// there too, and then lands `landed` after it: as a forge squash-merges a pull request it
/// rebased first. Returns the squash commit.
fn rebased_and_squashed(s: &mut Setup, lines: &[&str], landed: &[&str]) -> String {
    s.repo.git(&["switch", "-q", "-c", "topic", "main"]);
    for line in lines {
        s.change("main.txt", line);
    }
    s.repo.git(&["push", "-q", "-u", "origin", "topic"]);
    s.main_moves_on("Elsewhere");
    s.change("main.txt", "Main");
    let text: String = landed.iter().map(|l| format!("{l}\n")).collect();
    let mut file = std::fs::read_to_string(s.repo.path().join("main.txt")).unwrap();
    file.push_str(&text);
    s.repo.write("main.txt", file.as_bytes());
    let commit = s.repo.commit_all("topic (#1)");
    s.repo.git(&["push", "-q", "origin", "main"]);
    s.delete_on_origin("topic");
    commit
}

/// The rebase put its lines next to others: the same lines added count, not their neighbours
/// (#437).
#[test]
fn a_branch_rebased_before_its_squash_merge_is_found() {
    let mut s = Setup::new();
    let landed = rebased_and_squashed(&mut s, &["One", "Two"], &["One", "Two"]);
    assert_eq!(found(&s), [("topic".to_owned(), squash(&landed))]);
}

/// Not if one of its lines didn't land.
#[test]
fn a_branch_rebased_with_a_line_dropped_is_not_merged() {
    let mut s = Setup::new();
    rebased_and_squashed(&mut s, &["One", "Two"], &["One"]);
    assert_eq!(found(&s), [("topic".to_owned(), None)]);
}

/// Nor if one of them landed changed.
#[test]
fn a_branch_rebased_with_a_line_changed_is_not_merged() {
    let mut s = Setup::new();
    rebased_and_squashed(&mut s, &["One", "Two"], &["One", "Too"]);
    assert_eq!(found(&s), [("topic".to_owned(), None)]);
}

#[test]
fn a_branch_deleted_unmerged_is_not_merged() {
    let mut s = Setup::new();
    s.pushed_branch("topic", &["One"]);
    s.delete_on_origin("topic");
    assert_eq!(found(&s), [("topic".to_owned(), None)]);
}

/// The same changes on the default branch before the branch forked aren't where it landed.
#[test]
fn changes_from_before_the_branch_are_not_where_it_landed() {
    let mut s = Setup::new();
    // A change, and its revert, on the default branch.
    s.change("topic.txt", "One");
    s.repo.git(&["revert", "--no-edit", "HEAD"]);
    s.repo.git(&["push", "-q", "origin", "main"]);
    // The branch makes the same two commits again.
    s.repo.git(&["switch", "-q", "-c", "topic", "main"]);
    s.change("topic.txt", "One");
    s.repo.git(&["revert", "--no-edit", "HEAD"]);
    s.repo.git(&["push", "-q", "-u", "origin", "topic"]);
    s.repo.git(&["switch", "-q", "main"]);
    s.delete_on_origin("topic");
    assert_eq!(found(&s), [("topic".to_owned(), None)]);
}

/// One copy upstream stands for one of the branch's commits, however many make that change.
#[test]
fn one_copy_is_not_two_commits_rebased() {
    let mut s = Setup::new();
    // The branch adds a line, takes it out, and adds it again.
    s.repo.git(&["switch", "-q", "-c", "topic", "main"]);
    let add = s.change("topic.txt", "One");
    s.repo.git(&["revert", "--no-edit", "HEAD"]);
    s.change("topic.txt", "One");
    s.repo.git(&["push", "-q", "-u", "origin", "topic"]);
    // Only the first two have a copy on the default branch.
    s.main_moves_on("Elsewhere");
    s.repo.git(&["cherry-pick", &add, "topic~1"]);
    let added = s.repo.git(&["rev-parse", "HEAD~1"]);
    s.repo.git(&["push", "-q", "origin", "main"]);
    s.delete_on_origin("topic");
    // Not rebase-merged; the change of the whole branch is the added line, though.
    assert_eq!(found(&s), [("topic".to_owned(), squash(&added))]);
}

/// Merge commits and fast-forwards need nothing: the graph shows where they landed.
#[test]
fn only_gone_branches_the_default_branch_lacks_are_candidates() {
    let mut s = Setup::new();
    s.pushed_branch("merged", &["Merged"]);
    s.repo
        .git(&["merge", "-q", "--no-ff", "-m", "Merge", "merged"]);
    s.repo.git(&["push", "-q", "origin", "main"]);
    s.delete_on_origin("merged");
    s.pushed_branch("open", &["Open"]);
    s.pushed_branch("gone", &["Gone"]);
    s.delete_on_origin("gone");
    // No upstream at all.
    s.repo.git(&["branch", "-q", "local", "gone"]);
    let repo = s.load();
    assert_eq!(names(&repo, &merged::candidates(&repo)), ["gone"]);
}

#[test]
fn candidates_are_newest_tip_first() {
    let mut s = Setup::new();
    s.repo.set_clock(10);
    s.pushed_branch("old", &["Old"]);
    s.repo.set_clock(30);
    s.pushed_branch("new", &["New"]);
    s.repo.set_clock(20);
    s.pushed_branch("middle", &["Middle"]);
    for b in ["old", "new", "middle"] {
        s.delete_on_origin(b);
    }
    let repo = s.load();
    assert_eq!(
        names(&repo, &merged::candidates(&repo)),
        ["new", "middle", "old"]
    );
}

/// Several branches at once, one squashed and one rebased, as one check asks git.
#[test]
fn branches_are_checked_together() {
    let mut s = Setup::new();
    s.pushed_branch("squashed", &["S1", "S2"]);
    s.pushed_branch("rebased", &["R1", "R2"]);
    s.pushed_branch("abandoned", &["A1"]);
    s.main_moves_on("Elsewhere");
    let squashed = s.squash_merge("squashed");
    let rebased = s.rebase_merge("rebased");
    for b in ["squashed", "rebased", "abandoned"] {
        s.delete_on_origin(b);
    }
    let mut got = found(&s);
    got.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        got,
        [
            ("abandoned".to_owned(), None),
            (
                "rebased".to_owned(),
                Some(Merged {
                    how: Some(How::Rebase),
                    landed: Some(oid(&rebased)),
                    pull_request: None,
                })
            ),
            ("squashed".to_owned(), squash(&squashed)),
        ]
    );
}

/// The commit a merged branch landed as stays a node, though it has no ref and isn't a merge.
#[test]
fn the_commit_it_landed_as_is_a_node() {
    let mut s = Setup::new();
    s.pushed_branch("topic", &["One"]);
    s.main_moves_on("Before");
    let landed = s.squash_merge("topic");
    s.main_moves_on("After");
    s.repo.git(&["push", "-q", "origin", "main"]);
    s.delete_on_origin("topic");
    let repo = s.load();
    let landed: CommitIx = repo.lookup(&oid(&landed)).expect("in the snapshot");
    let topic = (branch(&repo, "topic"), landed);
    let options = GraphOptions::default();
    let without = revgraph::build(&repo, &options);
    assert_eq!(without.node_of(landed), None);
    let with = revgraph::build_with_landed(&repo, &options, &[], &[topic]);
    assert!(with.node_of(landed).is_some());
    assert_eq!(with.nodes.len(), without.nodes.len() + 1);
    // Not while the branch itself is hidden: nothing points there.
    let no_local = GraphOptions {
        show_local_branches: false,
        ..GraphOptions::default()
    };
    let graph = revgraph::build_with_landed(&repo, &no_local, &[], &[topic]);
    assert_eq!(graph.node_of(landed), None);
    // Only where it is shown anyway.
    let hidden = GraphOptions {
        show_remote_branches: false,
        current_branch_only: true,
        ..GraphOptions::default()
    };
    s.repo.git(&["switch", "-q", "topic"]);
    let repo = s.load();
    let topic = (branch(&repo, "topic"), landed);
    let graph = revgraph::build_with_landed(&repo, &hidden, &[], &[topic]);
    assert_eq!(graph.node_of(landed), None);
}

#[test]
fn answers_are_kept_by_branch_and_tip() {
    let mut s = Setup::new();
    s.pushed_branch("topic", &["One"]);
    s.main_moves_on("Elsewhere");
    let landed = s.squash_merge("topic");
    s.delete_on_origin("topic");
    let repo = s.load();
    let topic = branch(&repo, "topic");
    let tip = repo.commit(repo.refs[topic].target).oid;
    let mut answers = merged::MergedBranches::default();
    let answer = squash(&landed).unwrap();
    answers.insert("refs/heads/topic", tip, answer.clone());
    assert_eq!(answers.of(&repo, topic), Some(&answer));
    let landed = repo.lookup(&oid(&landed)).unwrap();
    assert_eq!(answers.landed(&repo), [(topic, landed)]);
    // The branch moved on: the answer was about another commit.
    answers.insert("refs/heads/topic", oid(&"0".repeat(40)), answer);
    assert_eq!(answers.of(&repo, topic), None);
    assert!(answers.landed(&repo).is_empty());
}
