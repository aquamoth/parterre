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
//!   in reserve, or GitHub says to wait, nothing is asked until the limit resets ([`pause`]).

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use super::{ForgeError, PullRequest, PullRequests, Remote};
use parterre_core::git::{Git, GitError};
use parterre_core::oid::Oid;

mod repositories;
pub use repositories::{Listed, Protocol, Repositories, repositories, repositories_canned};

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
        let url = url.trim();
        let (host, path) = match url.split_once("://") {
            Some((scheme, rest)) => {
                let scheme = scheme.to_ascii_lowercase();
                let scheme = scheme.strip_prefix("git+").unwrap_or(&scheme);
                if !matches!(scheme, "https" | "http" | "ssh" | "git") {
                    return None;
                }
                let (authority, path) = rest.split_once('/')?;
                let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
                let host = host.split_once(':').map_or(host, |(h, _)| h);
                (host, path)
            }
            // scp-like: [user@]host:path, with no slash before the colon.
            None => {
                let (authority, path) = url.split_once(':')?;
                if authority.contains('/') {
                    return None;
                }
                let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
                (host, path)
            }
        };
        let host = host.to_ascii_lowercase();
        if !matches!(
            host.as_str(),
            "github.com" | "www.github.com" | "ssh.github.com"
        ) {
            return None;
        }
        let path = path.trim_matches('/');
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

/// Whether an open pull request proposes a branch about to be deleted on a remote (#318).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Proposed {
    NotOnGithub,
    No,
    Open(PullRequest),
    /// GitHub couldn't be asked: why.
    Unknown(String),
}

/// For each remote branch `(remote, branch)`: whether an open pull request into `origin`'s
/// repository or its parent proposes it, from the repository its remote points at. Asks GitHub
/// once, whether pull requests are shown or not; `canned`, `--pull-requests-from`'s list, is
/// used instead if given. Blocks: run it on a worker thread.
pub fn proposals(git: &Git, branches: &[(String, String)], canned: Option<&str>) -> Vec<Proposed> {
    let urls = super::remote_urls(git).unwrap_or_default();
    let repos: Vec<Option<GithubRepo>> = branches
        .iter()
        .map(|(remote, _)| {
            let (_, url) = urls.iter().find(|(name, _)| name == remote)?;
            GithubRepo::from_url(url)
        })
        .collect();
    if repos.iter().all(Option::is_none) {
        return vec![Proposed::NotOnGithub; branches.len()];
    }
    let found = match canned {
        Some(json) => load_canned(git, json).map(|p| p.list),
        None => {
            let mut names: Vec<String> = branches.iter().map(|(_, b)| b.clone()).collect();
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
        .zip(repos)
        .map(|((_, branch), repo)| {
            let Some(repo) = repo else {
                return Proposed::NotOnGithub;
            };
            let list = match &found {
                Ok(list) => list,
                Err(e) => return Proposed::Unknown(e.to_string()),
            };
            let repo = repo.full_name();
            let proposing = list.iter().find(|pr| {
                pr.head_branch == *branch
                    && pr
                        .head_repo
                        .as_deref()
                        .is_some_and(|r| r.eq_ignore_ascii_case(&repo))
            });
            proposing.map_or(Proposed::No, |pr| Proposed::Open(pr.clone()))
        })
        .collect()
}

/// Pull requests from `json` instead of GitHub, for screenshots and demos without the network
/// (`--pull-requests-from`): an array of `{"number", "title", "author", "draft", "head",
/// "base"}`, where `head` and `base` are branches of `origin`, which must still point at
/// GitHub. Each one is on the commit `origin/<head>` is at.
pub fn load_canned(git: &Git, json: &str) -> Result<PullRequests, ForgeError> {
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
    pull_requests(git, |origin| {
        let name = origin.full_name();
        let list = canned
            .into_iter()
            .map(|pr| {
                let tip = format!("refs/remotes/origin/{}^{{commit}}", pr.head);
                let head = git
                    .query(&["rev-parse", "--verify", "--quiet", &tip])?
                    .and_then(|hex| Oid::from_hex(&hex))
                    .ok_or_else(|| {
                        ForgeError::Canned(format!("origin has no branch {}", pr.head))
                    })?;
                Ok(PullRequest {
                    url: format!("{WEB}{name}/pull/{}", pr.number),
                    number: pr.number,
                    title: pr.title,
                    author: pr.author,
                    draft: pr.draft,
                    head,
                    head_branch: pr.head,
                    head_repo: Some(name.clone()),
                    base_branch: pr.base,
                    base_repo: name.clone(),
                })
            })
            .collect::<Result<_, ForgeError>>()?;
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
pub(crate) struct Found {
    /// `origin`'s `owner/name` as GitHub has it now.
    pub name: String,
    pub list: Vec<PullRequest>,
    /// Unix time.
    pub pause_until: Option<u64>,
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
        let response = api.post(&request(origin, chunk))?;
        found.pause_until = found
            .pause_until
            .max(response.quota.and_then(Quota::pause_until));
        let repo = repository(&response.body, origin)?;
        found.name = repo.name_with_owner.clone();
        let is_kept = |pr: &json::PullRequest| match heads_from {
            HeadsFrom::AnyRepository => true,
            HeadsFrom::Origin => pr.head_repository.as_ref().is_some_and(|r| {
                r.name_with_owner
                    .eq_ignore_ascii_case(&repo.name_with_owner)
            }),
        };
        let mut add = |base: &str, connections: &HashMap<String, json::Connection>| {
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

/// A connection `b<i>` per branch, on `origin` and on its parent. Branch names go in as
/// variables, never into the query text.
fn request(origin: &GithubRepo, branches: &[String]) -> String {
    let connections: String = (0..branches.len())
        .map(|i| {
            format!(
                "b{i}:pullRequests(states:OPEN,headRefName:$b{i},\
                 first:{PULL_REQUESTS_PER_BRANCH}){{nodes{{...pr}}}}"
            )
        })
        .collect();
    let params: String = (0..branches.len())
        .map(|i| format!(",$b{i}:String!"))
        .collect();
    let query = format!(
        "query($owner:String!,$name:String!{params}){{repository(owner:$owner,name:$name){{\
         nameWithOwner parent{{nameWithOwner {connections}}}{connections}}}}}\
         fragment pr on PullRequest{{number title isDraft headRefOid headRefName \
         headRepository{{nameWithOwner}} baseRefName author{{login}}}}"
    );
    let mut variables = serde_json::Map::new();
    variables.insert("owner".into(), origin.owner.clone().into());
    variables.insert("name".into(), origin.name.clone().into());
    for (i, branch) in branches.iter().enumerate() {
        variables.insert(format!("b{i}"), branch.clone().into());
    }
    serde_json::json!({ "query": query, "variables": variables }).to_string()
}

fn repository(body: &str, origin: &GithubRepo) -> Result<json::Repository, ForgeError> {
    let answer: json::Answer<json::Data> = json::parse(body)?;
    if let Some(repo) = answer.data.and_then(|d| d.repository) {
        return Ok(repo);
    }
    Err(match answer.errors.into_iter().next() {
        Some(e) if e.kind.as_deref() == Some("NOT_FOUND") => ForgeError::NotFound {
            repo: origin.full_name(),
        },
        Some(e) => e.into_forge_error(),
        None => ForgeError::Parse("no repository in the answer".into()),
    })
}

mod json {
    use std::collections::HashMap;

    use serde::Deserialize;
    use serde::de::DeserializeOwned;

    use super::{ForgeError, GithubRepo, WEB};
    use parterre_core::oid::Oid;

    pub fn parse<T: DeserializeOwned>(body: &str) -> Result<T, ForgeError> {
        serde_json::from_str(body).map_err(|e| ForgeError::Parse(e.to_string()))
    }

    #[derive(Debug, Deserialize)]
    pub struct Answer<D> {
        pub data: Option<D>,
        #[serde(default)]
        pub errors: Vec<Error>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Data {
        pub repository: Option<Repository>,
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
                    status: 200,
                    message: self.message,
                }
            }
        }
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Repository {
        pub name_with_owner: String,
        #[serde(default)]
        pub parent: Option<Parent>,
        /// `b0`, `b1`, …: the pull requests of each branch asked about.
        #[serde(flatten)]
        pub connections: HashMap<String, Connection>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Parent {
        pub name_with_owner: String,
        #[serde(flatten)]
        pub connections: HashMap<String, Connection>,
    }

    #[derive(Debug, Deserialize)]
    pub struct Connection {
        pub nodes: Vec<PullRequest>,
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
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let mut cmd = Command::new("gh");
    parterre_core::shell_path::apply(&mut cmd)
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        // A GUI-subsystem app on Windows; without this a console window would flash up.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ForgeError::NoGh),
        Err(e) => return Err(ForgeError::Network(format!("could not run gh: {e}"))),
    };
    let deadline = Instant::now() + GH_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ForgeError::NotSignedIn);
            }
        }
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut stdout, &mut out);
    }
    Ok(out)
}

/// Never printed: its `Debug` shows stars.
#[cfg(feature = "github")]
#[derive(Clone, PartialEq, Eq)]
struct Token(String);

#[cfg(feature = "github")]
impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(***)")
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
    Err(ForgeError::Unsupported)
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
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

#[cfg(feature = "github")]
#[derive(Debug)]
struct Http {
    agent: ureq::Agent,
    token: Token,
}

#[cfg(feature = "github")]
impl Http {
    fn new(token: Token) -> Http {
        use ureq::tls::{RootCerts, TlsConfig};
        // The system's certificate authorities, including any a company adds.
        let tls = TlsConfig::builder()
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        let agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(Duration::from_secs(30)))
            // Error answers carry the reason, and the rate limit in their headers.
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent(concat!("parterre/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Http { agent, token }
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
            .map_err(|e| ForgeError::Network(e.to_string()))?;
        let header = |name: &str| -> Option<u64> {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse().ok())
        };
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
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_string()
            .map_err(|e| ForgeError::Network(e.to_string()))?;
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
            _ => {
                #[derive(serde::Deserialize)]
                struct Message {
                    message: String,
                }
                let message = serde_json::from_str::<Message>(&body)
                    .map(|m| m.message)
                    .unwrap_or_else(|_| body.chars().take(200).collect());
                Err(ForgeError::Status { status, message })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

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

    #[cfg(feature = "github")]
    #[test]
    fn tokens_are_not_printed() {
        let token = Token("gho_secret".into());
        assert_eq!(format!("{token:?}"), "Token(***)");
        assert!(!format!("{:?}", Some(token)).contains("secret"));
    }

    /// Answers each request with the next of `bodies`, and keeps the requests.
    pub(super) struct Fake {
        responses: RefCell<Vec<Response>>,
        pub asked: RefCell<Vec<serde_json::Value>>,
    }

    impl Fake {
        pub fn new(bodies: &[&str], quota: Option<Quota>) -> Fake {
            Fake {
                responses: RefCell::new(
                    bodies
                        .iter()
                        .rev()
                        .map(|b| Response {
                            body: (*b).to_owned(),
                            quota,
                        })
                        .collect(),
                ),
                asked: RefCell::new(Vec::new()),
            }
        }
    }

    impl Api for Fake {
        fn post(&self, body: &str) -> Result<Response, ForgeError> {
            self.asked
                .borrow_mut()
                .push(serde_json::from_str(body).expect("requests are JSON"));
            self.responses
                .borrow_mut()
                .pop()
                .ok_or_else(|| ForgeError::Network("no more answers".into()))
        }
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
        let api = Fake::new(&[&body], None);
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
        let asked = &api.asked.borrow()[0];
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
        let api = Fake::new(&[&body], None);
        let origin = repo("o", "r").unwrap();
        let found =
            open_pull_requests(&api, &origin, &["topic".into()], HeadsFrom::AnyRepository).unwrap();
        let heads: Vec<_> = found.list.iter().map(|p| p.head_repo.clone()).collect();
        assert_eq!(heads, [Some("someone/fork".into()), Some("o/r".into())]);
    }

    #[test]
    fn asks_about_a_hundred_branches_at_a_time() {
        let empty = r#"{"data":{"repository":{"nameWithOwner":"o/r","parent":null}}}"#;
        let api = Fake::new(&[empty, empty, empty], None);
        origins_own(&api, &repo("o", "r").unwrap(), &branches(250)).unwrap();
        let sizes: Vec<usize> = api
            .asked
            .borrow()
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
            &Fake::new(&[empty], Some(quota(500))),
            &repo("o", "r").unwrap(),
            &branches(1),
        )
        .unwrap();
        assert_eq!(found.pause_until, None);
        let found = origins_own(
            &Fake::new(&[empty], Some(quota(499))),
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
            origins_own(&Fake::new(&[missing], None), &origin, &branches(1)),
            Err(ForgeError::NotFound { .. })
        ));
        let odd = origins_own(&Fake::new(&["<html>"], None), &origin, &branches(1));
        assert!(matches!(odd, Err(ForgeError::Parse(_))));
        let other = r#"{"errors":[{"type":"FORBIDDEN","message":"SAML enforcement"}]}"#;
        match origins_own(&Fake::new(&[other], None), &origin, &branches(1)) {
            Err(e) => assert!(e.to_string().contains("SAML enforcement"), "{e}"),
            Ok(_) => panic!("an error was expected"),
        }
    }
}
