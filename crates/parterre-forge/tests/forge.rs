// The throwaway-repository helpers of parterre-core's tests, shared rather than copied.
#[path = "../../parterre-core/tests/common/mod.rs"]
mod common;

use common::TestRepo;
use parterre_core::git::Git;
use parterre_core::revgraph::{self, GraphOptions};
use parterre_forge as forge;
use parterre_forge::github::{self, GithubRepo};
use parterre_forge::{PullRequest, PullRequests, Remote};

#[test]
fn finds_github_origins_through_insteadof() {
    let mut r = TestRepo::new();
    r.commit("A");
    let git = Git::new(r.path());
    assert_eq!(github::origin(&git), None, "no origin");
    r.git(&["remote", "add", "origin", "/somewhere/else"]);
    assert_eq!(github::origin(&git), None, "not on GitHub");
    r.git(&["remote", "set-url", "origin", "gh:aquamoth/parterre"]);
    r.git(&["config", "url.git@github.com:.insteadOf", "gh:"]);
    let expected = GithubRepo {
        owner: "aquamoth".into(),
        name: "parterre".into(),
    };
    assert_eq!(github::origin(&git), Some(expected));
    r.git(&[
        "remote",
        "add",
        "up/stream",
        "https://github.com/up/stream.git",
    ]);
    assert_eq!(
        forge::remote_urls(&git).unwrap(),
        [
            (
                "origin".to_owned(),
                "git@github.com:aquamoth/parterre".to_owned()
            ),
            (
                "up/stream".to_owned(),
                "https://github.com/up/stream.git".to_owned()
            ),
        ]
    );
}

#[test]
fn pull_requests_label_their_heads_in_the_graph() {
    let mut r = TestRepo::new();
    r.commit("A");
    let pushed = r.commit("B");
    r.commit("C");
    r.git(&["remote", "add", "origin", "https://github.com/o/r"]);
    r.git(&["update-ref", "refs/remotes/origin/main", &pushed]);
    r.git(&["branch", "--set-upstream-to=origin/main", "main"]);
    let git = Git::new(r.path());
    let upstreams = forge::upstreams(&git).unwrap();
    assert_eq!(
        upstreams.get("refs/heads/main").map(String::as_str),
        Some("refs/remotes/origin/main")
    );

    let pr = PullRequest {
        number: 7,
        title: "Seven".into(),
        author: "someone".into(),
        draft: false,
        head: parterre_core::Oid::from_hex(&pushed).unwrap(),
        head_branch: "topic".into(),
        head_repo: Some("o/r".into()),
        base_branch: "main".into(),
        base_repo: "o/r".into(),
        url: "https://github.com/o/r/pull/7".into(),
    };
    let prs = PullRequests {
        list: vec![pr],
        remotes: vec![Remote {
            name: "origin".into(),
            repo: "o/r".into(),
        }],
        upstreams,
    };
    let options = GraphOptions {
        show_pull_requests: true,
        show_remote_branches: false,
        ..GraphOptions::default()
    };
    // With remote branches hidden, main stands for its upstream as the base.
    let repo = r.load();
    let heads: Vec<_> = prs.heads(&repo).into_iter().map(|(h, _)| h).collect();
    let g = revgraph::build_with_pull_requests(&repo, &options, &heads);
    let labelled: Vec<(String, Vec<usize>)> = g
        .nodes
        .iter()
        .map(|n| {
            (
                repo.commit(n.commit).subject.clone(),
                n.pull_requests.clone(),
            )
        })
        .collect();
    assert!(
        labelled.contains(&("B".to_owned(), vec![0])),
        "{labelled:?}"
    );
    assert!(labelled.contains(&("C".to_owned(), vec![])));
}

/// My fork, cloned with the parent as `upstream`, with remote branches hidden: my pull request
/// into the parent shows on my branch's commit, which only `origin/topic` has. Another fork's
/// pull request, were GitHub to list one, brings none of its commits in.
#[test]
fn a_forks_own_pull_request_shows_with_remote_branches_hidden() {
    let mut r = TestRepo::new();
    r.commit("A");
    let base = r.commit("B");
    let parent = r.commit("C");
    r.git(&["reset", "-q", "--hard", &base]);
    let mine = r.commit("D");
    r.git(&["reset", "-q", "--hard", &base]);
    let theirs = r.commit("E");
    r.git(&["reset", "-q", "--hard", &base]);
    r.git(&["remote", "add", "origin", "https://github.com/me/fork"]);
    r.git(&[
        "remote",
        "add",
        "upstream",
        "https://github.com/them/parent",
    ]);
    r.git(&["update-ref", "refs/remotes/origin/main", &base]);
    r.git(&["update-ref", "refs/remotes/origin/topic", &mine]);
    r.git(&["update-ref", "refs/remotes/upstream/main", &parent]);
    r.git(&["update-ref", "refs/remotes/other/topic", &theirs]);
    r.git(&["branch", "--set-upstream-to=origin/main", "main"]);
    let git = Git::new(r.path());
    let into_parent = |number, head: &str, head_repo: &str| PullRequest {
        number,
        title: format!("PR {number}"),
        author: "someone".into(),
        draft: false,
        head: parterre_core::Oid::from_hex(head).unwrap(),
        head_branch: "topic".into(),
        head_repo: Some(head_repo.into()),
        base_branch: "main".into(),
        base_repo: "them/parent".into(),
        url: format!("https://github.com/them/parent/pull/{number}"),
    };
    let prs = PullRequests {
        list: vec![
            into_parent(5, &mine, "me/fork"),
            into_parent(6, &theirs, "other/fork"),
        ],
        remotes: vec![
            Remote {
                name: "origin".into(),
                repo: "me/fork".into(),
            },
            Remote {
                name: "upstream".into(),
                repo: "them/parent".into(),
            },
        ],
        upstreams: forge::upstreams(&git).unwrap(),
    };
    let options = GraphOptions {
        show_pull_requests: true,
        show_remote_branches: false,
        ..GraphOptions::default()
    };
    let repo = r.load();
    let heads: Vec<_> = prs.heads(&repo).into_iter().map(|(h, _)| h).collect();
    let g = revgraph::build_with_pull_requests(&repo, &options, &heads);
    let labelled: Vec<(String, Vec<usize>)> = g
        .nodes
        .iter()
        .map(|n| {
            (
                repo.commit(n.commit).subject.clone(),
                n.pull_requests.clone(),
            )
        })
        .collect();
    assert!(
        labelled.contains(&("D".to_owned(), vec![0])),
        "{labelled:?}"
    );
    for absent in ["C", "E"] {
        assert!(
            labelled.iter().all(|(s, _)| s != absent),
            "{absent}: {labelled:?}"
        );
    }
}

#[test]
fn canned_pull_requests_are_on_their_heads_on_origin() {
    let mut r = TestRepo::new();
    r.commit("A");
    let pushed = r.commit("B");
    r.git(&["remote", "add", "origin", "git@github.com:o/r.git"]);
    r.git(&["update-ref", "refs/remotes/origin/main", &pushed]);
    r.git(&["update-ref", "refs/remotes/origin/topic", &pushed]);
    let git = Git::new(r.path());

    let json = r#"[{"number": 7, "title": "Seven", "author": "someone", "draft": true,
                    "head": "topic", "base": "main"}]"#;
    let prs = github::load_canned(&git, json).unwrap();
    assert_eq!(
        prs.list,
        [PullRequest {
            number: 7,
            title: "Seven".into(),
            author: "someone".into(),
            draft: true,
            head: parterre_core::Oid::from_hex(&pushed).unwrap(),
            head_branch: "topic".into(),
            head_repo: Some("o/r".into()),
            base_branch: "main".into(),
            base_repo: "o/r".into(),
            url: "https://github.com/o/r/pull/7".into(),
        }]
    );
    assert_eq!(
        prs.remotes,
        [Remote {
            name: "origin".into(),
            repo: "o/r".into(),
        }]
    );

    let missing = r#"[{"number": 8, "title": "Eight", "head": "gone", "base": "main"}]"#;
    let error = github::load_canned(&git, missing).unwrap_err();
    assert_eq!(
        error.to_string(),
        "pull requests file: origin has no branch gone"
    );
    let error = github::load_canned(&git, r#"[{"number": 9}]"#).unwrap_err();
    assert!(
        error.to_string().starts_with("pull requests file: "),
        "{error}"
    );
}

/// Asks GitHub for parterre's open pull requests from its `main`, signed in as `gh` is.
/// Needs the network and a signed-in `gh`, so only on request:
/// `cargo test -p parterre-core --features github --test forge -- --ignored`.
#[cfg(feature = "github")]
#[test]
#[ignore = "needs the network and a signed-in gh"]
fn loads_parterres_pull_requests_from_github() {
    let mut r = TestRepo::new();
    let a = r.commit("A");
    r.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/aquamoth/parterre.git",
    ]);
    r.git(&["update-ref", "refs/remotes/origin/main", &a]);
    let prs = github::load(&Git::new(r.path())).expect("GitHub answers");
    assert_eq!(
        prs.remotes,
        [Remote {
            name: "origin".into(),
            repo: "aquamoth/parterre".into()
        }]
    );
    for pr in &prs.list {
        assert!(
            pr.url
                .starts_with("https://github.com/aquamoth/parterre/pull/")
        );
        assert_eq!(pr.head_branch, "main");
    }
    eprintln!("{} open pull requests from main", prs.list.len());
}

#[test]
fn github_is_not_asked_without_fetched_branches() {
    let mut r = TestRepo::new();
    r.commit("A");
    r.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/aquamoth/parterre.git",
    ]);
    // No branch of origin fetched: nothing could be shown, so nothing is asked (not even for
    // a token), and there is no error.
    let prs = github::load(&Git::new(r.path())).expect("nothing to ask");
    assert!(prs.list.is_empty());
}

#[test]
fn a_remote_branch_proposed_by_an_open_pull_request_is_found_before_deleting_it() {
    let mut r = TestRepo::new();
    let tip = r.commit("A");
    r.git(&["remote", "add", "origin", "https://github.com/o/r"]);
    r.git(&["remote", "add", "fork", "https://github.com/someone/r"]);
    r.git(&["remote", "add", "elsewhere", "/srv/git/r"]);
    r.git(&["update-ref", "refs/remotes/origin/feature", &tip]);
    let canned = r#"[{"number": 12, "title": "Add feature", "head": "feature", "base": "main"}]"#;
    let branch = |remote: &str, branch: &str| (remote.to_owned(), branch.to_owned());
    let found = github::proposals(
        &Git::new(r.path()),
        &[
            branch("origin", "feature"),
            branch("origin", "main"),
            branch("fork", "feature"),
            branch("elsewhere", "feature"),
        ],
        Some(canned),
    );
    let github::Proposed::Open(pr) = &found[0] else {
        panic!("open: {found:?}")
    };
    assert_eq!((pr.number, pr.title.as_str()), (12, "Add feature"));
    assert_eq!(found[1], github::Proposed::No);
    // The canned list's pull requests are all origin's own.
    assert_eq!(found[2], github::Proposed::No);
    assert_eq!(found[3], github::Proposed::NotOnGithub);
    // Nothing on GitHub: nothing asked.
    assert_eq!(
        github::proposals(&Git::new(r.path()), &[branch("elsewhere", "x")], None),
        [github::Proposed::NotOnGithub]
    );
}
