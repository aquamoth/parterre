//! Cloning into a new folder (#358), from a local repository standing in for a remote.
mod common;

use common::TestRepo;
use parterre_core::branches::{Action, Branches, Outcome};
use parterre_core::clone::Cloning;
use parterre_core::remote::Live;
use parterre_util::CancelTree;

fn cloning(url: &str, parent: &std::path::Path, name: &str) -> Cloning {
    Cloning {
        url: url.to_owned(),
        parent: parent.to_owned(),
        name: name.to_owned(),
    }
}

fn execute(cloning: Cloning, live: &Live) -> Outcome {
    Branches::new(cloning.path())
        .with_live(live.clone())
        .execute(
            Action::Clone(Box::new(cloning)),
            None,
            &CancelTree::default(),
        )
}

#[test]
fn a_clone_gets_the_history_and_origin_and_streams_gits_output() {
    let mut source = TestRepo::new();
    source.write("a", b"a\n");
    let tip = source.commit_all("first");
    let parent = tempfile::tempdir().unwrap();
    let url = source.path().to_str().unwrap();
    let live = Live::default();
    let outcome = execute(cloning(url, parent.path(), "copy"), &live);
    assert!(matches!(outcome, Outcome::Done(_)), "{outcome:?}");
    let copy = parent.path().join("copy");
    let repo = parterre_core::git::load_repo(&copy).unwrap();
    let head = repo.resolve("HEAD").map(|c| repo.commit(c).oid.to_hex());
    assert_eq!(head.as_deref(), Some(tip.as_str()));
    let origin = parterre_core::git::Git::new(&copy)
        .run(&["remote", "get-url", "origin"])
        .unwrap();
    assert_eq!(origin.trim(), url);
    let steps = live.steps();
    assert_eq!(steps.len(), 1);
    assert!(
        steps[0].0.starts_with("git clone --progress -- "),
        "{steps:?}"
    );
}

#[test]
fn a_failed_clone_leaves_no_folder_and_says_why() {
    let parent = tempfile::tempdir().unwrap();
    let missing = parent.path().join("nowhere");
    let outcome = execute(
        cloning(missing.to_str().unwrap(), parent.path(), "copy"),
        &Live::default(),
    );
    let Outcome::Failed { error, report } = outcome else {
        panic!("{outcome:?}");
    };
    assert!(error.to_string().contains("credential helper"), "{error}");
    assert_eq!(report.steps.len(), 1);
    assert!(!parent.path().join("copy").exists());
}

#[test]
fn an_existing_folder_is_refused_before_git_runs() {
    let source = TestRepo::new();
    let parent = tempfile::tempdir().unwrap();
    std::fs::create_dir(parent.path().join("taken")).unwrap();
    std::fs::write(parent.path().join("taken").join("keep"), b"mine").unwrap();
    let outcome = execute(
        cloning(source.path().to_str().unwrap(), parent.path(), "taken"),
        &Live::default(),
    );
    let Outcome::Failed { error, report } = outcome else {
        panic!("{outcome:?}");
    };
    assert!(error.to_string().contains("already exists"), "{error}");
    assert!(report.steps.is_empty());
    assert!(parent.path().join("taken").join("keep").exists());
}
