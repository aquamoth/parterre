//! Comparing two commits: sides, and the files git lists between them.

mod common;

use common::TestRepo;
use parterre_core::changed_files::FileStatus;
use parterre_core::compare::Comparison;
use parterre_core::file_diff::{Content, FileDiffSpec, Rev};
use parterre_core::git::Git;
use parterre_core::{CommitIx, Oid, Repo};

fn ix(repo: &Repo, hash: &str) -> CommitIx {
    repo.lookup(&Oid::from_hex(hash).unwrap())
        .expect("commit in snapshot")
}

fn oid(hash: &str) -> Rev {
    Rev::Commit(Oid::from_hex(hash).unwrap())
}

/// `main` and `feature` forked from `base`: main changed `shared.txt` and added `main.txt`,
/// feature added `feature.txt`.
fn forked() -> (TestRepo, [String; 3]) {
    let mut r = TestRepo::new();
    r.write("shared.txt", b"one\n");
    let base = r.commit_all("base");
    r.branch("feature");
    r.write("feature.txt", b"f\n");
    let feature = r.commit_all("feature");
    r.checkout("main");
    r.write("shared.txt", b"one\ntwo\n");
    r.write("main.txt", b"m\n");
    let main = r.commit_all("main");
    (r, [base, feature, main])
}

fn listed(r: &TestRepo, c: Comparison) -> Vec<(String, FileStatus)> {
    let compared = c.run(&Git::new(r.path())).expect("compare");
    compared
        .files
        .into_iter()
        .map(|f| (f.path, f.status))
        .collect()
}

#[test]
fn an_ancestor_goes_on_the_left_whichever_is_picked_first() {
    let (r, [base, feature, main]) = forked();
    let repo = r.load();
    let c = Comparison::of(&repo, ix(&repo, &feature), ix(&repo, &base), false);
    assert_eq!((c.old, c.new), (oid(&base), oid(&feature)));
    // Diverged commits keep the order they were picked in.
    let c = Comparison::of(&repo, ix(&repo, &feature), ix(&repo, &main), false);
    assert_eq!((c.old, c.new), (oid(&feature), oid(&main)));
    let swapped = c.swapped();
    assert_eq!((swapped.old, swapped.new), (oid(&main), oid(&feature)));
}

#[test]
fn trees_are_compared_whole_or_since_the_common_ancestor() {
    let (r, [base, feature, main]) = forked();
    let repo = r.load();
    let c = Comparison::of(&repo, ix(&repo, &main), ix(&repo, &feature), false);
    assert_eq!(
        listed(&r, c),
        [
            ("feature.txt".into(), FileStatus::Added),
            ("main.txt".into(), FileStatus::Deleted),
            ("shared.txt".into(), FileStatus::Modified),
        ]
    );
    // Since the fork only what feature did counts.
    let c = Comparison {
        since_ancestor: true,
        ..c
    };
    let compared = c.run(&Git::new(r.path())).unwrap();
    assert_eq!(compared.base, Some(oid(&base)));
    assert_eq!(listed(&r, c), [("feature.txt".into(), FileStatus::Added)]);
}

#[test]
fn the_common_ancestor_matters_only_where_the_histories_forked() {
    let (r, [base, feature, main]) = forked();
    let repo = r.load();
    let commit = |h: &str| Oid::from_hex(h).unwrap();
    let c = Comparison::of(&repo, ix(&repo, &base), ix(&repo, &feature), false);
    assert!(!c.ancestor_matters(&repo));
    assert!(c.swapped().ancestor_matters(&repo));
    let c = Comparison::of(&repo, ix(&repo, &feature), ix(&repo, &main), false);
    assert!(c.ancestor_matters(&repo));
    // The working tree counts as HEAD, which is main.
    assert!(!Comparison::with_working_tree(commit(&main), false).ancestor_matters(&repo));
    assert!(!Comparison::with_working_tree(commit(&base), false).ancestor_matters(&repo));
    assert!(Comparison::with_working_tree(commit(&feature), false).ancestor_matters(&repo));
}

#[test]
fn unrelated_histories_have_no_common_ancestor() {
    let (mut r, [_, _, main]) = forked();
    r.git(&["checkout", "-q", "--orphan", "other"]);
    r.git(&["rm", "-rqf", "."]);
    r.write("other.txt", b"o\n");
    let other = r.commit_all("other");
    let c = Comparison {
        old: oid(&main),
        new: oid(&other),
        since_ancestor: true,
    };
    let compared = c.run(&Git::new(r.path())).unwrap();
    assert_eq!(compared.base, None);
    assert!(compared.files.is_empty());
    let whole = listed(
        &r,
        Comparison {
            since_ancestor: false,
            ..c
        },
    );
    assert_eq!(whole.len(), 3, "{whole:?}");
}

/// The working tree of `forked` on main: `shared.txt` edited, `staged.txt` added to the index,
/// `main.txt` deleted from disk, and `untracked.txt` never added.
fn dirty() -> (TestRepo, [String; 3]) {
    let (r, commits) = forked();
    r.write("shared.txt", b"one\ntwo\nthree\n");
    r.write("staged.txt", b"s\n");
    r.git(&["add", "staged.txt"]);
    std::fs::remove_file(r.path().join("main.txt")).unwrap();
    r.write("untracked.txt", b"u\n");
    (r, commits)
}

#[test]
fn the_working_tree_is_compared_staged_or_not_without_untracked_files() {
    let (r, [_, _, main]) = dirty();
    let c = Comparison::with_working_tree(Oid::from_hex(&main).unwrap(), false);
    assert_eq!(
        listed(&r, c),
        [
            ("main.txt".into(), FileStatus::Deleted),
            ("shared.txt".into(), FileStatus::Modified),
            ("staged.txt".into(), FileStatus::Added),
        ]
    );
    // The other way round, the working tree is the old side.
    assert_eq!(
        listed(&r, c.swapped()),
        [
            ("main.txt".into(), FileStatus::Added),
            ("shared.txt".into(), FileStatus::Modified),
            ("staged.txt".into(), FileStatus::Deleted),
        ]
    );
    // Listing refreshes the index only in memory.
    let index = std::fs::metadata(r.path().join(".git/index"))
        .unwrap()
        .modified()
        .unwrap();
    listed(&r, c);
    let after = std::fs::metadata(r.path().join(".git/index"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(index, after);
}

#[test]
fn since_the_common_ancestor_the_working_tree_counts_as_head() {
    let (r, [base, feature, _]) = dirty();
    let c = Comparison::with_working_tree(Oid::from_hex(&feature).unwrap(), true);
    let compared = c.run(&Git::new(r.path())).unwrap();
    assert_eq!(compared.base, Some(oid(&base)));
    // What main's working tree changed since it forked from feature: main.txt was added
    // and deleted again.
    assert_eq!(
        listed(&r, c),
        [
            ("shared.txt".into(), FileStatus::Modified),
            ("staged.txt".into(), FileStatus::Added),
        ]
    );
}

/// Both sides of `path` compared between HEAD and the working tree.
fn texts(r: &TestRepo, path: &str) -> (String, String) {
    let git = Git::new(r.path());
    let head = Oid::from_hex(&r.git(&["rev-parse", "HEAD"])).unwrap();
    let files = git.changed_in_working_tree(&head, false).unwrap();
    let file = files.iter().find(|f| f.path == path).expect("listed");
    let spec = FileDiffSpec::between(Some(Rev::Commit(head)), Rev::WorkingTree, file);
    match git.load_file_diff(&spec).unwrap().content {
        Content::Text { old, new, .. } => (old, new),
        other => panic!("expected text, got {other:?}"),
    }
}

#[test]
fn a_working_tree_file_is_read_as_git_diff_reads_it() {
    let mut r = TestRepo::new();
    r.write("crlf.txt", b"a\nb\n");
    r.write("last.txt", b"x\n");
    r.write("conv.up", b"hello\n");
    r.write(".gitattributes", b"*.up diff=upper\n");
    r.git(&["config", "diff.upper.textconv", "tr a-z A-Z <"]);
    r.commit_all("base");
    r.git(&["config", "core.autocrlf", "true"]);
    // Line endings are converted as git does on adding the file.
    r.write("crlf.txt", b"a\r\nB\r\n");
    assert_eq!(texts(&r, "crlf.txt"), ("a\nb\n".into(), "a\nB\n".into()));
    r.git(&["config", "core.autocrlf", "false"]);
    // A missing last newline stays missing.
    r.write("last.txt", b"x\ny");
    assert_eq!(texts(&r, "last.txt").1, "x\ny");
    // The textconv filter applies to both sides.
    r.write("conv.up", b"hello\nworld\n");
    assert_eq!(
        texts(&r, "conv.up"),
        ("HELLO\n".into(), "HELLO\nWORLD\n".into())
    );
}

#[test]
fn a_binary_working_tree_file_has_its_size_on_disk() {
    let mut r = TestRepo::new();
    r.write("img.bin", b"\x00\x01");
    r.commit_all("base");
    r.write("img.bin", b"\x00\x01\x02\x03\x04");
    let git = Git::new(r.path());
    let head = Oid::from_hex(&r.git(&["rev-parse", "HEAD"])).unwrap();
    let files = git.changed_in_working_tree(&head, false).unwrap();
    let spec = FileDiffSpec::between(Some(Rev::Commit(head)), Rev::WorkingTree, &files[0]);
    match git.load_file_diff(&spec).unwrap().content {
        Content::Binary { old_size, new_size } => {
            assert_eq!((old_size, new_size), (Some(2), Some(5)))
        }
        other => panic!("expected binary, got {other:?}"),
    }
}
