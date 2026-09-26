//! Open pull requests of the forge repository a repository's `origin` points at: GitHub, for
//! now. Not in TortoiseGit. Research and decisions: `docs/research/github-forks-and-pull-requests.md`.
//!
//! Nothing here fetches commits. A pull request is shown only where its head is a commit the
//! repository has already, as a label on that commit ([`PullRequests::heads`]).

use std::collections::HashMap;
use std::time::Duration;

use crate::git::{Git, GitError};
use crate::oid::Oid;
use crate::repo::{RefKind, Repo};
use crate::revgraph::PullRequestHead;

pub mod github;

/// An open pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    /// The login of whoever opened it.
    pub author: String,
    pub draft: bool,
    /// The commit its head branch is at.
    pub head: Oid,
    /// The branch it proposes, and `owner/name` of the repository that branch is in (`None` if
    /// that repository was deleted).
    pub head_branch: String,
    pub head_repo: Option<String>,
    /// The branch it would be merged into, and `owner/name` of the repository it is in: the
    /// repository the pull request belongs to.
    pub base_branch: String,
    pub base_repo: String,
    /// Its page on the forge, e.g. `https://github.com/owner/name/pull/12`.
    pub url: String,
}

impl PullRequest {
    /// The branch it proposes as GitHub names it: `topic` in the same repository,
    /// `owner:topic` in another (a fork), and `topic (deleted fork)` if that is gone.
    pub fn head_label(&self) -> String {
        match &self.head_repo {
            Some(repo) if !repo.eq_ignore_ascii_case(&self.base_repo) => {
                let owner = repo.split('/').next().unwrap_or(repo);
                format!("{owner}:{}", self.head_branch)
            }
            Some(_) => self.head_branch.clone(),
            None => format!("{} (deleted fork)", self.head_branch),
        }
    }
}

/// A remote of the local repository, and the forge repository (`owner/name`) it points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub repo: String,
}

/// The open pull requests of a repository, with what is needed to find their base branches
/// among its refs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PullRequests {
    /// Newest first.
    pub list: Vec<PullRequest>,
    /// The remotes that point at repositories on the forge.
    pub remotes: Vec<Remote>,
    /// The upstream of each local branch that has one, by full ref name:
    /// `refs/heads/main` → `refs/remotes/origin/main`.
    pub upstreams: HashMap<String, String>,
}

impl PullRequests {
    /// The pull requests whose heads are commits of `repo`, each with where it is shown: its
    /// head commit, and the refs of its base branch. Those are the remote-tracking branches of
    /// the base branch, in every remote that points at the pull request's repository, and the
    /// local branches that have one of them as upstream.
    pub fn heads(&self, repo: &Repo) -> Vec<(PullRequestHead, &PullRequest)> {
        let mut bases: HashMap<(String, &str), Vec<usize>> = HashMap::new();
        for (i, r) in repo.refs.iter().enumerate() {
            let tracked = match r.kind {
                RefKind::RemoteBranch => Some(r.full_name.as_str()),
                RefKind::LocalBranch => self.upstreams.get(&r.full_name).map(String::as_str),
                _ => None,
            };
            if let Some(key) = tracked.and_then(|t| remote_branch(&self.remotes, t)) {
                bases.entry(key).or_default().push(i);
            }
        }
        self.list
            .iter()
            .filter_map(|pr| {
                let commit = repo.lookup(&pr.head)?;
                let key = (pr.base_repo.to_ascii_lowercase(), pr.base_branch.as_str());
                let bases = bases.get(&key).cloned().unwrap_or_default();
                Some((PullRequestHead { commit, bases }, pr))
            })
            .collect()
    }
}

/// The forge repository (`owner/name`, lowercase) and branch of a remote-tracking branch such as
/// `refs/remotes/origin/main`. Remote names may contain slashes: the longest that fits is the
/// remote.
fn remote_branch<'a>(remotes: &[Remote], full_name: &'a str) -> Option<(String, &'a str)> {
    let rest = full_name.strip_prefix("refs/remotes/")?;
    remotes
        .iter()
        .filter_map(|remote| {
            let branch = rest.strip_prefix(&remote.name)?.strip_prefix('/')?;
            Some((remote, branch))
        })
        .max_by_key(|(remote, _)| remote.name.len())
        .map(|(remote, branch)| (remote.repo.to_ascii_lowercase(), branch))
}

#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    #[error("origin is not a GitHub repository")]
    NoForge,
    #[error("this build of parterre has no GitHub support")]
    Unsupported,
    /// GitHub is only asked signed in, as the GitHub CLI is.
    #[error("the GitHub CLI (gh) isn't installed")]
    NoGh,
    #[error("the GitHub CLI (gh) isn't signed in to github.com")]
    NotSignedIn,
    #[error("GitHub turned down the GitHub CLI's (gh's) sign-in")]
    TokenRejected,
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("could not reach GitHub: {0}")]
    Network(String),
    #[error("GitHub's hourly limit is nearly used up; asking again in {minutes} min")]
    RateLimited { minutes: u64 },
    #[error("GitHub has no repository {repo}, or gh's account can't see it")]
    NotFound { repo: String },
    #[error("GitHub answered {status}: {message}")]
    Status { status: u16, message: String },
    #[error("unexpected answer from GitHub: {0}")]
    Parse(String),
}

impl ForgeError {
    /// True if pull requests can't be asked for until the user signs in with `gh`.
    pub fn needs_sign_in(&self) -> bool {
        matches!(
            self,
            ForgeError::NoGh | ForgeError::NotSignedIn | ForgeError::TokenRejected
        )
    }

    /// How long GitHub asked to be left alone, if it did.
    pub fn wait(&self) -> Option<Duration> {
        match self {
            ForgeError::RateLimited { minutes } => Some(Duration::from_secs(minutes * 60)),
            _ => None,
        }
    }

    /// How to tell the user, in a dialog.
    pub fn explain(&self) -> Explanation {
        const SIGN_IN: &str = "gh auth login";
        let (title, body, command) = match self {
            ForgeError::NoGh => (
                "GitHub CLI not found",
                "parterre gets pull requests from GitHub through the GitHub CLI (gh), which \
                 isn't installed. Install it, sign in with this command in a terminal, and \
                 turn pull requests on again."
                    .to_owned(),
                Some(SIGN_IN),
            ),
            ForgeError::NotSignedIn => (
                "Not signed in to GitHub",
                "parterre gets pull requests from GitHub through the GitHub CLI (gh), which \
                 isn't signed in. Sign in with this command in a terminal, and turn pull \
                 requests on again."
                    .to_owned(),
                Some(SIGN_IN),
            ),
            ForgeError::TokenRejected => (
                "GitHub didn't accept the sign-in",
                "The GitHub CLI's sign-in may have expired. Sign in again with this command in \
                 a terminal, and turn pull requests on again."
                    .to_owned(),
                Some(SIGN_IN),
            ),
            ForgeError::RateLimited { minutes } => (
                "GitHub's hourly limit is nearly used up",
                format!(
                    "parterre stops asking GitHub while less than a tenth of your hourly \
                     allowance is left, so that your other tools keep working. Pull requests \
                     appear by themselves in about {minutes} min."
                ),
                None,
            ),
            ForgeError::NotFound { repo } => (
                "Repository not found on GitHub",
                format!(
                    "GitHub has no repository {repo}, or your account can't see it. For a \
                     private repository, check which account the GitHub CLI is signed in as:"
                ),
                Some("gh auth status"),
            ),
            ForgeError::Network(why) => (
                "Couldn't reach GitHub",
                format!(
                    "{}. Check the network connection, and turn pull requests on again.",
                    capitalised(why)
                ),
                None,
            ),
            other => (
                "Couldn't show pull requests",
                format!("{}.", capitalised(&other.to_string())),
                None,
            ),
        };
        Explanation {
            title,
            body,
            command,
        }
    }
}

/// What a dialog says about a [`ForgeError`]: a short title, a sentence or two, and a command
/// to run in a terminal, if one helps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Explanation {
    pub title: &'static str,
    pub body: String,
    pub command: Option<&'static str>,
}

/// `text` with its first letter in upper case.
fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// How long a loaded list is used before GitHub is asked again, as in t3code: a minute if it
/// has pull requests (they change), five if not.
pub fn fresh_for(found_any: bool) -> Duration {
    Duration::from_secs(if found_any { 60 } else { 5 * 60 })
}

/// How long to wait after `failures` failed loads in a row, as in t3code: 20 s, doubling, at
/// most 15 min.
pub fn retry_after(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(6);
    Duration::from_secs((20u64 << doublings).min(15 * 60))
}

/// The remotes of the repository `git` works on, with their URLs (`insteadOf` rewrites
/// applied, as git would fetch from them).
pub fn remote_urls(git: &Git) -> Result<Vec<(String, String)>, GitError> {
    Ok(parse_remote_urls(&git.run(&["remote", "-v"])?))
}

/// Parses `git remote -v`: `name<TAB>url (fetch)` lines, and the same with `(push)`. In a
/// partial clone the fetch line ends in the filter, e.g. `(fetch) [tree:0]`.
fn parse_remote_urls(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once('\t')?;
            let (url, _) = rest.split_once(" (fetch)")?;
            Some((name.to_owned(), url.to_owned()))
        })
        .collect()
}

/// The upstream of every local branch that has one (see [`PullRequests::upstreams`]).
pub fn upstreams(git: &Git) -> Result<HashMap<String, String>, GitError> {
    let out = git.run(&[
        "for-each-ref",
        "--format=%(refname)%00%(upstream)",
        "refs/heads",
    ])?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let (branch, upstream) = line.split_once('\0')?;
            (!upstream.is_empty()).then(|| (branch.to_owned(), upstream.to_owned()))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{Commit, CommitIx, GitRef, Head};

    fn oid(n: u8) -> Oid {
        Oid::from_hex(&format!("{n:02x}").repeat(20)).unwrap()
    }

    fn commit(n: u8) -> Commit {
        Commit {
            oid: oid(n),
            parents: Vec::new(),
            truncated: false,
            empty_tree: false,
            author_name: String::new(),
            author_email: String::new(),
            author_time: 0,
            author_date: String::new(),
            commit_time: 0,
            subject: String::new(),
        }
    }

    fn git_ref(full_name: &str, target: u32) -> GitRef {
        let (kind, name) = crate::git::classify_ref(full_name);
        GitRef {
            full_name: full_name.to_owned(),
            name,
            kind,
            target: CommitIx(target),
            annotated: false,
            is_head: false,
        }
    }

    fn pull_request(number: u64, head: u8, base_repo: &str, base_branch: &str) -> PullRequest {
        PullRequest {
            number,
            title: format!("PR {number}"),
            author: "someone".into(),
            draft: false,
            head: oid(head),
            head_branch: "feature".into(),
            head_repo: Some(base_repo.into()),
            base_branch: base_branch.into(),
            base_repo: base_repo.into(),
            url: format!("https://github.com/{base_repo}/pull/{number}"),
        }
    }

    #[test]
    fn heads_are_placed_on_local_commits_with_their_base_refs() {
        let repo = Repo::new(
            "/x".into(),
            vec![commit(1), commit(2)],
            vec![
                git_ref("refs/heads/main", 0),
                git_ref("refs/heads/topic", 0),
                git_ref("refs/remotes/origin/main", 0),
                git_ref("refs/remotes/upstream/main", 0),
                git_ref("refs/remotes/origin/feature", 1),
                git_ref("refs/remotes/origin/sub/main", 0),
                git_ref("refs/tags/main", 0),
            ],
            Head::Detached(CommitIx(0)),
        );
        let prs = PullRequests {
            list: vec![
                pull_request(12, 2, "Me/Fork", "main"),
                // Its head isn't in the repository: left out.
                pull_request(11, 9, "Me/Fork", "main"),
                pull_request(10, 2, "them/parent", "main"),
                pull_request(9, 1, "Me/Fork", "gone"),
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
            upstreams: HashMap::from([
                ("refs/heads/main".into(), "refs/remotes/origin/main".into()),
                (
                    "refs/heads/topic".into(),
                    "refs/remotes/upstream/main".into(),
                ),
            ]),
        };
        let heads: Vec<(u64, u32, Vec<usize>)> = prs
            .heads(&repo)
            .into_iter()
            .map(|(h, pr)| (pr.number, h.commit.0, h.bases))
            .collect();
        assert_eq!(
            heads,
            [
                // Owner and name compare without case, as on GitHub. origin/sub/main is the
                // branch sub/main, and the tag is no branch.
                (12, 1, vec![0, 2]),
                (10, 1, vec![1, 3]),
                (9, 0, vec![]),
            ]
        );
    }

    #[test]
    fn head_labels_name_the_fork() {
        let mut pr = pull_request(1, 1, "o/r", "main");
        assert_eq!(pr.head_label(), "feature");
        pr.head_repo = Some("O/R".into());
        assert_eq!(pr.head_label(), "feature");
        pr.head_repo = Some("them/fork".into());
        assert_eq!(pr.head_label(), "them:feature");
        pr.head_repo = None;
        assert_eq!(pr.head_label(), "feature (deleted fork)");
    }

    #[test]
    fn errors_are_explained_plainly() {
        let e = ForgeError::NotSignedIn.explain();
        assert_eq!(e.title, "Not signed in to GitHub");
        assert_eq!(e.command, Some("gh auth login"));
        assert!(e.body.ends_with("turn pull requests on again."));
        assert_eq!(ForgeError::NoGh.explain().command, Some("gh auth login"));
        let e = ForgeError::RateLimited { minutes: 12 }.explain();
        assert!(e.body.contains("in about 12 min"), "{}", e.body);
        assert_eq!(e.command, None);
        let e = ForgeError::Network("connection refused".into()).explain();
        assert!(e.body.starts_with("Connection refused. "), "{}", e.body);
        let e = ForgeError::Parse("bad JSON".into()).explain();
        assert_eq!(e.body, "Unexpected answer from GitHub: bad JSON.");
    }

    #[test]
    fn asks_again_like_t3code() {
        assert_eq!(fresh_for(true), Duration::from_secs(60));
        assert_eq!(fresh_for(false), Duration::from_secs(300));
        let waits: Vec<u64> = (1..=8).map(|n| retry_after(n).as_secs()).collect();
        assert_eq!(waits, [20, 40, 80, 160, 320, 640, 900, 900]);
        assert_eq!(retry_after(0).as_secs(), 20);
    }

    #[test]
    fn parses_remote_urls() {
        let out = "origin\thttps://github.com/o/r.git (fetch)\n\
                   origin\thttps://github.com/o/r.git (push)\n\
                   up\tgit@github.com:p/r.git (fetch)\n\
                   up\tno_push (push)\n\
                   partial\thttps://github.com/cli/cli.git (fetch) [tree:0]\n";
        assert_eq!(
            parse_remote_urls(out),
            [
                ("origin".to_owned(), "https://github.com/o/r.git".to_owned()),
                ("up".to_owned(), "git@github.com:p/r.git".to_owned()),
                (
                    "partial".to_owned(),
                    "https://github.com/cli/cli.git".to_owned()
                ),
            ]
        );
    }
}
