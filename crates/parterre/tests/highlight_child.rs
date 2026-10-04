//! Syntax colour in a child process (#209): a grammar that aborts, or a parse that never
//! ends, costs the colours and nothing else.

use parterre_highlight::{BUDGET, Engine, Kind, Language, in_child};
use parterre_util::Cancel;
use std::path::PathBuf;
use std::time::Duration;

fn exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_parterre"))
}

#[test]
fn the_child_highlights_like_the_process() {
    let text = "/// d\nfn main() { let s = \"x\"; }\n";
    let cancel = Cancel::new();
    let spans = Engine::Child(exe()).highlight(Language::Rust, text, &cancel);
    assert_eq!(
        spans,
        Engine::InProcess.highlight(Language::Rust, text, &cancel)
    );
    assert!(spans[1].contains(&(0..2, Kind::Keyword)), "{spans:?}");
}

#[test]
fn a_grammar_that_aborts_costs_only_the_colours() {
    // tree-sitter-yaml 0.7.2's scanner overruns the runtime's serialization buffer at this
    // depth, and the runtime's assertion aborts the process: the child's, not ours. Once
    // upstream bounds its buffer the child will answer instead; this process lives on either
    // way.
    let text = "- ".repeat(254)
        + "x
";
    let cancel = Cancel::new();
    let answer = in_child(&exe(), Language::Yaml, &text, &cancel, BUDGET);
    assert!(
        answer.as_ref().is_none_or(|spans| spans.len() == 1),
        "{answer:?}"
    );
}

#[test]
fn cancelling_from_another_thread_kills_the_child_at_once() {
    let text = "[".repeat(1 << 20);
    let cancel = Cancel::new();
    let stop = cancel.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        stop.cancel();
    });
    let start = std::time::Instant::now();
    assert_eq!(
        in_child(&exe(), Language::Json, &text, &cancel, BUDGET),
        None
    );
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
    canceller.join().unwrap();
}

#[test]
fn over_budget_or_cancelled_is_killed() {
    let text = "[".repeat(1 << 20);
    let cancel = Cancel::new();
    assert_eq!(
        in_child(&exe(), Language::Json, &text, &cancel, Duration::ZERO),
        None
    );
    cancel.cancel();
    assert_eq!(
        in_child(&exe(), Language::Json, "[1]\n", &cancel, BUDGET),
        None
    );
}

#[test]
fn an_unknown_language_or_a_wrong_program_gives_nothing() {
    let cancel = Cancel::new();
    let missing = PathBuf::from("/nonexistent/parterre");
    assert_eq!(
        in_child(&missing, Language::Rust, "fn f() {}\n", &cancel, BUDGET),
        None
    );
    let status = std::process::Command::new(exe())
        .args(["--highlight", "cobol"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(2));
}
