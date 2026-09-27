//! The history pane's list, from git's log of a file and its blame, in throwaway repositories.

mod common;

use common::TestRepo;
use parterre_core::Oid;
use parterre_core::blame::{BlameOptions, BlameSpec, Moves};
use parterre_core::file_diff::Rev;
use parterre_core::file_history::{FileHistory, Source};
use parterre_core::git::{Cancel, Git};

fn at(hash: &str, path: &str) -> BlameSpec {
    BlameSpec {
        rev: Rev::Commit(Oid::from_hex(hash).unwrap()),
        path: path.into(),
    }
}

fn working_tree(path: &str) -> BlameSpec {
    BlameSpec {
        rev: Rev::WorkingTree,
        path: path.into(),
    }
}

fn history_with(r: &TestRepo, spec: &BlameSpec, options: BlameOptions) -> FileHistory {
    let git = Git::new(r.path());
    let blame = git.blame(spec, options).unwrap();
    let log = git.file_log(spec, &Cancel::new()).unwrap();
    FileHistory::new(log, &blame, &r.load())
}

fn history(r: &TestRepo, spec: &BlameSpec) -> FileHistory {
    history_with(r, spec, BlameOptions::default())
}

/// Each row's subject ("wt" for the working tree changes), and a mark: `*` if it owns lines,
/// `[path]` if only the blame names it.
fn rows(h: &FileHistory) -> Vec<String> {
    h.rows
        .iter()
        .map(|r| {
            let mut s = match r.commit {
                Some(_) => r.subject.clone(),
                None => "wt".into(),
            };
            if r.owns_lines {
                s.push('*');
            }
            if let Source::Blame { paths } = &r.source {
                s.push_str(&format!(" [{}]", paths.join(", ")));
            }
            s
        })
        .collect()
}

fn oid(hash: &str) -> Option<Oid> {
    Oid::from_hex(hash)
}

#[test]
fn a_linear_history_lists_the_commits_that_changed_the_file() {
    let mut r = TestRepo::new();
    r.write("f.txt", b"one\ntwo\nthree\n");
    let c1 = r.commit_all("c1");
    r.write("f.txt", b"one\nTWO\nthree\n");
    let c2 = r.commit_all("c2");
    r.write("g.txt", b"other\n");
    r.commit_all("c3");
    r.write("f.txt", b"one\nTWO\nTHREE\n");
    let c4 = r.commit_all("c4");
    r.write("f.txt", b"one\nTWO\nTHREE\nfour\n");
    let c5 = r.commit_all("c5");

    let h = history(&r, &at(&c5, "f.txt"));
    assert_eq!(rows(&h), ["c5*", "c4*", "c2*", "c1*"]);
    assert_eq!(h.path, "f.txt");
    // Parents as the file's history has them: c3 didn't change the file.
    let row = &h.rows[h.row_of(oid(&c4)).unwrap()];
    assert_eq!(row.parents, [oid(&c2).unwrap()]);
    assert!(h.rows.iter().all(|r| r.snapshot.is_some()));
    assert_eq!(h.row_of(oid(&c1)), Some(3));
    // Dated as the snapshot's log dates them.
    let repo = r.load();
    for row in &h.rows {
        let ix = row.snapshot.unwrap();
        assert_eq!(row.author_date, repo.commit(ix).author_date);
    }
    let g = h.graph();
    assert_eq!((g.len(), g.lanes), (4, 1));
}

#[test]
fn commits_from_before_a_rename_come_from_the_blame_with_their_old_path() {
    let mut r = TestRepo::new();
    r.write("a.txt", b"one\ntwo\nthree\nfour\nfive\nsix\n");
    r.commit_all("c1");
    r.write("a.txt", b"ONE\ntwo\nthree\nfour\nfive\nsix\n");
    let c2 = r.commit_all("c2");
    r.write("other.txt", b"x\n");
    r.commit_all("c3 elsewhere");
    r.git(&["mv", "a.txt", "b.txt"]);
    r.write("b.txt", b"ONE\nTWO\nthree\nfour\nfive\nsix\n");
    let c4 = r.commit_all("c4 rename");
    r.write("b.txt", b"ONE\nTWO\nTHREE\nfour\nfive\nsix\n");
    let c5 = r.commit_all("c5");

    let h = history(&r, &at(&c5, "b.txt"));
    // The log stops at the rename; the blame names c2 and c1, which are older.
    assert_eq!(
        rows(&h),
        ["c5*", "c4 rename*", "c2* [a.txt]", "c1* [a.txt]"]
    );
    let c2_row = &h.rows[h.row_of(oid(&c2)).unwrap()];
    assert!(c2_row.parents.is_empty());
    assert!(c2_row.snapshot.is_some());
    assert_eq!(h.rows[h.row_of(oid(&c4)).unwrap()].source, Source::Log);
    // Every line's commit has a row.
    let blame = Git::new(r.path())
        .blame(&at(&c5, "b.txt"), BlameOptions::default())
        .unwrap();
    assert!(blame.origins.iter().all(|o| h.row_of(o.commit).is_some()));
    // The blame's rows stand alone in the graph.
    let g = h.graph().rows(0..4);
    assert_eq!(g[1].lower, []);
    assert_eq!((g[2].upper.clone(), g[2].lower.clone()), (vec![], vec![]));
}

#[test]
fn lines_moved_from_another_file_bring_its_commit() {
    let mut r = TestRepo::new();
    // git follows a line from another file only if it has 40 letters or digits or more.
    let long = "this line is long enough for git blame to follow it across files";
    r.write("src.txt", format!("{long}\nrest\n").as_bytes());
    r.commit_all("c1 src");
    r.write("a.txt", format!("a1\n{long}\n").as_bytes());
    r.write("src.txt", b"rest\n");
    let c2 = r.commit_all("c2 move");

    let options = BlameOptions {
        ignore_whitespace: false,
        moves: Moves::AcrossFiles,
    };
    let h = history_with(&r, &at(&c2, "a.txt"), options);
    assert_eq!(rows(&h), ["c2 move*", "c1 src* [src.txt]"]);
    // Without move detection the line is c2's, and c1 has nothing to do with the file.
    let h = history(&r, &at(&c2, "a.txt"));
    assert_eq!(rows(&h), ["c2 move*"]);
}

#[test]
fn a_merge_that_took_one_side_is_not_listed_but_an_evil_merge_is() {
    let mut r = TestRepo::new();
    r.write("f.txt", b"1\n2\n3\n4\n");
    r.commit_all("c1");
    r.branch("side");
    r.write("f.txt", b"1\nS\n3\n4\n");
    r.commit_all("s1");
    r.checkout("main");
    r.write("g.txt", b"g\n");
    r.commit_all("m1 other file");
    let m2 = r.merge("side", "M2 takes side");
    r.branch("side2");
    r.write("f.txt", b"1\nS\nT\n4\n");
    r.commit_all("t1");
    r.checkout("main");
    r.write("g.txt", b"g2\n");
    r.commit_all("m3 other file");
    // An evil merge: it changes the file beyond what either side had.
    r.git(&["merge", "-q", "--no-ff", "--no-commit", "side2"]);
    r.write("f.txt", b"1\nS\nT\nEVIL\n");
    r.git(&["add", "f.txt"]);
    let m4 = r.commit("M4 evil");

    let h = history(&r, &at(&m4, "f.txt"));
    assert_eq!(h.row_of(oid(&m2)), None);
    assert_eq!(rows(&h), ["M4 evil*", "t1*", "s1*", "c1*"]);
    let evil = &h.rows[0];
    assert_eq!(evil.parents.len(), 2);
    assert!(h.graph().rows(0..1)[0].merge);
}

#[test]
fn an_edit_in_the_working_tree_is_a_row_on_top_that_owns_its_lines() {
    let mut r = TestRepo::new();
    r.write("f.txt", b"one\ntwo\n");
    r.commit_all("c1");
    r.write("f.txt", b"one\nTWO\n");
    let h = history(&r, &working_tree("f.txt"));
    assert_eq!(rows(&h), ["wt*", "c1*"]);
    assert_eq!(h.row_of(None), Some(0));
    assert_eq!(h.rows[0].source, Source::WorkingTree);
    // It leads to the newest commit in the graph column.
    assert_eq!(h.graph().rows(0..1)[0].lower.len(), 1);
}

#[test]
fn an_edit_that_only_deletes_lines_still_has_its_row() {
    let mut r = TestRepo::new();
    r.write("f.txt", b"one\ntwo\n");
    r.commit_all("c1");
    r.write("f.txt", b"one\n");
    let h = history(&r, &working_tree("f.txt"));
    assert_eq!(rows(&h), ["wt", "c1*"]);
    // No change, no row.
    r.write("f.txt", b"one\ntwo\n");
    let h = history(&r, &working_tree("f.txt"));
    assert_eq!(rows(&h), ["c1*"]);
}

#[test]
fn a_staged_rename_lists_the_history_under_the_old_path() {
    let mut r = TestRepo::new();
    r.write("e.txt", b"one\ntwo\n");
    r.commit_all("c1");
    r.write("e.txt", b"one\nTWO\n");
    r.commit_all("c2");
    r.git(&["mv", "e.txt", "f.txt"]);

    let git = Git::new(r.path());
    let log = git
        .file_log(&working_tree("f.txt"), &Cancel::new())
        .unwrap();
    assert_eq!(log.path, "e.txt");
    // It differs from HEAD, where it has another name.
    assert!(log.working_tree_changed);
    let h = history(&r, &working_tree("f.txt"));
    assert_eq!(rows(&h), ["wt", "c2*", "c1*"]);
}

#[test]
fn during_a_merge_the_other_sides_commits_are_listed_even_outside_the_snapshot() {
    let mut r = TestRepo::new();
    r.write("f.txt", b"1\n2\n");
    r.commit_all("c1");
    r.branch("side");
    r.write("f.txt", b"1\nS\n");
    let s1 = r.commit_all("s1\n\nWhy the side changed it.");
    r.checkout("main");
    r.write("g.txt", b"g\n");
    r.commit_all("m1 other file");
    r.git(&["merge", "-q", "--no-ff", "--no-commit", "side"]);
    // Only MERGE_HEAD reaches s1 now, and the snapshot doesn't load it.
    r.git(&["branch", "-q", "-D", "side"]);

    let h = history(&r, &working_tree("f.txt"));
    assert_eq!(rows(&h), ["wt", "s1*", "c1*"]);
    let s1_row = &h.rows[h.row_of(oid(&s1)).unwrap()];
    assert_eq!(s1_row.source, Source::Log);
    assert_eq!(s1_row.snapshot, None);
    assert!(h.rows[2].snapshot.is_some());
    // Its whole message, for the row's tooltip, comes by its hash all the same.
    let details = Git::new(r.path()).details(&oid(&s1).unwrap()).unwrap();
    assert_eq!(details.message, "s1\n\nWhy the side changed it.");
}

#[test]
fn nothing_newer_than_the_blamed_revision_is_listed() {
    let mut r = TestRepo::new();
    r.write("f.txt", b"one\n");
    r.commit_all("c1");
    r.write("f.txt", b"one\ntwo\n");
    let c2 = r.commit_all("c2");
    r.write("f.txt", b"one\ntwo\nthree\n");
    r.commit_all("c3");
    let h = history(&r, &at(&c2, "f.txt"));
    assert_eq!(rows(&h), ["c2*", "c1*"]);
}

/// git blocks opening a mailmap that is a named pipe nobody writes, so the listing runs until
/// it is killed.
#[cfg(target_os = "linux")]
#[test]
fn cancelling_kills_git() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use parterre_core::git::GitError;

    let mut r = TestRepo::new();
    r.write("f.txt", b"one\n");
    let c1 = r.commit_all("c1");
    let fifo = r.path().join(".git").join("blocking-mailmap");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());
    r.git(&["config", "log.mailmap", "true"]);
    r.git(&["config", "mailmap.file", fifo.to_str().unwrap()]);

    // The processes running in the repository, by the `-C <dir>` parterre gives git.
    let dir = r.path().to_str().unwrap().to_owned();
    let running = move || -> Vec<u32> {
        let mut pids = Vec::new();
        for entry in std::fs::read_dir("/proc").unwrap().flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else {
                continue;
            };
            let args: Vec<&[u8]> = cmdline.split(|&b| b == 0).collect();
            if args.contains(&dir.as_bytes()) && args.contains(&b"log".as_slice()) {
                pids.push(pid);
            }
        }
        pids
    };

    let cancel = Cancel::new();
    let (tx, rx) = mpsc::channel();
    let (git, spec, c) = (Git::new(r.path()), at(&c1, "f.txt"), cancel.clone());
    std::thread::spawn(move || {
        let _ = tx.send(git.file_log(&spec, &c));
    });
    let start = Instant::now();
    while running().is_empty() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "git never started"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(rx.try_recv().is_err(), "git doesn't finish by itself");
    cancel.cancel();
    // Killed and waited for: gone as soon as `cancel` returns.
    assert_eq!(running(), Vec::<u32>::new());
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Err(GitError::Cancelled)) => {}
        other => {
            // Let a git still blocked go before failing.
            let _ = std::fs::write(&fifo, b"");
            panic!("expected Cancelled, got {other:?}");
        }
    }
    // A cancelled handle stops what comes after it at once.
    let again = Git::new(r.path()).file_log(&at(&c1, "f.txt"), &cancel);
    assert!(matches!(again, Err(GitError::Cancelled)));
}
