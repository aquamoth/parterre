mod common;

use common::TestRepo;
use parterre_core::forge::github::{self, GithubRepo};
use parterre_core::forge::{self, PullRequest, PullRequests, Remote};
use parterre_core::git::Git;
use parterre_core::revgraph::{self, GraphOptions};

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
