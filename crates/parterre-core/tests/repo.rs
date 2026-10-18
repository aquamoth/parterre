mod common;

use common::{TestRepo, feature_merge, ix};
use parterre_core::git::{CommitNote, Git, load_repo};
use parterre_core::revgraph::{self, GraphOptions};
use parterre_core::{Head, RefKind};

#[test]
fn loads_commits_refs_and_head() {
    let r = feature_merge();
    r.git(&["tag", "-a", "-m", "annotated", "v1", "HEAD~1"]);
    let repo = r.load();
    assert_eq!(repo.commits.len(), 6);
    assert!(
        matches!(&repo.head, Head::Branch { name, target: Some(_) } if name == "refs/heads/main")
    );
    let names: Vec<(&str, RefKind, bool)> = repo
        .refs
        .iter()
        .map(|r| (r.name.as_str(), r.kind, r.annotated))
        .collect();
    assert!(names.contains(&("main", RefKind::LocalBranch, false)));
    assert!(names.contains(&("feature", RefKind::LocalBranch, false)));
    assert!(names.contains(&("v1", RefKind::Tag, true)));
    let merge = repo.head_commit().unwrap();
    assert_eq!(repo.commit(merge).parents.len(), 2);
    let tag = repo.refs.iter().find(|r| r.name == "v1").unwrap();
    assert_eq!(
        repo.commit(tag.target).subject,
        "C",
        "annotated tags are peeled"
    );
}

#[test]
fn reads_full_commit_messages_committers_and_notes() {
    let mut r = TestRepo::new();
    r.commit("Subject line\n\nBody paragraph\n\tsecond line\nNotes:");
    // Amended by someone else, later: the author stays, the committer changes.
    r.set_clock(5);
    r.git(&["config", "user.name", "Other"]);
    r.git(&["commit", "-q", "--amend", "--allow-empty", "--no-edit"]);
    r.git(&["notes", "add", "-m", "First note", "-m", "  indented"]);
    r.git(&["notes", "--ref=review", "add", "-m", "Looks good"]);
    r.git(&["config", "notes.displayRef", "refs/notes/review"]);
    let repo = r.load();
    let c = repo.head_commit().unwrap();
    assert_eq!(repo.commit(c).subject, "Subject line");
    let d = Git::new(r.path()).details(&repo.commit(c).oid).unwrap();
    assert_eq!(
        d.message,
        "Subject line\n\nBody paragraph\n\tsecond line\nNotes:"
    );
    assert_eq!(d.author_date, "2023-11-14 22:14:20 +0000");
    assert_eq!(d.committer_date, "2023-11-14 22:18:20 +0000");
    assert_eq!(
        (d.committer_name.as_str(), d.committer_email.as_str()),
        ("Other", "test@example.com")
    );
    let note = |heading: &str, text: &str| CommitNote {
        heading: heading.to_owned(),
        text: text.to_owned(),
    };
    assert_eq!(
        d.notes,
        [
            note("Notes", "First note\n\n  indented"),
            note("Notes (review)", "Looks good")
        ]
    );
}

#[test]
fn commits_without_notes_have_none() {
    let mut r = TestRepo::new();
    r.commit("Plain");
    let repo = r.load();
    let c = repo.head_commit().unwrap();
    let d = Git::new(r.path()).details(&repo.commit(c).oid).unwrap();
    assert_eq!(d.message, "Plain");
    assert!(d.notes.is_empty());
}

#[test]
fn tags_of_tags_are_peeled_to_their_commit() {
    let mut r = TestRepo::new();
    r.commit("A");
    r.git(&["tag", "-a", "-m", "one", "t1"]);
    r.git(&["tag", "-a", "-m", "two", "t2", "t1"]);
    r.commit("B");
    let repo = r.load();
    let t2 = repo
        .refs
        .iter()
        .find(|r| r.name == "t2")
        .expect("nested tag loaded");
    assert_eq!(repo.commit(t2.target).subject, "A");
    assert!(t2.annotated);
}

#[test]
fn separator_characters_in_subjects_and_names_are_harmless() {
    let mut r = TestRepo::new();
    r.git(&["config", "user.name", "Wei\x1fZhang"]);
    r.commit("subject with \x1e record and \x1f unit separators");
    r.commit("second");
    let repo = r.load();
    assert_eq!(repo.commits.len(), 2);
    let first = repo.commits.iter().find(|c| c.parents.is_empty()).unwrap();
    assert_eq!(
        first.subject,
        "subject with \x1e record and \x1f unit separators"
    );
    assert_eq!(first.author_name, "Wei\x1fZhang");
    assert!(first.commit_time > 0);
}

#[test]
fn empty_repository_loads_without_commits() {
    let r = TestRepo::new();
    let repo = r.load();
    assert!(repo.commits.is_empty());
    assert!(matches!(repo.head, Head::Branch { target: None, .. }));
    let g = revgraph::build(&repo, &GraphOptions::default());
    assert!(g.nodes.is_empty());
}

#[test]
fn loads_from_inside_the_git_directory() {
    let mut r = TestRepo::new();
    r.commit("A");
    let repo = load_repo(&r.path().join(".git")).expect("load from .git");
    assert_eq!(repo.commits.len(), 1);
}

#[test]
fn notes_are_not_walked() {
    let mut r = TestRepo::new();
    r.commit("A");
    r.git(&["notes", "add", "-m", "a note"]);
    let repo = r.load();
    assert_eq!(
        repo.commits.len(),
        1,
        "the notes commit is not part of the history"
    );
    assert!(
        repo.refs
            .iter()
            .all(|r| !r.full_name.starts_with("refs/notes/"))
    );
}

#[test]
fn hash_length_follows_git() {
    let mut r = TestRepo::new();
    r.commit("one");
    // Several refs, so that several commits are sampled.
    r.git(&["tag", "t1"]);
    r.branch("side");
    r.commit("two");
    r.git(&["tag", "t2"]);
    let head = r.commit("three");
    let repo = r.load();
    assert_eq!(repo.abbrev_len, r.git(&["log", "-1", "--format=%h"]).len());
    assert_eq!(repo.abbrev_len, 7);
    assert_eq!(
        repo.commit(ix(&repo, &head)).oid.short(repo.abbrev_len),
        head[..7]
    );

    r.git(&["config", "core.abbrev", "12"]);
    assert_eq!(r.load().abbrev_len, 12);
}

#[test]
fn hash_length_of_an_empty_repository_is_the_default() {
    let r = TestRepo::new();
    assert_eq!(r.load().abbrev_len, 7);
}
