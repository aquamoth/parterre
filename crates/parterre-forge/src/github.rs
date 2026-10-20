//! Open pull requests from github.com (no GitHub Enterprise yet), asked for as t3code does:
//!
//! - **Which:** those of `origin`'s repository and, if it is a fork, the fork's own into its
//!   parent (research §12). Only those whose head is a fetched branch of `origin` are asked
//!   for, since only they can be shown: all of pingdotgg/t3code's open pull requests took 23 s.
//! - **How:** one GraphQL request per 100 such branches, a `pullRequests(headRefName:)`
//!   connection per branch on `origin` and its parent, with only the fields shown. As with
//!   t3code's `gh pr list --head <branch>`, GitHub matches the branch name and the head
//!   repository is checked here: another fork's `main` is not ours.
//! - **Signed in only:** with `gh auth token`'s token, or not at all.
//! - **Within a budget:** once less than a tenth of the hour's points is left, as t3code keeps
//!   in reserve, or GitHub says to wait, nothing is asked until the limit resets.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use super::{
    Forge, ForgeError, GoneBranch, MERGED_PER_BRANCH, Proposed, PullRequest, PullRequests, Remote,
    RemoteUrl,
};
#[cfg(feature = "github")]
use crate::http::{self, Token};
use parterre_core::git::{Git, GitError};
use parterre_core::merged::Merged;
use parterre_core::oid::Oid;

mod repositories;
pub use repositories::{Protocol, repositories, repositories_canned};

const WEB: &str = "https://github.com/";
const BRANCHES_PER_REQUEST: usize = 100;
/// As t3code: GitHub prices a connection of 100 like one of 1, and a name such as `main` can
/// have many other forks' pull requests.
const PULL_REQUESTS_PER_BRANCH: usize = 100;
const GH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubRepo {
    pub owner: String,
    pub name: String,
}

impl GithubRepo {
    /// The repository a remote URL points at, if it is on github.com: `https://github.com/o/r`,
    /// `git@github.com:o/r.git`, `ssh://git@ssh.github.com:443/o/r` and the like.
    pub fn from_url(url: &str) -> Option<GithubRepo> {
        let url = RemoteUrl::parse(url)?;
        if let Some(scheme) = &url.scheme {
            let scheme = scheme.strip_prefix("git+").unwrap_or(scheme);
            if !matches!(scheme, "https" | "http" | "ssh" | "git") {
                return None;
            }
        }
        if !matches!(
            url.host.as_str(),
            "github.com" | "www.github.com" | "ssh.github.com"
        ) {
            return None;
        }
        let path = url.path.trim_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        let (owner, name) = path.split_once('/')?;
        // Enterprise Managed Users' logins end in `_shortcode`.
        let owner_ok = !owner.is_empty()
            && owner
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
        let name_ok = !name.is_empty()
            && name != "."
            && name != ".."
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
        (owner_ok && name_ok).then(|| GithubRepo {
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    }

    fn from_full_name(full_name: &str) -> Option<GithubRepo> {
        GithubRepo::from_url(&format!("{WEB}{full_name}"))
    }

    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// Asks git only, not GitHub.
pub fn origin(git: &Git) -> Option<GithubRepo> {
    let url = git.query(&["remote", "get-url", "origin"]).ok()??;
    GithubRepo::from_url(&url)
}

/// Asks nothing if no branch of `origin` has been fetched. Blocks: run it on a worker thread.
pub fn load(git: &Git) -> Result<PullRequests, ForgeError> {
    pull_requests(git, |origin| {
        let branches = remote_branches(git, "origin")?;
        if branches.is_empty() {
            return Ok(Found {
                name: origin.full_name(),
                list: Vec::new(),
                pause_until: None,
            });
        }
        with_api(|api| open_pull_requests(api, origin, &branches, HeadsFrom::Origin))
    })
}

/// For each `(repository, branch)`: whether an open pull request into `origin`'s repository
/// or its parent proposes it, a fork's too. Asks GitHub once, about these branches only;
/// `canned`, `--pull-requests-from`'s list, is used instead if given.
pub(crate) fn proposals(
    git: &Git,
    branches: &[(&GithubRepo, &str)],
    canned: Option<&str>,
) -> Vec<Proposed> {
    let found = match canned {
        Some(json) => load_canned(git, json).map(|p| p.list),
        None => {
            let mut names: Vec<String> = branches.iter().map(|(_, b)| (*b).to_owned()).collect();
            names.sort();
            names.dedup();
            origin(git).ok_or(ForgeError::NoForge).and_then(|origin| {
                with_api(|api| open_pull_requests(api, &origin, &names, HeadsFrom::AnyRepository))
                    .map(|found| found.list)
            })
        }
    };
    branches
        .iter()
        .map(|(repo, branch)| Proposed::among(&found, &repo.full_name(), branch))
        .collect()
}

/// For each of `branches`: the merged pull request from it into `origin`'s repository or its
/// parent, whose head was its tip when merged. Asks GitHub once per 100 branch names.
pub(crate) fn merged(
    origin: &GithubRepo,
    branches: &[GoneBranch],
) -> Result<Vec<Option<Merged>>, ForgeError> {
    let mut names: Vec<String> = branches.iter().map(|b| b.name.clone()).collect();
    names.sort();
    names.dedup();
    let found = with_api(|api| merged_pull_requests(api, origin, &names))?;
    if let Some(until) = found.pause_until {
        pause(until);
    }
    Ok(merged_of(&found.list, branches))
}

/// Each branch's pull request among `found`: from it, with its tip as the head.
fn merged_of(found: &[MergedPullRequest], branches: &[GoneBranch]) -> Vec<Option<Merged>> {
    branches
        .iter()
        .map(|b| {
            let pr = found
                .iter()
                .find(|pr| pr.head_branch == b.name && pr.head == b.tip)?;
            Some(Merged {
                how: None,
                landed: pr.merge_commit,
                pull_request: Some(pr.number),
            })
        })
        .collect()
}

/// Pull requests from `json` instead of GitHub, for screenshots and demos without the network
/// (`--pull-requests-from`): an array of `{"number", "title", "author", "draft", "head",
/// "base"}`, where `head` and `base` are branches of `origin`, which must still point at
/// GitHub. Each one is on the commit `origin/<head>` is at.
pub fn load_canned(git: &Git, json: &str) -> Result<PullRequests, ForgeError> {
    pull_requests(git, |origin| {
        let name = origin.full_name();
        let list = super::canned(git, json, &name, |number| {
            format!("{WEB}{name}/pull/{number}")
        })?;
        Ok(Found {
            name,
            list,
            pause_until: None,
        })
    })
}

/// The pull requests `find` finds for `origin`, with the remotes and upstreams that place them.
fn pull_requests(
    git: &Git,
    find: impl FnOnce(&GithubRepo) -> Result<Found, ForgeError>,
) -> Result<PullRequests, ForgeError> {
    let urls = super::remote_urls(git)?;
    let origin = urls
        .iter()
        .find(|(name, _)| name == "origin")
        .and_then(|(_, url)| GithubRepo::from_url(url))
        .ok_or(ForgeError::NoForge)?;
    let upstreams = super::upstreams(git)?;
    let found = find(&origin)?;
    if let Some(until) = found.pause_until {
        pause(until);
    }
    let origin_name = origin.full_name();
    let remotes = urls
        .iter()
        .filter_map(|(name, url)| {
            let repo = GithubRepo::from_url(url)?.full_name();
            // A renamed repository answers under its new name; remotes may still use the old.
            let repo = if repo.eq_ignore_ascii_case(&origin_name) {
                found.name.clone()
            } else {
                repo
            };
            Some(Remote {
                name: name.clone(),
                repo,
            })
        })
        .collect();
    Ok(PullRequests {
        list: found.list,
        remotes,
        upstreams,
    })
}

fn remote_branches(git: &Git, remote: &str) -> Result<Vec<String>, GitError> {
    let prefix = format!("refs/remotes/{remote}/");
    let out = git.run(&["for-each-ref", "--format=%(refname)%00%(symref)", &prefix])?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let (name, symref) = line.split_once('\0')?;
            // `origin/HEAD` names another branch.
            if !symref.is_empty() {
                return None;
            }
            name.strip_prefix(&prefix).map(str::to_owned)
        })
        .collect())
}

/// Unix time until which GitHub is not asked, process-wide: the budget is the user's, whatever
/// the repository.
static PAUSED_UNTIL: Mutex<u64> = Mutex::new(0);

/// `until` is Unix time.
fn pause(until: u64) {
    let mut paused = PAUSED_UNTIL.lock().unwrap_or_else(PoisonError::into_inner);
    *paused = (*paused).max(until);
}

/// GitHub's rate-limit headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Quota {
    pub remaining: u64,
    pub limit: u64,
    /// Unix time.
    pub reset: u64,
}

impl Quota {
    fn pause_until(self) -> Option<u64> {
        (self.remaining * 10 < self.limit).then_some(self.reset)
    }
}

#[derive(Debug)]
pub(crate) struct Response {
    pub body: String,
    pub quota: Option<Quota>,
}

/// GraphQL requests; a trait so that tests can answer them.
pub(crate) trait Api {
    fn post(&self, body: &str) -> Result<Response, ForgeError>;
}

#[derive(Debug)]
pub(crate) struct Found<P = PullRequest> {
    /// `origin`'s `owner/name` as GitHub has it now.
    pub name: String,
    pub list: Vec<P>,
    /// Unix time.
    pub pause_until: Option<u64>,
}

/// A merged pull request from a branch of `origin`'s own repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MergedPullRequest {
    pub number: u64,
    /// As it was when merged.
    pub head: Oid,
    pub head_branch: String,
    pub merge_commit: Option<Oid>,
}

/// Which pull requests from the branches asked about are kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeadsFrom {
    Origin,
    /// A fork's too.
    AnyRepository,
}

/// The open pull requests whose head is one of `branches`: into `origin`, and into its parent
/// if it is a fork. By base repository, newest first.
fn open_pull_requests(
    api: &dyn Api,
    origin: &GithubRepo,
    branches: &[String],
    heads_from: HeadsFrom,
) -> Result<Found, ForgeError> {
    let mut found = Found {
        name: origin.full_name(),
        list: Vec::new(),
        pause_until: None,
    };
    for chunk in branches.chunks(BRANCHES_PER_REQUEST) {
        let response = api.post(&request(origin, chunk, Asked::Open))?;
        found.pause_until = found
            .pause_until
            .max(response.quota.and_then(Quota::pause_until));
        let repo: json::Repository<json::PullRequest> = repository(&response.body, origin)?;
        found.name = repo.name_with_owner.clone();
        let is_kept = |pr: &json::PullRequest| match heads_from {
            HeadsFrom::AnyRepository => true,
            HeadsFrom::Origin => pr.head_repository.as_ref().is_some_and(|r| {
                r.name_with_owner
                    .eq_ignore_ascii_case(&repo.name_with_owner)
            }),
        };
        let mut add = |base: &str, connections: &HashMap<String, json::Connection<_>>| {
            let Some(base) = GithubRepo::from_full_name(base) else {
                return;
            };
            found.list.extend(
                connections
                    .values()
                    .flat_map(|c| &c.nodes)
                    .filter(|pr| is_kept(pr))
                    .filter_map(|pr| pr.to_pull_request(&base)),
            );
        };
        add(&repo.name_with_owner, &repo.connections);
        if let Some(parent) = &repo.parent {
            add(&parent.name_with_owner, &parent.connections);
        }
    }
    found.list.sort_by(|a, b| {
        a.base_repo
            .cmp(&b.base_repo)
            .then_with(|| b.number.cmp(&a.number))
    });
    found
        .list
        .dedup_by(|a, b| a.base_repo == b.base_repo && a.number == b.number);
    Ok(found)
}

/// The merged pull requests whose head is one of `branches` of `origin`'s own repository,
/// into it, and into its parent if it is a fork.
fn merged_pull_requests(
    api: &dyn Api,
    origin: &GithubRepo,
    branches: &[String],
) -> Result<Found<MergedPullRequest>, ForgeError> {
    let mut found = Found {
        name: origin.full_name(),
        list: Vec::new(),
        pause_until: None,
    };
    for chunk in branches.chunks(BRANCHES_PER_REQUEST) {
        let response = api.post(&request(origin, chunk, Asked::Merged))?;
        found.pause_until = found
            .pause_until
            .max(response.quota.and_then(Quota::pause_until));
        let repo: json::Repository<json::MergedPullRequest> = repository(&response.body, origin)?;
        found.name = repo.name_with_owner.clone();
        let own = |pr: &&json::MergedPullRequest| {
            pr.head_repository.as_ref().is_some_and(|r| {
                r.name_with_owner
                    .eq_ignore_ascii_case(&repo.name_with_owner)
            })
        };
        let parent = repo.parent.iter().flat_map(|p| p.connections.values());
        found.list.extend(
            repo.connections
                .values()
                .chain(parent)
                .flat_map(|c| &c.nodes)
                .filter(own)
                .filter_map(json::MergedPullRequest::to_merged),
        );
    }
    Ok(found)
}

/// Which pull requests [`request`] asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asked {
    Open,
    /// Newest first.
    Merged,
}

/// A connection `b<i>` per branch, on `origin` and on its parent. Branch names go in as
/// variables, never into the query text.
fn request(origin: &GithubRepo, branches: &[String], asked: Asked) -> String {
    let (state, rest, fields) = match asked {
        Asked::Open => (
            "OPEN",
            format!("first:{PULL_REQUESTS_PER_BRANCH}"),
            "number title isDraft headRefOid headRefName headRepository{nameWithOwner} \
             baseRefName author{login}",
        ),
        Asked::Merged => (
            "MERGED",
            format!("first:{MERGED_PER_BRANCH},orderBy:{{field:CREATED_AT,direction:DESC}}"),
            "number headRefOid headRefName headRepository{nameWithOwner} mergeCommit{oid}",
        ),
    };
    let connections: String = (0..branches.len())
        .map(|i| {
            format!("b{i}:pullRequests(states:{state},headRefName:$b{i},{rest}){{nodes{{...pr}}}}")
        })
        .collect();
    let params: String = (0..branches.len())
        .map(|i| format!(",$b{i}:String!"))
        .collect();
    let query = format!(
        "query($owner:String!,$name:String!{params}){{repository(owner:$owner,name:$name){{\
         nameWithOwner parent{{nameWithOwner {connections}}}{connections}}}}}\
         fragment pr on PullRequest{{{fields}}}"
    );
    let mut variables = serde_json::Map::new();
    variables.insert("owner".into(), origin.owner.clone().into());
    variables.insert("name".into(), origin.name.clone().into());
    for (i, branch) in branches.iter().enumerate() {
        variables.insert(format!("b{i}"), branch.clone().into());
    }
    serde_json::json!({ "query": query, "variables": variables }).to_string()
}

fn repository<P: serde::de::DeserializeOwned>(
    body: &str,
    origin: &GithubRepo,
) -> Result<json::Repository<P>, ForgeError> {
    let answer: json::Answer<json::Data<P>> = super::parse_answer(Forge::Github, body)?;
    if let Some(repo) = answer.data.and_then(|d| d.repository) {
        return Ok(repo);
    }
    Err(match answer.errors.into_iter().next() {
        Some(e) if e.kind.as_deref() == Some("NOT_FOUND") => ForgeError::NotFound {
            forge: Forge::Github,
            repo: origin.full_name(),
        },
        Some(e) => e.into_forge_error(),
        None => ForgeError::Parse(Forge::Github, "no repository in the answer".into()),
    })
}

mod json {
    use std::collections::HashMap;

    use serde::Deserialize;

    use super::{Forge, ForgeError, GithubRepo, WEB};
    use parterre_core::oid::Oid;

    #[derive(Debug, Deserialize)]
    pub struct Answer<D> {
        pub data: Option<D>,
        #[serde(default)]
        pub errors: Vec<Error>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Data<P> {
        pub repository: Option<Repository<P>>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Error {
        #[serde(rename = "type")]
        pub kind: Option<String>,
        pub message: String,
    }

    impl Error {
        pub fn into_forge_error(self) -> ForgeError {
            if self.kind.as_deref() == Some("RATE_LIMITED") {
                ForgeError::RateLimited { minutes: 60 }
            } else {
                ForgeError::Status {
                    forge: Forge::Github,
                    status: 200,
                    message: self.message,
                }
            }
        }
    }

    /// With the pull requests `P` of each branch asked about.
    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase", bound(deserialize = "P: Deserialize<'de>"))]
    pub struct Repository<P> {
        pub name_with_owner: String,
        #[serde(default)]
        pub parent: Option<Parent<P>>,
        /// `b0`, `b1`, …: the pull requests of each branch asked about.
        #[serde(flatten)]
        pub connections: HashMap<String, Connection<P>>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Parent<P> {
        pub name_with_owner: String,
        #[serde(flatten)]
        pub connections: HashMap<String, Connection<P>>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Connection<P> {
        pub nodes: Vec<P>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct MergedPullRequest {
        number: u64,
        head_ref_oid: String,
        head_ref_name: String,
        pub head_repository: Option<Named>,
        merge_commit: Option<Commit>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Commit {
        oid: String,
    }

    impl MergedPullRequest {
        /// `None` if its head isn't a commit id.
        pub fn to_merged(&self) -> Option<super::MergedPullRequest> {
            Some(super::MergedPullRequest {
                number: self.number,
                head: Oid::from_hex(&self.head_ref_oid)?,
                head_branch: self.head_ref_name.clone(),
                merge_commit: self
                    .merge_commit
                    .as_ref()
                    .and_then(|c| Oid::from_hex(&c.oid)),
            })
        }
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PullRequest {
        number: u64,
        title: String,
        #[serde(default)]
        is_draft: bool,
        head_ref_oid: String,
        head_ref_name: String,
        pub head_repository: Option<Named>,
        base_ref_name: String,
        author: Option<Login>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Named {
        pub name_with_owner: String,
    }

    #[derive(Debug, Deserialize)]
    pub struct Login {
        login: String,
    }

    impl PullRequest {
        /// `None` if its head isn't a commit id. The URL is built here, not taken from the
        /// answer, so that only github.com is ever opened.
        pub fn to_pull_request(&self, base: &GithubRepo) -> Option<crate::PullRequest> {
            Some(crate::PullRequest {
                number: self.number,
                title: self.title.clone(),
                author: self
                    .author
                    .as_ref()
                    .map(|a| a.login.clone())
                    .unwrap_or_default(),
                draft: self.is_draft,
                head: Oid::from_hex(&self.head_ref_oid)?,
                head_branch: self.head_ref_name.clone(),
                head_repo: self
                    .head_repository
                    .as_ref()
                    .map(|r| r.name_with_owner.clone()),
                base_branch: self.base_ref_name.clone(),
                base_repo: base.full_name(),
                url: format!("{WEB}{}/pull/{}", base.full_name(), self.number),
            })
        }
    }
}

/// What `gh` prints for `args`: [`ForgeError::NoGh`] if it isn't installed,
/// [`ForgeError::NotSignedIn`] if it fails or doesn't answer within [`GH_TIMEOUT`].
fn gh(args: &[&str]) -> Result<String, ForgeError> {
    let mut cmd = std::process::Command::new("gh");
    parterre_core::shell_path::apply(&mut cmd)
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1");
    match super::output_within(&mut cmd, "", GH_TIMEOUT) {
        Ok(Some(out)) => Ok(out),
        Ok(None) => Err(ForgeError::NotSignedIn),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ForgeError::NoGh),
        Err(e) => Err(ForgeError::Network(
            Forge::Github,
            format!("could not run gh: {e}"),
        )),
    }
}

#[cfg(feature = "github")]
fn gh_token() -> Result<Token, ForgeError> {
    let out = gh(&["auth", "token", "--hostname", "github.com"])?;
    let token = out.trim();
    // Tokens are letters, digits and underscores; anything else is not one.
    let plausible = !token.is_empty()
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_');
    plausible
        .then(|| Token(token.to_owned()))
        .ok_or(ForgeError::NotSignedIn)
}

/// Runs `calls` against the API, signed in with `gh`'s token; without one, or while the
/// budget is paused, GitHub is not asked.
#[cfg(feature = "github")]
fn with_api<T>(calls: impl Fn(&dyn Api) -> Result<T, ForgeError>) -> Result<T, ForgeError> {
    if let Some(wait) = paused_for() {
        return Err(ForgeError::RateLimited {
            minutes: wait.as_secs().div_ceil(60),
        });
    }
    let token = gh_token()?;
    calls(&Http::new(token))
}

#[cfg(not(feature = "github"))]
fn with_api<T>(_: impl Fn(&dyn Api) -> Result<T, ForgeError>) -> Result<T, ForgeError> {
    Err(ForgeError::Unsupported(Forge::Github))
}

#[cfg(feature = "github")]
fn paused_for() -> Option<Duration> {
    let until = *PAUSED_UNTIL.lock().unwrap_or_else(PoisonError::into_inner);
    let now = unix_now();
    (until > now).then(|| Duration::from_secs(until - now))
}

#[cfg(feature = "github")]
fn unix_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Tokens are only ever sent here.
#[cfg(feature = "github")]
const GRAPHQL: &str = "https://api.github.com/graphql";

#[cfg(feature = "github")]
#[derive(Debug)]
struct Http {
    agent: ureq::Agent,
    token: Token,
}

#[cfg(feature = "github")]
impl Http {
    fn new(token: Token) -> Http {
        Http {
            agent: http::agent(),
            token,
        }
    }
}

#[cfg(feature = "github")]
impl Api for Http {
    fn post(&self, body: &str) -> Result<Response, ForgeError> {
        let Token(token) = &self.token;
        let mut response = self
            .agent
            .post(GRAPHQL)
            .header("Authorization", &format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(http::network_error(Forge::Github))?;
        let header = |name| http::number_header(&response, name);
        let quota = match (
            header("x-ratelimit-remaining"),
            header("x-ratelimit-limit"),
            header("x-ratelimit-reset"),
        ) {
            (Some(remaining), Some(limit), Some(reset)) => Some(Quota {
                remaining,
                limit,
                reset,
            }),
            _ => None,
        };
        // GitHub's secondary limits say how long to wait.
        let retry_after = header("retry-after");
        let status = response.status().as_u16();
        let body = http::read_body(&mut response, Forge::Github)?;
        match status {
            200..=299 => Ok(Response { body, quota }),
            401 => Err(ForgeError::TokenRejected),
            403 | 429 if retry_after.is_some() || quota.is_some_and(|q| q.remaining == 0) => {
                let until = match (retry_after, quota) {
                    (Some(secs), _) => unix_now() + secs,
                    (None, Some(q)) => q.reset,
                    (None, None) => unix_now() + 60,
                };
                pause(until);
                Err(ForgeError::RateLimited {
                    minutes: until.saturating_sub(unix_now()).div_ceil(60),
                })
            }
            _ => Err(http::status_error(Forge::Github, status, &body)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn repo(owner: &str, name: &str) -> Option<GithubRepo> {
        Some(GithubRepo {
            owner: owner.into(),
            name: name.into(),
        })
    }

    #[test]
    fn github_remote_urls() {
        for url in [
            "https://github.com/aquamoth/parterre",
            "https://github.com/aquamoth/parterre.git",
            "https://github.com/aquamoth/parterre/",
            "https://user@github.com/aquamoth/parterre.git",
            "HTTPS://GitHub.com/aquamoth/parterre",
            "http://www.github.com/aquamoth/parterre",
            "git+https://github.com/aquamoth/parterre",
            "ssh://git@github.com/aquamoth/parterre.git",
            "ssh://git@ssh.github.com:443/aquamoth/parterre.git",
            "git://github.com/aquamoth/parterre",
            "git@github.com:aquamoth/parterre.git",
            "github.com:aquamoth/parterre",
            " git@github.com:aquamoth/parterre \n",
        ] {
            assert_eq!(
                GithubRepo::from_url(url),
                repo("aquamoth", "parterre"),
                "{url}"
            );
        }
        assert_eq!(
            GithubRepo::from_url("git@github.com:my-org/some.repo_name.git"),
            repo("my-org", "some.repo_name")
        );
        assert_eq!(
            GithubRepo::from_url("https://github.com/jdoe_acme/r"),
            repo("jdoe_acme", "r")
        );
    }

    #[test]
    fn other_remote_urls() {
        for url in [
            "https://gitlab.com/aquamoth/parterre",
            "https://github.com.evil.example/aquamoth/parterre",
            "https://dev.azure.com/org/project/_git/repo",
            "git@bitbucket.org:aquamoth/parterre.git",
            "/home/me/src/parterre",
            "../parterre",
            "C:\\src\\parterre",
            "file:///srv/github.com/aquamoth/parterre",
            "https://github.com/aquamoth",
            "https://github.com/aquamoth/parterre/tree/main",
            "https://github.com/aquamoth/..",
            "https://github.com/aqua moth/parterre",
            "https://github.com/aquamoth/parterre?x=1",
            "",
        ] {
            assert_eq!(GithubRepo::from_url(url), None, "{url}");
        }
    }

    impl Api for Fake<Response> {
        fn post(&self, body: &str) -> Result<Response, ForgeError> {
            self.answer(Forge::Github, body)
        }
    }

    /// Answers each request with the next of `bodies`.
    pub(super) fn fake(bodies: &[&str], quota: Option<Quota>) -> Fake<Response> {
        Fake::new(bodies.iter().map(|body| {
            Ok(Response {
                body: (*body).to_owned(),
                quota,
            })
        }))
    }

    pub(super) fn requests(api: &Fake<Response>) -> Vec<serde_json::Value> {
        api.asked
            .borrow()
            .iter()
            .map(|request| serde_json::from_str(request).expect("requests are JSON"))
            .collect()
    }

    fn pr(number: u64, head_repo: Option<&str>, head: char) -> String {
        let repo = head_repo.map_or("null".to_owned(), |r| {
            format!(r#"{{"nameWithOwner":"{r}"}}"#)
        });
        format!(
            r#"{{"number":{number},"title":"PR {number}","isDraft":{draft},
                "headRefOid":"{sha}","headRefName":"topic","headRepository":{repo},
                "baseRefName":"main","author":{author}}}"#,
            draft = number.is_multiple_of(2),
            sha = head.to_string().repeat(40),
            author = if number == 3 {
                "null"
            } else {
                r#"{"login":"someone"}"#
            },
        )
    }

    fn branches(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("topic-{i}")).collect()
    }

    fn origins_own(
        api: &dyn Api,
        origin: &GithubRepo,
        branches: &[String],
    ) -> Result<Found, ForgeError> {
        open_pull_requests(api, origin, branches, HeadsFrom::Origin)
    }

    #[test]
    fn asks_for_each_branch_on_origin_and_its_parent() {
        let body = format!(
            r#"{{"data":{{"repository":{{"nameWithOwner":"Me/Fork",
                "b0":{{"nodes":[{}, {}]}},
                "b1":{{"nodes":[]}},
                "parent":{{"nameWithOwner":"up/stream",
                    "b0":{{"nodes":[{}, {}]}},
                    "b1":{{"nodes":[{}]}}}}}}}}}}"#,
            pr(7, Some("me/fork"), 'a'),
            // Someone else's fork of the same repository, with a branch of the same name.
            pr(8, Some("other/fork"), 'b'),
            pr(40, Some("other/fork"), 'c'),
            pr(39, Some("me/fork"), 'd'),
            pr(3, None, 'e'),
        );
        let api = fake(&[&body], None);
        let origin = repo("me", "fork").unwrap();
        let found = origins_own(&api, &origin, &["topic".into(), "a\"b".into()]).unwrap();
        assert_eq!(found.name, "Me/Fork");
        let got: Vec<(u64, &str, &str)> = found
            .list
            .iter()
            .map(|p| (p.number, p.base_repo.as_str(), p.url.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (7, "Me/Fork", "https://github.com/Me/Fork/pull/7"),
                (39, "up/stream", "https://github.com/up/stream/pull/39"),
            ]
        );
        assert_eq!(found.list[0].head.to_hex(), "a".repeat(40));
        assert_eq!(found.pause_until, None);
        let asked = &requests(&api)[0];
        assert_eq!(asked["variables"]["owner"], "me");
        assert_eq!(asked["variables"]["b1"], "a\"b");
        let query = asked["query"].as_str().unwrap();
        assert!(query.contains("b1:pullRequests(states:OPEN,headRefName:$b1,first:100)"));
        assert!(!query.contains("a\""));
    }

    #[test]
    fn asked_before_deleting_a_branch_it_keeps_every_repositorys_pull_requests() {
        let body = format!(
            r#"{{"data":{{"repository":{{"nameWithOwner":"o/r",
                "b0":{{"nodes":[{}, {}]}},"parent":null}}}}}}"#,
            pr(7, Some("o/r"), 'a'),
            pr(8, Some("someone/fork"), 'b'),
        );
        let api = fake(&[&body], None);
        let origin = repo("o", "r").unwrap();
        let found =
            open_pull_requests(&api, &origin, &["topic".into()], HeadsFrom::AnyRepository).unwrap();
        let heads: Vec<_> = found.list.iter().map(|p| p.head_repo.clone()).collect();
        assert_eq!(heads, [Some("someone/fork".into()), Some("o/r".into())]);
    }

    #[test]
    fn asks_about_a_hundred_branches_at_a_time() {
        let empty = r#"{"data":{"repository":{"nameWithOwner":"o/r","parent":null}}}"#;
        let api = fake(&[empty, empty, empty], None);
        origins_own(&api, &repo("o", "r").unwrap(), &branches(250)).unwrap();
        let sizes: Vec<usize> = requests(&api)
            .iter()
            .map(|a| a["variables"].as_object().unwrap().len() - 2)
            .collect();
        assert_eq!(sizes, [100, 100, 50]);
    }

    #[test]
    fn stops_before_the_last_tenth_of_the_budget() {
        let empty = r#"{"data":{"repository":{"nameWithOwner":"o/r","parent":null}}}"#;
        let quota = |remaining| Quota {
            remaining,
            limit: 5000,
            reset: 1_900_000_000,
        };
        let found = origins_own(
            &fake(&[empty], Some(quota(500))),
            &repo("o", "r").unwrap(),
            &branches(1),
        )
        .unwrap();
        assert_eq!(found.pause_until, None);
        let found = origins_own(
            &fake(&[empty], Some(quota(499))),
            &repo("o", "r").unwrap(),
            &branches(1),
        )
        .unwrap();
        assert_eq!(found.pause_until, Some(1_900_000_000));
    }

    #[test]
    fn errors_are_passed_on() {
        let origin = repo("me", "gone").unwrap();
        let missing = r#"{"data":{"repository":null},"errors":[{"type":"NOT_FOUND",
            "message":"Could not resolve to a Repository with the name 'me/gone'."}]}"#;
        assert!(matches!(
            origins_own(&fake(&[missing], None), &origin, &branches(1)),
            Err(ForgeError::NotFound { .. })
        ));
        let odd = origins_own(&fake(&["<html>"], None), &origin, &branches(1));
        assert!(matches!(odd, Err(ForgeError::Parse(..))));
        let other = r#"{"errors":[{"type":"FORBIDDEN","message":"SAML enforcement"}]}"#;
        match origins_own(&fake(&[other], None), &origin, &branches(1)) {
            Err(e) => assert!(e.to_string().contains("SAML enforcement"), "{e}"),
            Ok(_) => panic!("an error was expected"),
        }
    }

    fn merged_pr(number: u64, branch: &str, head_repo: &str, head: char, merged: char) -> String {
        format!(
            r#"{{"number":{number},"headRefOid":"{sha}","headRefName":"{branch}",
                "headRepository":{{"nameWithOwner":"{head_repo}"}},
                "mergeCommit":{{"oid":"{merge}"}}}}"#,
            sha = head.to_string().repeat(40),
            merge = merged.to_string().repeat(40),
        )
    }

    fn gone(name: &str, tip: char) -> GoneBranch {
        GoneBranch {
            name: name.into(),
            tip: Oid::from_hex(&tip.to_string().repeat(40)).unwrap(),
        }
    }

    /// #384: only a merged pull request whose head was the branch's tip, from origin's own
    /// repository, into it or into its parent.
    #[test]
    fn merged_pull_requests_are_found_by_branch_and_tip() {
        let body = format!(
            r#"{{"data":{{"repository":{{"nameWithOwner":"Me/Fork",
                "b0":{{"nodes":[{}, {}]}},
                "b1":{{"nodes":[{}]}},
                "parent":{{"nameWithOwner":"up/stream",
                    "b0":{{"nodes":[]}},
                    "b1":{{"nodes":[{}]}}}}}}}}}}"#,
            // The name used again: a newer pull request from another tip first.
            merged_pr(9, "fix", "me/fork", 'e', '9'),
            merged_pr(4, "fix", "me/fork", 'a', '4'),
            // Another fork's branch of the same name and tip.
            merged_pr(8, "topic", "other/fork", 'b', '8'),
            merged_pr(31, "topic", "me/fork", 'b', '3'),
        );
        let api = fake(&[&body], None);
        let origin = repo("me", "fork").unwrap();
        let found = merged_pull_requests(&api, &origin, &["fix".into(), "topic".into()]).unwrap();
        let branches = [gone("fix", 'a'), gone("topic", 'b'), gone("fix", 'c')];
        let landed = |c: char| Some(Oid::from_hex(&c.to_string().repeat(40)).unwrap());
        assert_eq!(
            merged_of(&found.list, &branches),
            [
                Some(Merged {
                    how: None,
                    landed: landed('4'),
                    pull_request: Some(4),
                }),
                Some(Merged {
                    how: None,
                    landed: landed('3'),
                    pull_request: Some(31),
                }),
                None,
            ]
        );
        let query = requests(&api)[0]["query"].as_str().unwrap().to_owned();
        assert!(
            query.contains(
                "b1:pullRequests(states:MERGED,headRefName:$b1,first:10,\
                 orderBy:{field:CREATED_AT,direction:DESC})"
            ),
            "{query}"
        );
        assert!(query.contains("mergeCommit{oid}"), "{query}");
    }
}
