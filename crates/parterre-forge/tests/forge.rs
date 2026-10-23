// parterre-core's test helpers, shared rather than copied.
#[path = "../../parterre-core/tests/common/mod.rs"]
mod common;

use common::TestRepo;
use parterre_core::git::Git;
use parterre_core::revgraph::{self, GraphOptions};
use parterre_forge as forge;
use parterre_forge::github::{self, GithubRepo};
use parterre_forge::{PullRequest, PullRequests, Remote};

/// Each node's subject and pull requests, in a graph with remote branches hidden.
fn labelled_nodes(r: &TestRepo, prs: &PullRequests) -> Vec<(String, Vec<usize>)> {
    let options = GraphOptions {
        show_pull_requests: true,
        show_remote_branches: false,
        ..GraphOptions::default()
    };
    let repo = r.load();
    let heads: Vec<_> = prs.heads(&repo).into_iter().map(|(h, _)| h).collect();
    let g = revgraph::build_with_pull_requests(&repo, &options, &heads);
    g.nodes
        .iter()
        .map(|n| {
            (
                repo.commit(n.commit).subject.clone(),
                n.pull_requests.clone(),
            )
        })
        .collect()
}

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
    // With remote branches hidden, main stands for its upstream as the base.
    let labelled = labelled_nodes(&r, &prs);
    assert!(
        labelled.contains(&("B".to_owned(), vec![0])),
        "{labelled:?}"
    );
    assert!(labelled.contains(&("C".to_owned(), vec![])));
}

/// My fork, cloned with the parent as `upstream`: my pull request into the parent shows on my
/// branch's commit, which only `origin/topic` has. Another fork's pull request, were GitHub to
/// list one, brings none of its commits in.
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
    let labelled = labelled_nodes(&r, &prs);
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

/// `cargo test -p parterre-forge --features github --test forge -- --ignored`
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

/// #384: parterre's own #422, merged from `t3/prune-grep` at 79e5e5a.
/// `cargo test -p parterre-forge --features github --test forge -- --ignored merged`
#[cfg(feature = "github")]
#[test]
#[ignore = "needs the network and a signed-in gh"]
fn finds_a_merged_pull_request_of_parterre_on_github() {
    let oid = |hex: &str| parterre_core::Oid::from_hex(hex).unwrap();
    let origin = forge::Origin::Github(GithubRepo {
        owner: "aquamoth".into(),
        name: "parterre".into(),
    });
    let r = TestRepo::new();
    let branches = [
        forge::GoneBranch {
            name: "t3/prune-grep".into(),
            tip: oid("79e5e5af1f789743024e9bdbd0cc5466d153ff4b"),
        },
        // The same name at another commit is another branch.
        forge::GoneBranch {
            name: "t3/prune-grep".into(),
            tip: oid(&"0".repeat(40)),
        },
    ];
    let found = origin
        .merged(&Git::new(r.path()), &branches)
        .expect("GitHub answers");
    assert_eq!(
        found,
        [
            Some(parterre_core::merged::Merged {
                how: None,
                landed: Some(oid("ffbf0b479fd70868e677597460810010a985145e")),
                pull_request: Some(422),
            }),
            None,
        ]
    );
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
    // Not even for a token.
    let prs = github::load(&Git::new(r.path())).expect("nothing to ask");
    assert!(prs.list.is_empty());
}

fn remote_branch(remote: &str, branch: &str) -> (String, String) {
    (remote.to_owned(), branch.to_owned())
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
    let found = forge::proposals(
        &Git::new(r.path()),
        &[
            remote_branch("origin", "feature"),
            remote_branch("origin", "main"),
            remote_branch("fork", "feature"),
            remote_branch("elsewhere", "feature"),
        ],
        Some(canned),
    );
    let forge::Proposed::Open(pr) = &found[0] else {
        panic!("open: {found:?}")
    };
    assert_eq!((pr.number, pr.title.as_str()), (12, "Add feature"));
    assert_eq!(found[1], forge::Proposed::No);
    // The canned list's pull requests are all origin's own.
    assert_eq!(found[2], forge::Proposed::No);
    assert_eq!(found[3], forge::Proposed::NotOnForge);
    // Nothing on a forge: nothing asked.
    assert_eq!(
        forge::proposals(
            &Git::new(r.path()),
            &[remote_branch("elsewhere", "x")],
            None
        ),
        [forge::Proposed::NotOnForge]
    );
}

/// The same before deleting a branch of an Azure DevOps remote: its repository's active pull
/// requests are looked through, whatever form its remote's URL has.
#[test]
fn an_azure_devops_branch_proposed_by_an_active_pull_request_is_found_before_deleting_it() {
    let mut r = TestRepo::new();
    let tip = r.commit("A");
    r.git(&[
        "remote",
        "add",
        "origin",
        "https://o@dev.azure.com/o/My%20Project/_git/r",
    ]);
    r.git(&[
        "remote",
        "add",
        "ssh",
        "git@ssh.dev.azure.com:v3/o/My%20Project/r",
    ]);
    r.git(&["remote", "add", "elsewhere", "/srv/git/r"]);
    r.git(&["update-ref", "refs/remotes/origin/feature", &tip]);
    let canned = r#"[{"number": 7, "title": "Add feature", "head": "feature", "base": "main"}]"#;
    let found = forge::proposals(
        &Git::new(r.path()),
        &[
            remote_branch("origin", "feature"),
            remote_branch("origin", "main"),
            remote_branch("ssh", "feature"),
            remote_branch("elsewhere", "feature"),
        ],
        Some(canned),
    );
    let forge::Proposed::Open(pr) = &found[0] else {
        panic!("open: {found:?}")
    };
    assert_eq!(
        pr.url,
        "https://dev.azure.com/o/My%20Project/_git/r/pullrequest/7"
    );
    assert_eq!(found[1], forge::Proposed::No);
    assert_eq!(found[2], found[0], "the same repository over SSH");
    assert_eq!(found[3], forge::Proposed::NotOnForge);
}

/// An Azure DevOps origin, as its SSH remote names it, and another remote of the same
/// repository over HTTPS: both are the repository the pull requests belong to, so `upstream`'s
/// branches are bases too.
#[test]
fn azure_devops_origins_have_pull_requests_on_their_heads() {
    let mut r = TestRepo::new();
    r.commit("A");
    let pushed = r.commit("B");
    let git = Git::new(r.path());
    r.git(&[
        "remote",
        "add",
        "origin",
        "git@ssh.dev.azure.com:v3/o/My%20Project/r",
    ]);
    r.git(&[
        "remote",
        "add",
        "upstream",
        "https://o@dev.azure.com/o/My%20Project/_git/r",
    ]);
    r.git(&["update-ref", "refs/remotes/origin/main", &pushed]);
    r.git(&["update-ref", "refs/remotes/origin/topic", &pushed]);
    let origin = forge::origin(&git).expect("origin is on Azure DevOps");
    assert_eq!(origin.forge(), forge::Forge::AzureDevops);

    let json = r#"[{"number": 7, "title": "Seven", "head": "topic", "base": "main"}]"#;
    let prs = origin.load_canned(&git, json).unwrap();
    let repo_name = "o/My Project/r";
    assert_eq!(
        prs.list,
        [PullRequest {
            number: 7,
            title: "Seven".into(),
            author: String::new(),
            draft: false,
            head: parterre_core::Oid::from_hex(&pushed).unwrap(),
            head_branch: "topic".into(),
            head_repo: Some(repo_name.into()),
            base_branch: "main".into(),
            base_repo: repo_name.into(),
            url: "https://dev.azure.com/o/My%20Project/_git/r/pullrequest/7".into(),
        }]
    );
    assert_eq!(
        prs.remotes,
        [
            Remote {
                name: "origin".into(),
                repo: repo_name.into(),
            },
            Remote {
                name: "upstream".into(),
                repo: repo_name.into(),
            },
        ]
    );
    let repo = r.load();
    let heads = prs.heads(&repo);
    assert_eq!(heads.len(), 1);
    assert!(heads[0].0.head.is_some(), "origin/topic is its head branch");
}

/// #384: as [`loads_pull_requests_from_azure_devops`], with `PARTERRE_AZURE_MERGED="BRANCH
/// HEAD NUMBER MERGE_COMMIT"` naming a completed pull request of that repository, from BRANCH
/// at HEAD.
#[cfg(feature = "azure-devops")]
#[test]
#[ignore = "needs the network and a sign-in to Azure DevOps"]
fn finds_a_completed_pull_request_on_azure_devops() {
    let url = std::env::var("PARTERRE_AZURE_REPO").expect("PARTERRE_AZURE_REPO is set");
    let merged = std::env::var("PARTERRE_AZURE_MERGED").expect("PARTERRE_AZURE_MERGED is set");
    let [branch, head, number, landed] = merged.split(' ').collect::<Vec<_>>()[..] else {
        panic!("PARTERRE_AZURE_MERGED is BRANCH HEAD NUMBER MERGE_COMMIT");
    };
    let oid = |hex: &str| parterre_core::Oid::from_hex(hex).expect("a commit id");
    let mut r = TestRepo::new();
    r.commit("A");
    r.git(&["remote", "add", "origin", &url]);
    let git = Git::new(r.path());
    let origin = forge::origin(&git).expect("on Azure DevOps");
    let asked = [forge::GoneBranch {
        name: branch.into(),
        tip: oid(head),
    }];
    let found = origin.merged(&git, &asked).expect("Azure DevOps answers");
    let found = found[0].clone().expect("found merged");
    assert_eq!(found.pull_request, Some(number.parse().unwrap()));
    assert_eq!(found.landed, Some(oid(landed)));
    eprintln!("{found:?}");
}

/// `PARTERRE_AZURE_REPO=https://dev.azure.com/o/p/_git/r cargo test -p parterre-forge
/// --features azure-devops --test forge -- --ignored azure`
#[cfg(feature = "azure-devops")]
#[test]
#[ignore = "needs the network and a sign-in to Azure DevOps"]
fn loads_pull_requests_from_azure_devops() {
    let url = std::env::var("PARTERRE_AZURE_REPO").expect("PARTERRE_AZURE_REPO is set");
    let mut r = TestRepo::new();
    r.commit("A");
    r.git(&["remote", "add", "origin", &url]);
    let prs = forge::azure::load(&Git::new(r.path())).expect("Azure DevOps answers");
    for pr in &prs.list {
        assert!(pr.url.starts_with("https://dev.azure.com/"), "{}", pr.url);
        assert_eq!(pr.head.to_hex().len(), 40);
    }
    eprintln!("{} active pull requests", prs.list.len());
    // Before deleting its branch, an active pull request is found again.
    if let Some(pr) = prs.list.first() {
        let found = forge::proposals(
            &Git::new(r.path()),
            &[("origin".to_owned(), pr.head_branch.clone())],
            None,
        );
        assert_eq!(found, [forge::Proposed::Open(pr.clone())]);
    }
}
