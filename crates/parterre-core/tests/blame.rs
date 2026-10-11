mod common;

use common::TestRepo;
use parterre_core::Oid;
use parterre_core::blame::{Blame, BlameOptions, BlameSpec, Moves};
use parterre_core::file_diff::Rev;
use parterre_core::git::Git;

fn at(hash: &str, path: &str) -> BlameSpec {
    BlameSpec {
        rev: Rev::Commit(Oid::from_hex(hash).unwrap()),
        path: path.into(),
    }
}

fn blame(r: &TestRepo, spec: &BlameSpec, options: BlameOptions) -> Blame {
    Git::new(r.path()).blame(spec, options).unwrap()
}

/// Each line's text and the subject of the commit it comes from ("-" for none).
fn lines(b: &Blame) -> Vec<(String, String)> {
    b.lines
        .iter()
        .map(|l| {
            let o = &b.origins[l.origin];
            let who = if o.commit.is_some() {
                o.summary.clone()
            } else {
                "-".into()
            };
            (l.raw.clone(), who)
        })
        .collect()
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[test]
fn lines_belong_to_the_commits_that_last_changed_them_across_a_rename() {
    let mut r = TestRepo::new();
    r.write("a.txt", b"one\ntwo\nthree\n");
    let first = r.commit_all("first");
    r.git(&["mv", "a.txt", "b.txt"]);
    r.write("b.txt", b"one\n2\nthree\n");
    let second = r.commit_all("second");

    let b = blame(&r, &at(&second, "b.txt"), BlameOptions::default());
    assert_eq!(
        lines(&b),
        pairs(&[("one", "first"), ("2", "second"), ("three", "first")])
    );
    // The root commit is an origin like any other, not a boundary.
    let first_origin = &b.origins[b.lines[0].origin];
    assert!(!first_origin.boundary);
    assert_eq!(first_origin.path, "a.txt");
    assert_eq!(first_origin.commit, Oid::from_hex(&first));

    let second_origin = &b.origins[b.lines[1].origin];
    let before = second_origin.previous_blame().unwrap();
    assert_eq!(before, at(&first, "a.txt"));
    let b = blame(&r, &before, BlameOptions::default());
    assert_eq!(
        lines(&b),
        pairs(&[("one", "first"), ("two", "first"), ("three", "first")])
    );
}

#[test]
fn the_working_tree_has_lines_of_no_commit_yet() {
    let mut r = TestRepo::new();
    r.write("a.txt", b"one\ntwo\n");
    r.commit_all("first");
    r.write("a.txt", b"one\nTWO\n");
    let spec = BlameSpec {
        rev: Rev::WorkingTree,
        path: "a.txt".into(),
    };
    let b = blame(&r, &spec, BlameOptions::default());
    assert_eq!(lines(&b), pairs(&[("one", "first"), ("TWO", "-")]));
    let changes = b.origins[b.lines[1].origin].changes().unwrap();
    assert_eq!(changes.new.unwrap().rev, Rev::WorkingTree);
    let head = r.git(&["rev-parse", "HEAD"]);
    assert_eq!(
        changes.old.unwrap().rev,
        Rev::Commit(Oid::from_hex(&head).unwrap())
    );
}

#[test]
fn whitespace_and_moved_lines_can_be_ignored() {
    let mut r = TestRepo::new();
    // git follows a moved line only if it has 20 letters or digits or more.
    let long = "beta is long enough to be followed";
    r.write(
        "a.txt",
        format!("alpha\n{long}\ngamma\ndelta\nepsilon\n").as_bytes(),
    );
    r.commit_all("first");
    // Re-indents one line and moves another to the end.
    r.write(
        "a.txt",
        format!("  alpha\ngamma\ndelta\nepsilon\n{long}\n").as_bytes(),
    );
    let second = r.commit_all("second");
    let spec = at(&second, "a.txt");

    let plain = blame(&r, &spec, BlameOptions::default());
    assert_eq!(plain.lines[0].raw, "  alpha");
    assert_eq!(lines(&plain)[0].1, "second");
    assert_eq!(lines(&plain)[4].1, "second");

    let options = BlameOptions {
        ignore_whitespace: true,
        moves: Moves::WithinFile,
    };
    let b = blame(&r, &spec, options);
    assert_eq!(lines(&b)[0].1, "first");
    assert_eq!(lines(&b)[4], (long.into(), "first".into()));
    assert_eq!(b.lines[4].orig_line, 1);
}

#[test]
fn a_file_with_odd_characters_in_its_name_is_blamed() {
    let mut r = TestRepo::new();
    // Straight into the index: Windows can't hold a `"` in a file name.
    r.stage("we \"ird\" é.txt", b"x\n");
    let c = r.commit("odd");
    let b = blame(&r, &at(&c, "we \"ird\" é.txt"), BlameOptions::default());
    assert_eq!(b.origins[0].path, "we \"ird\" é.txt");
}
