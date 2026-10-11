mod common;

use std::time::{Duration, Instant};

use common::{TestRepo, git_fails, read_text};
use parterre_core::git::Git;
use parterre_core::merge_tool::{self, Configured, Tool};

fn detect(r: &TestRepo) -> merge_tool::Detected {
    merge_tool::detect(&Git::new(r.path())).unwrap()
}

#[test]
fn a_user_defined_tool_is_usable() {
    let r = TestRepo::new();
    r.git(&["config", "mergetool.mine.cmd", "true"]);
    r.git(&["config", "merge.tool", "mine"]);
    let d = detect(&r);
    assert_eq!(d.configured, Configured::Usable("mine".into()));
    assert_eq!(
        d.usable(),
        Some(Tool {
            name: "mine".into(),
            path: None
        })
    );
}

#[test]
fn a_gui_tool_beats_the_plain_one() {
    let r = TestRepo::new();
    r.git(&["config", "mergetool.mine.cmd", "true"]);
    r.git(&["config", "merge.tool", "vimdiff"]);
    r.git(&["config", "merge.guitool", "mine"]);
    assert_eq!(detect(&r).configured, Configured::Usable("mine".into()));
}

#[test]
fn a_terminal_tool_is_not_usable_from_parterre() {
    let r = TestRepo::new();
    r.git(&["config", "merge.tool", "vimdiff"]);
    r.git(&["config", "mergetool.mine.cmd", "true"]);
    let d = detect(&r);
    assert_eq!(
        d.configured,
        Configured::Unusable {
            why: "vimdiff runs in a terminal".into(),
            terminal: true
        }
    );
    // The user's own tools are offered instead.
    assert!(d.installed.iter().any(|t| t.name == "mine"));
    assert!(d.installed.iter().all(|t| !t.name.starts_with("vimdiff")));
}

#[test]
fn an_unknown_tool_is_not_available() {
    let r = TestRepo::new();
    r.git(&["config", "merge.tool", "no-such-tool"]);
    assert_eq!(
        detect(&r).configured,
        Configured::Unusable {
            why: "no-such-tool is not available".into(),
            terminal: false
        }
    );
}

fn wait_resolved(r: &TestRepo, path: &str) {
    let start = Instant::now();
    while !r.git(&["ls-files", "--unmerged", "--", path]).is_empty() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the tool never finished"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn opening_a_file_runs_git_mergetool_and_git_stages_the_result() {
    let mut r = TestRepo::new();
    r.write("list.txt", b"one\ntwo\n");
    r.commit_all("Base");
    r.branch("topic");
    r.write("list.txt", b"one\nTWO (topic)\n");
    r.commit_all("Topic");
    r.checkout("main");
    r.write("list.txt", b"one\n2 (main)\n");
    r.commit_all("Main");
    git_fails(r.path(), &["merge", "-q", "topic"]);
    // A tool that takes their side, as a user resolving it would.
    r.git(&[
        "config",
        "mergetool.theirs.cmd",
        "cp \"$REMOTE\" \"$MERGED\"",
    ]);
    r.git(&["config", "mergetool.theirs.trustExitCode", "true"]);
    r.git(&["config", "mergetool.keepBackup", "false"]);
    let tool = Tool {
        name: "theirs".into(),
        path: None,
    };
    merge_tool::open(r.path(), &tool, "list.txt").unwrap();
    wait_resolved(&r, "list.txt");
    assert_eq!(read_text(&r.path().join("list.txt")), "one\nTWO (topic)\n");
}

#[test]
fn detecting_does_not_ask_git_tool_by_tool() {
    let r = TestRepo::new();
    let start = Instant::now();
    detect(&r);
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "took {:?}",
        start.elapsed()
    );
}
