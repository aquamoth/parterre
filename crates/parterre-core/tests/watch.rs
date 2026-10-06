//! Noticing ref changes (automatic reload) against real repositories.

mod common;

use common::TestRepo;
use parterre_core::watch::RefStorage;

/// Runs `change` and reports whether the fingerprint moved.
fn changes(storage: &RefStorage, change: impl FnOnce()) -> bool {
    let before = storage.fingerprint();
    change();
    storage.fingerprint() != before
}

#[test]
fn fingerprint_follows_ref_changes() {
    let mut r = TestRepo::new();
    r.commit("a");
    let storage = RefStorage::locate(r.path()).expect("locate");

    assert!(!changes(&storage, || {
        r.git(&["status"]);
        r.git(&["log", "--oneline"]);
        r.load();
    }));
    assert!(changes(&storage, || {
        r.commit("b");
    }));
    assert!(changes(&storage, || {
        r.git(&["branch", "topic"]);
    }));
    assert!(changes(&storage, || {
        r.git(&["tag", "v1"]);
    }));
    assert!(changes(&storage, || r.checkout("topic")));
    assert!(changes(&storage, || {
        r.git(&["pack-refs", "--all"]);
    }));
    // A packed ref, moved and then deleted.
    assert!(changes(&storage, || {
        r.git(&["update-ref", "refs/tags/v1", "HEAD~1"]);
    }));
    assert!(changes(&storage, || {
        r.git(&["branch", "-D", "main"]);
    }));
}

#[test]
fn same_refs_tells_real_changes_from_rewrites() {
    let mut r = TestRepo::new();
    r.commit("a");
    r.git(&["branch", "topic"]);
    let before = r.load();

    r.git(&["pack-refs", "--all"]);
    assert!(before.same_refs(&r.load()), "packing moves no ref");

    r.checkout("topic");
    assert!(!before.same_refs(&r.load()), "HEAD moved to another branch");
    r.checkout("main");

    r.git(&["update-ref", "refs/heads/topic", "HEAD"]);
    r.commit("b");
    assert!(!before.same_refs(&r.load()), "main moved");
}

#[test]
fn linked_worktree_sees_its_own_head_and_shared_refs() {
    let mut r = TestRepo::new();
    r.commit("a");
    let wt = r.dir.path().join("wt");
    r.git(&["worktree", "add", "-q", "-b", "side", wt.to_str().unwrap()]);
    let storage = RefStorage::locate(&wt).expect("locate");

    // HEAD of the worktree lives in its own git dir.
    assert!(changes(&storage, || {
        r.git(&["-C", wt.to_str().unwrap(), "checkout", "-q", "--detach"]);
    }));
    // Branches are shared with the main working tree.
    assert!(changes(&storage, || {
        r.commit("b");
    }));
}

#[test]
fn fingerprint_follows_other_worktrees() {
    let mut r = TestRepo::new();
    r.commit("a");
    r.commit("b");
    let others = tempfile::tempdir().expect("tempdir");
    let wt = others.path().join("wt");
    let wt_arg = wt.to_string_lossy().into_owned();
    let storage = RefStorage::locate(r.path()).expect("locate");
    let in_wt = |args: &[&str]| {
        let out = std::process::Command::new(parterre_core::git::program())
            .current_dir(&wt)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };

    assert!(changes(&storage, || {
        r.git(&["worktree", "add", "-q", "--detach", &wt_arg, "HEAD~1"]);
    }));
    assert!(!changes(&storage, || {
        std::fs::write(wt.join("untracked"), "x").expect("write");
        in_wt(&["status"]);
    }));
    assert!(changes(&storage, || in_wt(&[
        "checkout", "-q", "--detach", "main"
    ])));
    assert!(changes(&storage, || {
        in_wt(&["commit", "-q", "--allow-empty", "-m", "c"]);
    }));
    assert!(changes(&storage, || {
        r.git(&["worktree", "lock", &wt_arg]);
    }));
    assert!(changes(&storage, || {
        r.git(&["worktree", "unlock", &wt_arg]);
    }));
    assert!(changes(&storage, || {
        std::fs::remove_dir_all(&wt).expect("delete the worktree");
    }));
    assert!(changes(&storage, || {
        r.git(&["worktree", "prune"]);
    }));
    // The same from inside a linked worktree: a checkout in the main one.
    r.git(&["worktree", "add", "-q", "--detach", &wt_arg]);
    let linked = RefStorage::locate(&wt).expect("locate");
    assert!(changes(&linked, || r.checkout("HEAD~1")));
}
