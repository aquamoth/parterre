//! Design: `docs/research/github-forks-and-pull-requests.md`. Nothing here fetches commits: a
//! pull request is shown only on a head commit the repository already has.

use std::collections::HashMap;
use std::fmt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use parterre_core::git::{Git, GitError};
use parterre_core::oid::Oid;
use parterre_core::repo::{RefKind, Repo};
use parterre_core::revgraph::PullRequestHead;
use parterre_core::text::capitalized;

pub mod azure;
pub mod github;
mod repositories;

pub use repositories::{Listed, Repositories};

use azure::AzureRepo;
use github::GithubRepo;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub draft: bool,
    pub head: Oid,
    pub head_branch: String,
    /// `owner/name`; `None` once that repository is deleted.
    pub head_repo: Option<String>,
    pub base_branch: String,
    /// `owner/name`: the repository the pull request belongs to.
    pub base_repo: String,
    pub url: String,
}

impl PullRequest {
    /// The head branch as GitHub labels it.
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

/// The forge repository a repository's `origin` points at, which its pull requests are asked of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Github(GithubRepo),
    AzureDevops(AzureRepo),
}

impl Origin {
    pub fn from_url(url: &str) -> Option<Origin> {
        GithubRepo::from_url(url)
            .map(Origin::Github)
            .or_else(|| AzureRepo::from_url(url).map(Origin::AzureDevops))
    }

    pub fn forge(&self) -> Forge {
        match self {
            Origin::Github(_) => Forge::Github,
            Origin::AzureDevops(_) => Forge::AzureDevops,
        }
    }

    /// `owner/name` on GitHub, `org/project/name` on Azure DevOps.
    pub fn full_name(&self) -> String {
        match self {
            Origin::Github(repo) => repo.full_name(),
            Origin::AzureDevops(repo) => repo.full_name(),
        }
    }

    /// Blocks: run it on a worker thread.
    pub fn load(&self, git: &Git) -> Result<PullRequests, ForgeError> {
        match self {
            Origin::Github(_) => github::load(git),
            Origin::AzureDevops(_) => azure::load(git),
        }
    }

    /// From `json` instead of the forge (`--pull-requests-from`).
    pub fn load_canned(&self, git: &Git, json: &str) -> Result<PullRequests, ForgeError> {
        match self {
            Origin::Github(_) => github::load_canned(git, json),
            Origin::AzureDevops(_) => azure::load_canned(git, json),
        }
    }
}

/// Whether an open pull request proposes a branch about to be deleted on a remote (#318).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Proposed {
    NotOnForge,
    No,
    Open(PullRequest),
    /// The forge couldn't be asked: why.
    Unknown(String),
}

impl Proposed {
    /// Whether one of a forge's open pull requests proposes `branch` of `repo`.
    fn among(found: &Result<Vec<PullRequest>, ForgeError>, repo: &str, branch: &str) -> Proposed {
        let list = match found {
            Ok(list) => list,
            Err(e) => return Proposed::Unknown(e.to_string()),
        };
        let proposing = list.iter().find(|pr| {
            pr.head_branch == branch
                && pr
                    .head_repo
                    .as_deref()
                    .is_some_and(|r| r.eq_ignore_ascii_case(repo))
        });
        proposing.map_or(Proposed::No, |pr| Proposed::Open(pr.clone()))
    }
}

/// For each remote branch `(remote, branch)`: whether an open pull request proposes it, on the
/// forge its remote points at, whether pull requests are shown or not. GitHub is asked once,
/// and each Azure DevOps repository once, about their own remotes' branches only; `canned`,
/// `--pull-requests-from`'s list, is used instead if given. Blocks: run it on a worker thread.
pub fn proposals(git: &Git, branches: &[(String, String)], canned: Option<&str>) -> Vec<Proposed> {
    let urls = remote_urls(git).unwrap_or_default();
    let on: Vec<Option<(&str, Origin)>> = branches
        .iter()
        .map(|(remote, _)| {
            let (_, url) = urls.iter().find(|(name, _)| name == remote)?;
            Some((url.as_str(), Origin::from_url(url)?))
        })
        .collect();
    let mut found = vec![Proposed::NotOnForge; branches.len()];
    let mut github = Vec::new();
    let mut azure = Vec::new();
    for (i, (on, (_, branch))) in on.iter().zip(branches).enumerate() {
        match on {
            Some((_, Origin::Github(repo))) => github.push((i, (repo, branch.as_str()))),
            Some((url, Origin::AzureDevops(repo))) => {
                azure.push((i, (*url, repo, branch.as_str())))
            }
            None => {}
        }
    }
    if !github.is_empty() {
        let asked: Vec<_> = github.iter().map(|(_, b)| *b).collect();
        for ((i, _), proposed) in github.iter().zip(github::proposals(git, &asked, canned)) {
            found[*i] = proposed;
        }
    }
    if !azure.is_empty() {
        let asked: Vec<_> = azure.iter().map(|(_, b)| *b).collect();
        for ((i, _), proposed) in azure.iter().zip(azure::proposals(git, &asked, canned)) {
            found[*i] = proposed;
        }
    }
    found
}

/// Asks git only, not the forge.
pub fn origin(git: &Git) -> Option<Origin> {
    let url = git.query(&["remote", "get-url", "origin"]).ok()??;
    Origin::from_url(&url)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    /// The forge repository it points at: `owner/name` on GitHub, `org/project/name` on Azure
    /// DevOps.
    pub repo: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PullRequests {
    pub list: Vec<PullRequest>,
    /// Only those pointing at the forge.
    pub remotes: Vec<Remote>,
    /// `refs/heads/main` → `refs/remotes/origin/main`, for each local branch with an upstream.
    pub upstreams: HashMap<String, String>,
}

impl PullRequests {
    /// The pull requests whose heads are commits of `repo`, each with where it is shown. Its
    /// base branch's refs are the remote-tracking branches of every remote pointing at the pull
    /// request's repository, and the local branches tracking one of them; its head branch's is
    /// the first remote-tracking branch of a remote pointing at the head repository.
    pub fn heads(&self, repo: &Repo) -> Vec<(PullRequestHead, &PullRequest)> {
        let mut bases: HashMap<(String, &str), Vec<usize>> = HashMap::new();
        let mut tracking: HashMap<(String, &str), usize> = HashMap::new();
        for (i, r) in repo.refs.iter().enumerate() {
            let tracked = match r.kind {
                RefKind::RemoteBranch => Some(r.full_name.as_str()),
                RefKind::LocalBranch => self.upstreams.get(&r.full_name).map(String::as_str),
                _ => None,
            };
            if let Some(key) = tracked.and_then(|t| remote_branch(&self.remotes, t)) {
                if r.kind == RefKind::RemoteBranch {
                    tracking.entry(key.clone()).or_insert(i);
                }
                bases.entry(key).or_default().push(i);
            }
        }
        self.list
            .iter()
            .filter_map(|pr| {
                let commit = repo.lookup(&pr.head)?;
                let key = (pr.base_repo.to_ascii_lowercase(), pr.base_branch.as_str());
                let bases = bases.get(&key).cloned().unwrap_or_default();
                let head = pr.head_repo.as_ref().and_then(|head_repo| {
                    let key = (head_repo.to_ascii_lowercase(), pr.head_branch.as_str());
                    tracking.get(&key).copied()
                });
                Some((
                    PullRequestHead {
                        commit,
                        bases,
                        head,
                    },
                    pr,
                ))
            })
            .collect()
    }
}

/// The forge repository (`owner/name`, lowercase) and branch of `refs/remotes/<remote>/<branch>`.
/// Remote names may contain slashes: the longest remote that fits wins.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Forge {
    Github,
    AzureDevops,
}

impl Forge {
    /// Whether this build of parterre has its feature.
    pub fn supported(self) -> bool {
        match self {
            Forge::Github => cfg!(feature = "github"),
            Forge::AzureDevops => cfg!(feature = "azure-devops"),
        }
    }

    /// Whose sign-in parterre asks with, as an error message names it.
    fn account(self) -> &'static str {
        match self {
            Forge::Github => "gh's account",
            Forge::AzureDevops => "git's sign-in",
        }
    }
}

impl fmt::Display for Forge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Forge::Github => "GitHub",
            Forge::AzureDevops => "Azure DevOps",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    #[error("origin is not on GitHub or Azure DevOps")]
    NoForge,
    #[error("this build of parterre has no {0} support")]
    Unsupported(Forge),
    #[error("the GitHub CLI (gh) isn't installed")]
    NoGh,
    #[error("the GitHub CLI (gh) isn't signed in to github.com")]
    NotSignedIn,
    #[error("GitHub turned down the GitHub CLI's (gh's) sign-in")]
    TokenRejected,
    #[error("git has no sign-in for {url} that it can give without asking")]
    NoGitSignIn { url: String },
    #[error("Azure DevOps turned down git's sign-in for {url}")]
    GitSignInRejected { url: String },
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("could not reach {0}: {1}")]
    Network(Forge, String),
    #[error("GitHub's hourly limit is nearly used up; asking again in {minutes} min")]
    RateLimited { minutes: u64 },
    #[error("Azure DevOps asked parterre to wait; asking again in {minutes} min")]
    Throttled { minutes: u64 },
    #[error("{forge} has no repository {repo}, or {} can't see it", forge.account())]
    NotFound { forge: Forge, repo: String },
    #[error("{forge} answered {status}: {message}")]
    Status {
        forge: Forge,
        status: u16,
        message: String,
    },
    #[error("unexpected answer from {0}: {1}")]
    Parse(Forge, String),
    /// From `--pull-requests-from`'s file.
    #[error("pull requests file: {0}")]
    Canned(String),
}

impl ForgeError {
    pub fn needs_sign_in(&self) -> bool {
        matches!(
            self,
            ForgeError::NoGh
                | ForgeError::NotSignedIn
                | ForgeError::TokenRejected
                | ForgeError::NoGitSignIn { .. }
                | ForgeError::GitSignInRejected { .. }
        )
    }

    /// How long the forge asked to be left alone, if it did.
    pub fn wait(&self) -> Option<Duration> {
        match self {
            ForgeError::RateLimited { minutes } | ForgeError::Throttled { minutes } => {
                Some(Duration::from_secs(minutes * 60))
            }
            _ => None,
        }
    }

    pub fn explain(&self) -> Explanation {
        let sign_in = || Some("gh auth login".to_owned());
        // Asked for its refs, git signs in to the repository, with Git Credential Manager
        // keeping the sign-in, and changes nothing. Over HTTPS also for an SSH origin.
        let git_sign_in = |url: &str| Some(format!("git ls-remote {url}"));
        let (title, body, command) = match self {
            ForgeError::NoGh => (
                "GitHub CLI not found".to_owned(),
                "parterre gets pull requests from GitHub through the GitHub CLI (gh), which \
                 isn't installed. Install it, sign in with this command in a terminal, and \
                 turn pull requests on again."
                    .to_owned(),
                sign_in(),
            ),
            ForgeError::NotSignedIn => (
                "Not signed in to GitHub".to_owned(),
                "parterre gets pull requests from GitHub through the GitHub CLI (gh), which \
                 isn't signed in. Sign in with this command in a terminal, and turn pull \
                 requests on again."
                    .to_owned(),
                sign_in(),
            ),
            ForgeError::TokenRejected => (
                "GitHub didn't accept the sign-in".to_owned(),
                "The GitHub CLI's sign-in may have expired. Sign in again with this command in \
                 a terminal, and turn pull requests on again."
                    .to_owned(),
                sign_in(),
            ),
            ForgeError::NoGitSignIn { url } => (
                "Not signed in to Azure DevOps".to_owned(),
                "parterre asks Azure DevOps with the sign-in git has for origin, and git has \
                 none it can give without asking. Run this command in a terminal, signing in \
                 when asked (Git Credential Manager keeps the sign-in), and turn pull requests \
                 on again."
                    .to_owned(),
                git_sign_in(url),
            ),
            ForgeError::GitSignInRejected { url } => (
                "Azure DevOps didn't accept the sign-in".to_owned(),
                "The sign-in git has for origin may have expired, or not be allowed to read \
                 code. Run this command in a terminal, signing in again if asked, and turn \
                 pull requests on again."
                    .to_owned(),
                git_sign_in(url),
            ),
            ForgeError::RateLimited { minutes } => (
                "GitHub's hourly limit is nearly used up".to_owned(),
                format!(
                    "parterre stops asking GitHub while less than a tenth of your hourly \
                     allowance is left, so that your other tools keep working. Pull requests \
                     appear by themselves in about {minutes} min."
                ),
                None,
            ),
            ForgeError::Throttled { minutes } => (
                "Azure DevOps asked parterre to wait".to_owned(),
                format!(
                    "Azure DevOps is slowing down your requests. Pull requests appear by \
                     themselves in about {minutes} min."
                ),
                None,
            ),
            ForgeError::NotFound {
                forge: Forge::Github,
                repo,
            } => (
                "Repository not found on GitHub".to_owned(),
                format!(
                    "GitHub has no repository {repo}, or your account can't see it. For a \
                     private repository, check which account the GitHub CLI is signed in as:"
                ),
                Some("gh auth status".to_owned()),
            ),
            ForgeError::NotFound {
                forge: Forge::AzureDevops,
                repo,
            } => (
                "Repository not found on Azure DevOps".to_owned(),
                format!(
                    "Azure DevOps has no repository {repo}, or the account git signs in to it \
                     with can't see it."
                ),
                None,
            ),
            ForgeError::Network(forge, why) => (
                format!("Couldn't reach {forge}"),
                format!(
                    "{}. Check the network connection, and turn pull requests on again.",
                    capitalized(why)
                ),
                None,
            ),
            other => (
                "Couldn't show pull requests".to_owned(),
                format!("{}.", capitalized(&other.to_string())),
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Explanation {
    pub title: String,
    pub body: String,
    /// To run in a terminal.
    pub command: Option<String>,
}

/// Never printed: its `Debug` shows stars.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Token(String);

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(***)")
    }
}

/// What `cmd` prints with `input` on its stdin, if it succeeds within `timeout`: `Ok(None)` if
/// it fails, or takes longer and is stopped. Errs if it can't be started. Its errors are
/// dropped.
pub(crate) fn output_within(
    cmd: &mut Command,
    input: &str,
    timeout: Duration,
) -> std::io::Result<Option<String>> {
    use std::io::{Read as _, Write as _};

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        // A GUI-subsystem app on Windows; without this a console window would flash up.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn()?;
    // Written and read from threads of their own, so that neither pipe can stall it.
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let input = input.to_owned();
    std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        let _ = tx.send(out);
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            // What it started may hold the pipe open after it exits: waited for until the
            // deadline at most, and then left to finish alone.
            Ok(Some(status)) if status.success() => {
                let left = deadline.saturating_duration_since(Instant::now());
                return Ok(rx.recv_timeout(left).ok());
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
        }
    }
}

#[cfg(any(feature = "github", feature = "azure-devops"))]
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

#[cfg(any(feature = "github", feature = "azure-devops"))]
fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    // The system's certificate authorities, including any a company adds.
    let tls = TlsConfig::builder()
        .root_certs(RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(Duration::from_secs(30)))
        // Error answers carry the reason, and the rate limit in their headers.
        .http_status_as_error(false)
        .max_redirects(0)
        .user_agent(concat!("parterre/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// How long a loaded list is used before GitHub is asked again, as in t3code.
pub fn fresh_for(found_any: bool) -> Duration {
    Duration::from_secs(if found_any { 60 } else { 5 * 60 })
}

/// How long after [`origin_moved`] GitHub is asked once more, however fresh the list: a pull
/// request is usually opened seconds after its branch is pushed, after the load the push
/// started (#294).
pub const RECHECK_AFTER_PUSH: Duration = Duration::from_secs(60);

/// How long to wait after `failures` failed loads in a row, as in t3code.
pub fn retry_after(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(6);
    Duration::from_secs((20u64 << doublings).min(15 * 60))
}

/// Whether `origin`'s branches differ between two snapshots: they are what GitHub is asked
/// about.
pub fn origin_moved(old: &Repo, new: &Repo) -> bool {
    fn branches(repo: &Repo) -> Vec<(&str, Oid)> {
        let mut branches: Vec<_> = repo
            .refs
            .iter()
            .filter(|r| {
                r.kind == RefKind::RemoteBranch && r.full_name.starts_with("refs/remotes/origin/")
            })
            .map(|r| (r.full_name.as_str(), repo.commit(r.target).oid))
            .collect();
        branches.sort_unstable();
        branches
    }
    branches(old) != branches(new)
}

/// Each remote's name and fetch URL, with `insteadOf` applied.
pub fn remote_urls(git: &Git) -> Result<Vec<(String, String)>, GitError> {
    Ok(parse_remote_urls(&git.run(&["remote", "-v"])?))
}

/// In a partial clone the fetch line ends in the filter: `(fetch) [tree:0]`.
fn parse_remote_urls(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once('\t')?;
            let (url, _) = rest.split_once(" (fetch)")?;
            Some((name.to_owned(), url.to_owned()))
        })
        .collect()
}

/// Pull requests from `json` (`--pull-requests-from`), for screenshots and demos without the
/// network: an array of `{"number", "title", "author", "draft", "head", "base"}`, where `head`
/// and `base` are branches of `origin`, the pull requests' repository, `repo` on its forge.
/// Each one is on the commit `origin/<head>` is at; `url` makes its page from its number.
fn canned(
    git: &Git,
    json: &str,
    repo: &str,
    url: impl Fn(u64) -> String,
) -> Result<Vec<PullRequest>, ForgeError> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Canned {
        number: u64,
        title: String,
        #[serde(default)]
        author: String,
        #[serde(default)]
        draft: bool,
        head: String,
        base: String,
    }
    let canned: Vec<Canned> =
        serde_json::from_str(json).map_err(|e| ForgeError::Canned(e.to_string()))?;
    canned
        .into_iter()
        .map(|pr| {
            let tip = format!("refs/remotes/origin/{}^{{commit}}", pr.head);
            let head = git
                .query(&["rev-parse", "--verify", "--quiet", &tip])?
                .and_then(|hex| Oid::from_hex(&hex))
                .ok_or_else(|| ForgeError::Canned(format!("origin has no branch {}", pr.head)))?;
            Ok(PullRequest {
                url: url(pr.number),
                number: pr.number,
                title: pr.title,
                author: pr.author,
                draft: pr.draft,
                head,
                head_branch: pr.head,
                head_repo: Some(repo.to_owned()),
                base_branch: pr.base,
                base_repo: repo.to_owned(),
            })
        })
        .collect()
}

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
    use parterre_core::repo::{Commit, CommitIx, GitRef, Head};

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
        let (kind, name) = parterre_core::git::classify_ref(full_name);
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
        let heads: Vec<(u64, u32, Vec<usize>, Option<usize>)> = prs
            .heads(&repo)
            .into_iter()
            .map(|(h, pr)| (pr.number, h.commit.0, h.bases, h.head))
            .collect();
        assert_eq!(
            heads,
            [
                // Owner and name compare without case, as on GitHub. origin/sub/main is the
                // branch sub/main, and the tag is no branch.
                // The fork's feature is origin/feature; the parent's isn't fetched.
                (12, 1, vec![0, 2], Some(4)),
                (10, 1, vec![1, 3], None),
                (9, 0, vec![], Some(4)),
            ]
        );
    }

    #[test]
    fn origin_moves_with_its_own_branches_only() {
        let repo = |refs: &[(&str, u32)]| {
            Repo::new(
                "/x".into(),
                vec![commit(1), commit(2)],
                refs.iter()
                    .map(|&(name, target)| git_ref(name, target))
                    .collect(),
                Head::Detached(CommitIx(0)),
            )
        };
        let before = repo(&[
            ("refs/heads/main", 0),
            ("refs/remotes/origin/main", 0),
            ("refs/remotes/upstream/main", 0),
        ]);
        // A commit, and a fetch from another remote: no.
        let after = repo(&[
            ("refs/heads/main", 1),
            ("refs/remotes/origin/main", 0),
            ("refs/remotes/upstream/main", 1),
        ]);
        assert!(!origin_moved(&before, &after));
        // A push of a new branch, and of a branch that moved: yes.
        let pushed = repo(&[
            ("refs/heads/main", 0),
            ("refs/remotes/origin/main", 0),
            ("refs/remotes/origin/topic", 1),
            ("refs/remotes/upstream/main", 0),
        ]);
        assert!(origin_moved(&before, &pushed));
        let moved = repo(&[
            ("refs/heads/main", 0),
            ("refs/remotes/origin/main", 1),
            ("refs/remotes/upstream/main", 0),
        ]);
        assert!(origin_moved(&before, &moved));
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
        assert_eq!(e.command.as_deref(), Some("gh auth login"));
        assert!(e.body.ends_with("turn pull requests on again."));
        assert_eq!(
            ForgeError::NoGh.explain().command.as_deref(),
            Some("gh auth login")
        );
        let e = ForgeError::RateLimited { minutes: 12 }.explain();
        assert!(e.body.contains("in about 12 min"), "{}", e.body);
        assert_eq!(e.command, None);
        let e = ForgeError::Network(Forge::Github, "connection refused".into()).explain();
        assert!(e.body.starts_with("Connection refused. "), "{}", e.body);
        let e = ForgeError::Parse(Forge::Github, "bad JSON".into()).explain();
        assert_eq!(e.body, "Unexpected answer from GitHub: bad JSON.");
    }

    #[test]
    fn tokens_are_not_printed() {
        let token = Token("gho_secret".into());
        assert_eq!(format!("{token:?}"), "Token(***)");
        assert!(!format!("{:?}", Some(token)).contains("secret"));
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
