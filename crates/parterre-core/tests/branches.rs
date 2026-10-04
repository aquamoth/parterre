//! Exercise the public branch-operation interface against real, disposable repositories.
mod common;

use common::TestRepo;
use parterre_core::Oid;
use parterre_core::branches::{Action, Branches, Catalog, Create, CreateDraft, Outcome, Warning};
use parterre_util::CancelTree;

fn oid(s: &str) -> Oid {
    Oid::from_hex(s).unwrap()
}
fn execute(r: &TestRepo, a: Action) -> Outcome {
    Branches::new(r.path()).execute(a, None, &CancelTree::default())
}
fn done(out: Outcome) {
    assert!(matches!(out, Outcome::Done(_)), "{out:?}");
}
fn warning(out: Outcome) -> Warning {
    match out {
        Outcome::Warning(w) => w,
        other => panic!("expected a warning: {other:?}"),
    }
}
fn deletion(r: &TestRepo, name: &str) -> Action {
    Action::Delete {
        name: name.into(),
        tip: oid(&r.git(&["rev-parse", &format!("refs/heads/{name}")])),
    }
}
fn unique_branch() -> (TestRepo, Action, Oid) {
    let mut r = TestRepo::new();
    r.commit("base");
    r.branch("topic");
    let tip = oid(&r.commit("only on topic"));
    r.checkout("main");
    let a = deletion(&r, "topic");
    (r, a, tip)
}

fn draft_repository() -> (TestRepo, Oid, Catalog) {
    let mut r = TestRepo::new();
    let start = oid(&r.commit("selected"));
    for remote in ["origin", "upstream"] {
        r.git(&[
            "remote",
            "add",
            remote,
            &format!("https://example.invalid/{remote}"),
        ]);
        r.git(&[
            "update-ref",
            &format!("refs/remotes/{remote}/topic"),
            &start.to_hex(),
        ]);
    }
    r.git(&[
        "update-ref",
        "refs/remotes/origin/team/feature",
        &start.to_hex(),
    ]);
    let catalog = Catalog::load(r.path()).unwrap();
    (r, start, catalog)
}

#[test]
fn selected_remote_follows_local_name_until_a_tracking_branch_is_chosen() {
    let (_r, start, catalog) = draft_repository();
    let mut draft = CreateDraft::new(&catalog, start, None);
    draft.set_name("my-work".into());
    draft.set_remote(Some("origin".into()));
    assert_eq!(draft.upstream().as_deref(), Some("origin/my-work"));
    draft.set_name("renamed-work".into());
    assert_eq!(draft.upstream().as_deref(), Some("origin/renamed-work"));
    // Explicitly picking the same value pins it too.
    draft.set_track_name(&catalog, "renamed-work".into());
    draft.set_name("local-only-name".into());
    assert_eq!(draft.upstream().as_deref(), Some("origin/renamed-work"));
    // Choosing another remote starts automatic naming again.
    draft.set_remote(Some("upstream".into()));
    assert_eq!(
        draft.upstream().as_deref(),
        Some("upstream/local-only-name")
    );
    draft.set_name("last-name".into());
    assert_eq!(draft.upstream().as_deref(), Some("upstream/last-name"));
}

#[test]
fn creation_defaults_to_first_remote_even_without_a_branch_at_the_selected_commit() {
    let (mut r, start, catalog) = draft_repository();
    let mut first = CreateDraft::new(&catalog, start, None);
    assert_eq!(first.remote(), Some("origin"));
    // An eligible branch on a later remote does not change the default remote.
    r.git(&["update-ref", "-d", "refs/remotes/origin/topic"]);
    r.git(&["update-ref", "-d", "refs/remotes/origin/team/feature"]);
    let catalog = Catalog::load(r.path()).unwrap();
    first = CreateDraft::new(&catalog, start, None);
    assert_eq!(first.remote(), Some("origin"));
    assert_eq!(first.track_name(), "");
    let selected = oid(&r.commit("no remote branches here"));
    let catalog = Catalog::load(r.path()).unwrap();
    let mut draft = CreateDraft::new(&catalog, selected, None);
    assert_eq!(draft.remote(), Some("origin"));
    draft.set_name("future-work".into());
    assert_eq!(draft.upstream().as_deref(), Some("origin/future-work"));
    r.git(&["remote", "remove", "origin"]);
    r.git(&["remote", "remove", "upstream"]);
    let catalog = Catalog::load(r.path()).unwrap();
    let mut draft = CreateDraft::new(&catalog, selected, None);
    assert!(draft.remote().is_none());
    draft.set_name("local-only".into());
    assert!(draft.upstream().is_none());
}

#[test]
fn tracking_reset_resumes_following_the_local_name_without_changing_remote() {
    let (_r, start, catalog) = draft_repository();
    let mut draft = CreateDraft::new(&catalog, start, Some("upstream/topic"));
    draft.set_name("my-local".into());
    assert!(draft.can_restore_track_name());
    draft.restore_track_name();
    assert_eq!(draft.remote(), Some("upstream"));
    assert_eq!(draft.name(), "my-local");
    assert_eq!(draft.upstream().as_deref(), Some("upstream/my-local"));
    assert!(!draft.can_restore_track_name());
    draft.set_name("next-local".into());
    assert_eq!(draft.upstream().as_deref(), Some("upstream/next-local"));
    draft.set_track_name(&catalog, "custom-upstream".into());
    draft.set_name("last-local".into());
    assert_eq!(
        draft.upstream().as_deref(),
        Some("upstream/custom-upstream")
    );
    draft.restore_track_name();
    assert_eq!(draft.upstream().as_deref(), Some("upstream/last-local"));
    // A same-value explicit selection still needs a reset to resume automatic naming.
    draft.set_track_name(&catalog, "last-local".into());
    assert!(draft.can_restore_track_name());
    draft.restore_track_name();
    draft.set_name("follows-again".into());
    assert_eq!(draft.upstream().as_deref(), Some("upstream/follows-again"));
    draft.set_remote(None);
    assert!(!draft.can_restore_track_name());
    draft.restore_track_name();
    assert!(draft.upstream().is_none());
    assert_eq!(draft.track_name(), "");
}

#[test]
fn none_remote_clears_tracking_and_reselection_generates_a_future_branch() {
    let (r, start, catalog) = draft_repository();
    let mut draft = CreateDraft::new(&catalog, start, None);
    draft.set_name("new-local".into());
    draft.set_remote(None);
    assert_eq!(draft.track_name(), "");
    draft.set_name("untracked".into());
    assert!(draft.upstream().is_none());
    assert!(draft.remote_branches(&catalog).is_empty());
    done(execute(
        &r,
        Action::Create(Create {
            start,
            name: draft.name().into(),
            track: draft.upstream(),
            switch: false,
        }),
    ));
    assert!(
        Catalog::load(r.path())
            .unwrap()
            .locals
            .iter()
            .find(|b| b.name == "untracked")
            .unwrap()
            .upstream
            .is_none()
    );
    draft.set_name("future-local".into());
    draft.set_remote(Some("origin".into()));
    done(execute(
        &r,
        Action::Create(Create {
            start,
            name: draft.name().into(),
            track: draft.upstream(),
            switch: false,
        }),
    ));
    assert_eq!(
        r.git(&["config", "branch.future-local.merge"]),
        "refs/heads/future-local"
    );
}

#[test]
fn remote_branch_choices_are_scoped_and_explicit_choices_keep_local_name_rules() {
    let (r, start, mut catalog) = draft_repository();
    let mut draft = CreateDraft::new(&catalog, start, None);
    draft.set_remote(Some("upstream".into()));
    assert_eq!(draft.remote_branches(&catalog), ["topic"]);
    draft.set_remote(Some("origin".into()));
    assert_eq!(draft.remote_branches(&catalog), ["team/feature", "topic"]);
    r.git(&["branch", "topic"]);
    r.git(&["branch", "--set-upstream-to=origin/topic", "topic"]);
    catalog = Catalog::load(r.path()).unwrap();
    draft.set_track_name(&catalog, "topic".into());
    assert_eq!(draft.name(), "topic-2");
    assert_eq!(catalog.trackers(&draft.upstream().unwrap()), ["topic"]);
    draft.set_name("my-local".into());
    draft.set_track_name(&catalog, "team/feature".into());
    assert_eq!(draft.name(), "my-local");
    draft.restore_suggested_name(&catalog);
    assert_eq!(draft.name(), "team/feature");
    assert_eq!(draft.upstream().as_deref(), Some("origin/team/feature"));
}

#[test]
fn remote_switch_pins_its_selected_branch_while_default_recommendation_can_follow_edits() {
    let (_r, start, catalog) = draft_repository();
    let mut explicit = CreateDraft::new(&catalog, start, Some("upstream/topic"));
    explicit.set_name("different-local".into());
    assert_eq!(explicit.upstream().as_deref(), Some("upstream/topic"));
    let mut suggested = CreateDraft::new(&catalog, start, None);
    assert_eq!(suggested.upstream().as_deref(), Some("origin/team/feature"));
    suggested.set_name("different-local".into());
    assert_eq!(
        suggested.upstream().as_deref(),
        Some("origin/different-local")
    );
}

#[test]
fn create_at_selected_commit_with_a_different_or_future_upstream() {
    let mut r = TestRepo::new();
    let start = oid(&r.commit("selected"));
    let remote = r.commit("upstream advanced");
    r.git(&["remote", "add", "origin", "https://example.invalid/r.git"]);
    r.git(&["update-ref", "refs/remotes/origin/topic", &remote]);
    for (name, track) in [
        ("local-one", "origin/topic"),
        ("local-two", "origin/not-created"),
    ] {
        done(execute(
            &r,
            Action::Create(Create {
                start,
                name: name.into(),
                track: Some(track.into()),
                switch: false,
            }),
        ));
        assert_eq!(r.git(&["rev-parse", name]), start.to_hex());
        assert_eq!(
            r.git(&["config", &format!("branch.{name}.remote")]),
            "origin"
        );
        assert_eq!(
            r.git(&["config", &format!("branch.{name}.merge")]),
            format!("refs/heads/{}", track.strip_prefix("origin/").unwrap())
        );
    }
    assert_eq!(r.git(&["branch", "--show-current"]), "main");
    assert!(
        !Catalog::load(r.path())
            .unwrap()
            .remotes
            .iter()
            .any(|b| b.name == "origin/not-created")
    );
}

#[test]
fn create_without_tracking_overrides_auto_setup_merge() {
    let mut r = TestRepo::new();
    let start = oid(&r.commit("base"));
    r.git(&["config", "branch.autoSetupMerge", "always"]);
    done(execute(
        &r,
        Action::Create(Create {
            start,
            name: "new".into(),
            track: None,
            switch: false,
        }),
    ));
    assert!(
        Catalog::load(r.path())
            .unwrap()
            .locals
            .iter()
            .find(|b| b.name == "new")
            .unwrap()
            .upstream
            .is_none()
    );
}

#[test]
fn combined_create_and_switch_preserves_changes_and_creates_nothing_on_refusal() {
    let mut r = TestRepo::new();
    r.write("file", b"base");
    let start = oid(&r.commit_all("base"));
    r.write("file", b"different");
    r.commit_all("main advances");
    r.write("file", b"dirty");
    r.git(&["remote", "add", "origin", "https://example.invalid/r.git"]);
    let out = execute(
        &r,
        Action::Create(Create {
            start,
            name: "new".into(),
            track: Some("origin/new".into()),
            switch: true,
        }),
    );
    assert!(matches!(out, Outcome::Failed { .. }), "{out:?}");
    let c = Catalog::load(r.path()).unwrap();
    assert!(!c.locals.iter().any(|b| b.name == "new"));
    assert_eq!(c.current.as_deref(), Some("main"));
    assert_eq!(std::fs::read(r.path().join("file")).unwrap(), b"dirty");
}

#[test]
fn reject_collisions_invalid_names_and_unknown_remotes_before_creating() {
    let mut r = TestRepo::new();
    let start = oid(&r.commit("base"));
    r.git(&["branch", "existing/path"]);
    for (name, track) in [
        ("main", None),
        ("existing", None),
        ("main/nested", None),
        ("HEAD", None),
        ("bad name", None),
        ("@{-1}", None),
        ("valid", Some("missing/topic")),
    ] {
        let out = execute(
            &r,
            Action::Create(Create {
                start,
                name: name.into(),
                track: track.map(str::to_owned),
                switch: false,
            }),
        );
        assert!(matches!(out, Outcome::Failed { .. }), "{name}: {out:?}");
    }
    assert_eq!(Catalog::load(r.path()).unwrap().locals.len(), 2);
}

#[test]
fn longest_remote_prefix_and_duplicate_tracking_use_upstream_relationships() {
    let mut r = TestRepo::new();
    let start = oid(&r.commit("base"));
    r.git(&["remote", "add", "team", "https://example.invalid/a"]);
    // Existing configurations can have overlapping remote names. Recent Git refuses
    // creating that overlap with `remote add`, so reproduce the older config directly.
    r.git(&["config", "remote.team/sub.url", "https://example.invalid/b"]);
    r.git(&[
        "config",
        "remote.team/sub.fetch",
        "+refs/heads/*:refs/remotes/team/sub/*",
    ]);
    for name in ["different-name", "another-name"] {
        done(execute(
            &r,
            Action::Create(Create {
                start,
                name: name.into(),
                track: Some("team/sub/topic".into()),
                switch: false,
            }),
        ));
    }
    let c = Catalog::load(r.path()).unwrap();
    assert_eq!(
        c.trackers("team/sub/topic"),
        ["another-name", "different-name"]
    );
    assert_eq!(
        c.tracking_parts("team/sub/topic"),
        Some(("team/sub", "topic"))
    );
    assert_eq!(
        c.suggested_name("team/sub/different-name"),
        "different-name-2"
    );
    assert!(c.suggested_name("team/sub/bad name").is_empty());
}

#[test]
fn clean_switch_and_create_and_switch_use_the_open_worktree() {
    let mut r = TestRepo::new();
    let start = oid(&r.commit("base"));
    r.git(&["branch", "topic"]);
    done(execute(&r, Action::Switch("topic".into())));
    assert_eq!(r.git(&["branch", "--show-current"]), "topic");
    done(execute(
        &r,
        Action::Create(Create {
            start,
            name: "new".into(),
            track: None,
            switch: true,
        }),
    ));
    assert_eq!(r.git(&["branch", "--show-current"]), "new");
}

#[test]
fn occupied_current_and_other_worktree_branches_are_not_deleted_or_switched_to() {
    let mut r = TestRepo::new();
    r.commit("base");
    r.git(&["branch", "topic"]);
    let wt = tempfile::tempdir().unwrap();
    let path = wt.path().join("linked");
    r.git(&["worktree", "add", path.to_str().unwrap(), "topic"]);
    for a in [
        deletion(&r, "main"),
        deletion(&r, "topic"),
        Action::Switch("topic".into()),
    ] {
        assert!(matches!(execute(&r, a), Outcome::Failed { .. }));
    }
    let c = Catalog::load(r.path()).unwrap();
    assert_eq!(
        c.occupied["topic"].canonicalize().unwrap(),
        path.canonicalize().unwrap()
    );
    assert_eq!(c.locals.len(), 2);
}

#[test]
fn rebase_and_bisect_reserve_a_branch_even_when_head_is_detached() {
    for state in [
        "rebase-merge/head-name",
        "rebase-apply/head-name",
        "BISECT_START",
    ] {
        let mut r = TestRepo::new();
        let hash = r.commit("base");
        r.git(&["branch", "topic"]);
        let wt = tempfile::tempdir().unwrap();
        let path = wt.path().join("linked");
        r.git(&["worktree", "add", "--detach", path.to_str().unwrap(), &hash]);
        let dir = std::process::Command::new("git")
            .args([
                "-C",
                path.to_str().unwrap(),
                "rev-parse",
                "--absolute-git-dir",
            ])
            .output()
            .unwrap();
        let file =
            std::path::PathBuf::from(String::from_utf8(dir.stdout).unwrap().trim()).join(state);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            if state == "BISECT_START" {
                "topic\n"
            } else {
                "refs/heads/topic\n"
            },
        )
        .unwrap();
        let c = Catalog::load(r.path()).unwrap();
        assert_eq!(
            c.occupied["topic"].canonicalize().unwrap(),
            path.canonicalize().unwrap()
        );
        assert!(matches!(
            execute(&r, deletion(&r, "topic")),
            Outcome::Failed { .. }
        ));
        assert!(matches!(
            execute(&r, Action::Switch("topic".into())),
            Outcome::Failed { .. }
        ));
    }
}

#[test]
fn merged_branch_deletion_is_one_click() {
    let mut r = TestRepo::new();
    r.commit("base");
    r.git(&["branch", "topic"]);
    done(execute(&r, deletion(&r, "topic")));
    assert!(
        !Catalog::load(r.path())
            .unwrap()
            .locals
            .iter()
            .any(|b| b.name == "topic")
    );
}

#[test]
fn unmerged_but_protected_by_branch_remote_or_commit_tags_deletes_without_warning() {
    for kind in ["branch", "remote", "tag", "annotated", "nested"] {
        let (r, action, tip) = unique_branch();
        match kind {
            "branch" => {
                r.git(&["branch", "keep", &tip.to_hex()]);
            }
            "remote" => {
                r.git(&["update-ref", "refs/remotes/origin/keep", &tip.to_hex()]);
            }
            "tag" => {
                r.git(&["tag", "keep", &tip.to_hex()]);
            }
            "annotated" => {
                r.git(&["tag", "-a", "keep", "-m", "keep", &tip.to_hex()]);
            }
            _ => {
                r.git(&["tag", "-a", "inner", "-m", "inner", &tip.to_hex()]);
                r.git(&["tag", "-a", "keep", "-m", "outer", "inner"]);
                r.git(&["tag", "-d", "inner"]);
            }
        }
        let out = execute(&r, action);
        done(out);
        assert!(
            !Catalog::load(r.path())
                .unwrap()
                .locals
                .iter()
                .any(|b| b.name == "topic")
        );
    }
}

#[test]
fn other_detached_worktrees_including_missing_locked_ones_protect_their_commits() {
    for missing in [false, true] {
        let (r, action, tip) = unique_branch();
        let wt = tempfile::tempdir().unwrap();
        let p = wt.path().join("linked");
        r.git(&[
            "worktree",
            "add",
            "--detach",
            p.to_str().unwrap(),
            &tip.to_hex(),
        ]);
        if missing {
            r.git(&["worktree", "lock", p.to_str().unwrap()]);
            std::fs::remove_dir_all(&p).unwrap();
        }
        done(execute(&r, action));
    }
}

#[test]
fn warning_contains_exact_endangered_ids_and_approval_deletes_only_that_branch() {
    let (r, action, tip) = unique_branch();
    let w = warning(execute(&r, action.clone()));
    assert_eq!(w.commits, [tip]);
    assert!(
        Catalog::load(r.path())
            .unwrap()
            .locals
            .iter()
            .any(|b| b.name == "topic")
    );
    done(Branches::new(r.path()).execute(action, Some(&w), &CancelTree::default()));
    assert_eq!(r.git(&["branch", "--show-current"]), "main");
    assert_eq!(Catalog::load(r.path()).unwrap().locals.len(), 1);
    // The warning's snapshot survives removal of the last ref for Show in log.
    assert!(w.repo.lookup(&tip).is_some());
}

#[test]
fn changed_deletion_tip_cannot_reuse_an_approval() {
    let (mut r, action, _) = unique_branch();
    let w = warning(execute(&r, action.clone()));
    r.checkout("topic");
    let new_tip = r.commit("new work");
    r.checkout("main");
    assert!(matches!(
        Branches::new(r.path()).execute(action, Some(&w), &CancelTree::default()),
        Outcome::Failed { .. }
    ));
    assert_eq!(r.git(&["rev-parse", "topic"]), new_tip);
}

#[test]
fn changing_the_lost_set_with_the_same_count_requires_another_warning() {
    let mut r = TestRepo::new();
    let base = r.commit("base");
    r.branch("a");
    let a = r.commit("a");
    r.checkout("main");
    r.branch("b");
    let b = r.commit("b");
    r.branch("topic");
    r.merge("a", "join");
    r.checkout("main");
    r.git(&["branch", "-D", "a", "b"]);
    r.git(&["tag", "protect", &a]);
    let action = deletion(&r, "topic");
    let first = warning(execute(&r, action.clone()));
    r.git(&["tag", "-f", "protect", &b]);
    let second =
        warning(Branches::new(r.path()).execute(action, Some(&first), &CancelTree::default()));
    assert_eq!(first.commits.len(), second.commits.len());
    assert_ne!(first.commits, second.commits);
    assert_eq!(r.git(&["rev-parse", "main"]), base);
}

#[test]
fn departing_detached_head_warns_before_switch_and_create_and_switch() {
    for create in [false, true] {
        let mut r = TestRepo::new();
        let base = r.commit("base");
        r.checkout("--detach");
        let tip = oid(&r.commit("detached work"));
        let action = if create {
            Action::Create(Create {
                start: oid(&base),
                name: "new".into(),
                track: None,
                switch: true,
            })
        } else {
            Action::Switch("main".into())
        };
        let w = warning(execute(&r, action.clone()));
        assert_eq!(w.commits, [tip]);
        assert_eq!(r.git(&["rev-parse", "HEAD"]), tip.to_hex());
        done(Branches::new(r.path()).execute(action, Some(&w), &CancelTree::default()));
    }
}

#[test]
fn creating_a_branch_at_detached_head_preserves_it_without_loss_warning() {
    let mut r = TestRepo::new();
    r.commit("base");
    r.checkout("--detach");
    let start = oid(&r.commit("detached work"));
    done(execute(
        &r,
        Action::Create(Create {
            start,
            name: "saved".into(),
            track: None,
            switch: true,
        }),
    ));
    assert_eq!(r.git(&["branch", "--show-current"]), "saved");
}

#[test]
fn git_refusal_preserves_staged_unstaged_and_untracked_files() {
    let mut r = TestRepo::new();
    r.write("file", b"base");
    r.commit_all("base");
    r.git(&["branch", "old"]);
    r.write("file", b"main");
    r.commit_all("main");
    r.write("file", b"staged");
    r.git(&["add", "file"]);
    r.write("file", b"unstaged");
    r.write("loose", b"untracked");
    let before = r.git(&["status", "--porcelain"]);
    assert!(matches!(
        execute(&r, Action::Switch("old".into())),
        Outcome::Failed { .. }
    ));
    assert_eq!(r.git(&["status", "--porcelain"]), before);
    assert_eq!(std::fs::read(r.path().join("file")).unwrap(), b"unstaged");
    assert_eq!(std::fs::read(r.path().join("loose")).unwrap(), b"untracked");
}

#[test]
fn a_missing_upstream_uses_git_head_fallback_and_a_ref_lock_never_forces_deletion() {
    let (r, action, _) = unique_branch();
    r.git(&["remote", "add", "origin", "https://example.invalid/r"]);
    r.git(&["config", "branch.topic.remote", "origin"]);
    r.git(&["config", "branch.topic.merge", "refs/heads/gone"]);
    assert!(matches!(execute(&r, action), Outcome::Warning(_)));
    let mut merged = TestRepo::new();
    merged.commit("base");
    merged.git(&["branch", "topic"]);
    std::fs::write(merged.path().join(".git/refs/heads/topic.lock"), "locked").unwrap();
    match execute(&merged, deletion(&merged, "topic")) {
        Outcome::Failed { report, .. } => assert_eq!(report.steps.len(), 1),
        out => panic!("{out:?}"),
    }
    assert_eq!(Catalog::load(merged.path()).unwrap().locals.len(), 2);
}

#[test]
fn pre_cancelled_operation_has_no_side_effects() {
    let mut r = TestRepo::new();
    let start = oid(&r.commit("base"));
    let cancel = CancelTree::default();
    cancel.cancel();
    let out = Branches::new(r.path()).execute(
        Action::Create(Create {
            start,
            name: "new".into(),
            track: None,
            switch: true,
        }),
        None,
        &cancel,
    );
    assert!(matches!(out, Outcome::Failed { .. }));
    assert_eq!(Catalog::load(r.path()).unwrap().locals.len(), 1);
}

#[test]
fn auto_setup_rebase_is_honored_for_existing_and_future_upstreams() {
    let mut r = TestRepo::new();
    let start = oid(&r.commit("base"));
    r.git(&["remote", "add", "origin", "https://example.invalid/r"]);
    r.git(&["update-ref", "refs/remotes/origin/topic", &start.to_hex()]);
    r.git(&["config", "branch.autoSetupRebase", "remote"]);
    for (name, upstream) in [("existing", "origin/topic"), ("future", "origin/future")] {
        done(execute(
            &r,
            Action::Create(Create {
                start,
                name: name.into(),
                track: Some(upstream.into()),
                switch: false,
            }),
        ));
        assert_eq!(r.git(&["config", &format!("branch.{name}.rebase")]), "true");
    }
}

#[test]
fn incomplete_worktree_administration_fails_closed() {
    let mut r = TestRepo::new();
    r.commit("base");
    r.git(&["branch", "topic"]);
    std::fs::write(r.path().join(".git/worktrees"), "not a directory").unwrap();
    assert!(Catalog::load(r.path()).is_err());
    assert!(matches!(
        execute(&r, deletion(&r, "topic")),
        Outcome::Failed { .. }
    ));
    assert_eq!(
        r.git(&["rev-parse", "topic"]),
        r.git(&["rev-parse", "main"])
    );
}

#[cfg(unix)]
#[test]
fn cancelling_checkout_stops_its_hook_and_child_process() {
    check_checkout_cancellation(false);
}

#[cfg(unix)]
#[test]
fn cancelling_checkout_stops_a_hook_that_ignores_term() {
    check_checkout_cancellation(true);
}

#[cfg(unix)]
fn check_checkout_cancellation(ignore_term: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mut r = TestRepo::new();
    r.commit("base");
    r.git(&["branch", "topic"]);
    let hooks = r.path().join("hooks");
    std::fs::create_dir(&hooks).unwrap();
    let hook = hooks.join("post-checkout");
    let marker = r.path().join("hook-started");
    let completed = r.path().join("hook-completed");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\n{}\nprintf started > '{}'\nsleep 30\nprintf completed > '{}'\n",
            if ignore_term { "trap '' TERM" } else { "" },
            marker.display(),
            completed.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    r.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    let cancel = CancelTree::default();
    let worker_cancel = cancel.clone();
    let branch_tool = Branches::new(r.path());
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let outcome = branch_tool.execute(Action::Switch("topic".into()), None, &worker_cancel);
        tx.send(outcome).unwrap();
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !marker.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    cancel.cancel();
    assert!(marker.exists(), "checkout hook did not start");
    let out = rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .expect("cancellation left a hook running");
    worker.join().unwrap();
    assert!(matches!(out, Outcome::Failed { .. }), "{out:?}");
    assert!(!completed.exists());
}

#[test]
fn bare_current_branch_cannot_be_deleted_even_when_git_would_allow_it() {
    let mut r = TestRepo::new();
    let tip = oid(&r.commit("only commit"));
    let bare = tempfile::tempdir().unwrap();
    r.git(&[
        "clone",
        "--bare",
        r.path().to_str().unwrap(),
        bare.path().to_str().unwrap(),
    ]);
    let catalog = Catalog::load(bare.path()).unwrap();
    assert!(!catalog.has_working_tree);
    assert_eq!(catalog.current.as_deref(), Some("main"));
    let outcome = Branches::new(bare.path()).execute(
        Action::Delete {
            name: "main".into(),
            tip,
        },
        None,
        &CancelTree::default(),
    );
    assert!(matches!(outcome, Outcome::Failed { .. }), "{outcome:?}");
    assert_eq!(Catalog::load(bare.path()).unwrap().locals.len(), 1);
}

#[test]
fn upstream_configuration_failure_reports_the_branch_that_was_created() {
    let mut r = TestRepo::new();
    let start = oid(&r.commit("base"));
    r.git(&["remote", "add", "origin", "https://example.invalid/r"]);
    std::fs::write(r.path().join(".git/config.lock"), "locked").unwrap();
    let out = execute(
        &r,
        Action::Create(Create {
            start,
            name: "created".into(),
            track: Some("origin/future".into()),
            switch: false,
        }),
    );
    let Outcome::Failed { error, .. } = out else {
        panic!("{out:?}");
    };
    assert!(
        error.to_string().contains("Branch created now exists"),
        "{error}"
    );
    assert_eq!(r.git(&["rev-parse", "created"]), start.to_hex());
    assert_eq!(r.git(&["branch", "--show-current"]), "main");
}

#[cfg(unix)]
#[test]
fn failing_checkout_hook_reports_that_the_new_branch_is_checked_out() {
    use std::os::unix::fs::PermissionsExt;
    let mut r = TestRepo::new();
    let start = oid(&r.commit("base"));
    let hook = r.path().join(".git/hooks/post-checkout");
    std::fs::write(&hook, "#!/bin/sh\nprintf 'hook refused' >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = execute(
        &r,
        Action::Create(Create {
            start,
            name: "created".into(),
            track: None,
            switch: true,
        }),
    );
    let Outcome::Failed { error, .. } = out else {
        panic!("{out:?}");
    };
    assert!(
        error
            .to_string()
            .contains("Branch created now exists and is checked out"),
        "{error}"
    );
    assert_eq!(r.git(&["branch", "--show-current"]), "created");
}
