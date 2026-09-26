//! Open pull requests from GitHub (github.com only; no GitHub Enterprise yet), asked for the
//! way t3code does: per branch, signed in, cached, and within a budget.
//!
//! - **Which:** the pull requests the user can act on: those of the repository `origin` points
//!   at and, if that is a fork, the fork's own ones into its parent (research §12). Only those
//!   whose head branch is a branch of `origin` fetched here are asked for, since only they can
//!   be shown; asking the parent for all of its open pull requests took 23 s for
//!   pingdotgg/t3code.
//! - **How:** one GraphQL request per 100 such branches, with a `pullRequests(headRefName:)`
//!   connection per branch, for `origin` and its parent at once, and only the fields shown.
//!   As t3code's `gh pr list --head <branch>`, the branch name is matched on GitHub and the
//!   head repository here: another fork's `main` is not ours.
//! - **Signed in only:** with the token of a signed-in `gh` (`gh auth token`), or not at all.
//!   GitHub is never asked without one. `GH_TOKEN` and git's credential helpers (research §6)
//!   can come later.
//! - **Within a budget:** once fewer than a tenth of the hour's points are left, as t3code
//!   keeps in reserve, or GitHub says to wait, nothing is asked until the limit resets
//!   ([`pause`]). How often a repository is asked again is up to the caller:
//!   [`super::fresh_for`] and [`super::retry_after`].

use std::fmt;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{ForgeError, PullRequest, PullRequests, Remote};
use crate::git::Git;

/// GitHub's GraphQL endpoint. Tokens are only ever sent here.
#[cfg_attr(not(feature = "github"), allow(dead_code))]
const GRAPHQL: &str = "https://api.github.com/graphql";
const WEB: &str = "https://github.com/";
/// Branches asked about in one request.
const BRANCHES_PER_REQUEST: usize = 100;
/// Open pull requests asked for per branch. As t3code: GitHub prices a connection of 100 like
/// one of 1, and a name such as `main` can have many other forks' pull requests.
const PER_BRANCH: usize = 100;

/// A repository on github.com.
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

    /// `owner/name`, from a full name the API gave; `None` if it doesn't look like one.
    fn from_full_name(full_name: &str) -> Option<GithubRepo> {
        GithubRepo::from_url(&format!("{WEB}{full_name}"))
    }

    /// `owner/name`.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// The GitHub repository `origin` points at, if any. Asks git only, not GitHub.
pub fn origin(git: &Git) -> Option<GithubRepo> {
    let url = git.query(&["remote", "get-url", "origin"]).ok()??;
    GithubRepo::from_url(&url)
}

/// Loads the open pull requests for the repository `git` works on (see the module docs): no
/// request if no branch of `origin` has been fetched, otherwise one per 100 of them, signed in
/// with `gh`'s token. Run it on a worker thread.
pub fn load(git: &Git) -> Result<PullRequests, ForgeError> {
    let urls = super::remote_urls(git)?;
    let origin = urls
        .iter()
        .find(|(name, _)| name == "origin")
        .and_then(|(_, url)| GithubRepo::from_url(url))
        .ok_or(ForgeError::NoForge)?;
    let upstreams = super::upstreams(git)?;
    let branches = remote_branches(git, "origin")?;
    let found = if branches.is_empty() {
        Found {
            name: origin.full_name(),
            list: Vec::new(),
            pause_until: None,
        }
    } else {
        with_api(|api| open_pull_requests(api, &origin, &branches))?
    };
    if let Some(until) = found.pause_until {
        pause(until);
    }
    let remotes = urls
        .iter()
        .filter_map(|(name, url)| {
            let repo = GithubRepo::from_url(url)?;
            // A renamed repository answers under its new name; remotes may still use the old.
            let repo = if repo.full_name().eq_ignore_ascii_case(&origin.full_name()) {
                found.name.clone()
            } else {
                repo.full_name()
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

/// The branches of the remote `remote` (`refs/remotes/<remote>/*`, without `HEAD`).
fn remote_branches(git: &Git, remote: &str) -> Result<Vec<String>, crate::git::GitError> {
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

#[cfg_attr(not(feature = "github"), allow(dead_code))]
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Asks GitHub nothing until `until` (Unix time).
fn pause(until: u64) {
    let mut paused = PAUSED_UNTIL.lock().unwrap_or_else(|e| e.into_inner());
    *paused = (*paused).max(until);
}

/// How long GitHub is not to be asked yet, if at all.
#[cfg_attr(not(feature = "github"), allow(dead_code))]
fn paused_for() -> Option<Duration> {
    let until = *PAUSED_UNTIL.lock().unwrap_or_else(|e| e.into_inner());
    let now = now();
    (until > now).then(|| Duration::from_secs(until - now))
}

/// What GitHub's rate-limit headers said: points left of the hour's, and when they reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Quota {
    pub remaining: u64,
    pub limit: u64,
    /// Unix time.
    pub reset: u64,
}

impl Quota {
    /// Until when to ask nothing more: the reset, once less than a tenth is left.
    fn pause_until(self) -> Option<u64> {
        (self.remaining * 10 < self.limit).then_some(self.reset)
    }
}

/// An answer's body, and the rate limit it reported.
#[derive(Debug)]
pub(crate) struct Answer {
    pub body: String,
    pub quota: Option<Quota>,
}

/// GraphQL requests; a trait so that tests can answer them.
pub(crate) trait Api {
    fn post(&self, body: &str) -> Result<Answer, ForgeError>;
}

/// What [`open_pull_requests`] found.
#[derive(Debug)]
pub(crate) struct Found {
    /// `origin`'s `owner/name` as GitHub has it now.
    pub name: String,
    /// Newest first.
    pub list: Vec<PullRequest>,
    /// Ask nothing more until then (Unix time): the budget is nearly used up.
    pub pause_until: Option<u64>,
}

/// The open pull requests whose head is one of `branches` of `origin`: into `origin`, and into
/// its parent if it is a fork.
pub(crate) fn open_pull_requests(
    api: &dyn Api,
    origin: &GithubRepo,
    branches: &[String],
) -> Result<Found, ForgeError> {
    let mut found = Found {
        name: origin.full_name(),
        list: Vec::new(),
        pause_until: None,
    };
    for chunk in branches.chunks(BRANCHES_PER_REQUEST) {
        let answer = api.post(&request(origin, chunk))?;
        found.pause_until = found
            .pause_until
            .max(answer.quota.and_then(Quota::pause_until));
        let repo = repository(&answer.body, origin)?;
        found.name = repo.name_with_owner.clone();
        let ours = |pr: &json::PullRequest| {
            pr.head_repository.as_ref().is_some_and(|r| {
                r.name_with_owner
                    .eq_ignore_ascii_case(&repo.name_with_owner)
            })
        };
        let into =
            |base: &str, connections: &std::collections::HashMap<String, json::Connection>| {
                let base = GithubRepo::from_full_name(base)?;
                Some(
                    connections
                        .values()
                        .flat_map(|c| &c.nodes)
                        .filter(|pr| ours(pr))
                        .filter_map(|pr| pr.to_pull_request(&base))
                        .collect::<Vec<_>>(),
                )
            };
        found
            .list
            .extend(into(&repo.name_with_owner, &repo.connections).unwrap_or_default());
        if let Some(parent) = &repo.parent {
            found
                .list
                .extend(into(&parent.name_with_owner, &parent.connections).unwrap_or_default());
        }
    }
    found.list.sort_by(|a, b| {
        (&a.base_repo, std::cmp::Reverse(a.number))
            .cmp(&(&b.base_repo, std::cmp::Reverse(b.number)))
    });
    found
        .list
        .dedup_by(|a, b| a.base_repo == b.base_repo && a.number == b.number);
    Ok(found)
}

/// The GraphQL request, as JSON, for the pull requests of `branches`: a connection `b<i>` per
/// branch, on `origin` and on its parent. Branch names go in as variables, never into the
/// query text.
fn request(origin: &GithubRepo, branches: &[String]) -> String {
    let connections: String = (0..branches.len())
        .map(|i| {
            format!(
                "b{i}:pullRequests(states:OPEN,headRefName:$b{i},first:{PER_BRANCH}){{nodes{{...pr}}}}"
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

/// The repository in an answer, or why there is none.
fn repository(body: &str, origin: &GithubRepo) -> Result<json::Repository, ForgeError> {
    let answer: json::Answer =
        serde_json::from_str(body).map_err(|e| ForgeError::Parse(e.to_string()))?;
    if let Some(repo) = answer.data.and_then(|d| d.repository) {
        return Ok(repo);
    }
    let error = answer.errors.into_iter().next();
    Err(match error {
        Some(e) if e.kind.as_deref() == Some("NOT_FOUND") => ForgeError::NotFound {
            repo: origin.full_name(),
        },
        Some(e) if e.kind.as_deref() == Some("RATE_LIMITED") => {
            ForgeError::RateLimited { minutes: 60 }
        }
        Some(e) => ForgeError::Status {
            status: 200,
            message: e.message,
        },
        None => ForgeError::Parse("no repository in the answer".into()),
    })
}

/// The API's JSON, as far as it is read.
mod json {
    use std::collections::HashMap;

    use serde::Deserialize;

    use super::{GithubRepo, WEB};
    use crate::oid::Oid;

    #[derive(Debug, Deserialize)]
    pub struct Answer {
        pub data: Option<Data>,
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
        /// The pull request, into `base`, if its head is a commit id. Its page is built here,
        /// not taken from the answer, so that only github.com is ever opened.
        pub fn to_pull_request(&self, base: &GithubRepo) -> Option<crate::forge::PullRequest> {
            Some(crate::forge::PullRequest {
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

/// A token for the API. Never printed: its `Debug` shows stars.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Token(String);

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(***)")
    }
}

/// The token of a signed-in `gh` (`gh auth token`): [`ForgeError::NoGh`] if `gh` isn't
/// installed, [`ForgeError::NotSignedIn`] if it isn't signed in to github.com or doesn't answer
/// within a few seconds.
#[cfg_attr(not(feature = "github"), allow(dead_code))]
fn gh_token() -> Result<Token, ForgeError> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let mut cmd = Command::new("gh");
    cmd.args(["auth", "token", "--hostname", "github.com"])
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        // As for git: no console window flashing up.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ForgeError::NoGh),
        Err(e) => return Err(ForgeError::Network(format!("could not run gh: {e}"))),
    };
    let deadline = Instant::now() + Duration::from_secs(5);
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

/// The API over HTTPS, with ureq.
#[cfg(feature = "github")]
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
impl fmt::Debug for Http {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Http")
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "github")]
impl Api for Http {
    fn post(&self, body: &str) -> Result<Answer, ForgeError> {
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
            .limit(64 * 1024 * 1024)
            .read_to_string()
            .map_err(|e| ForgeError::Network(e.to_string()))?;
        match status {
            200..=299 => Ok(Answer { body, quota }),
            401 => Err(ForgeError::TokenRejected),
            403 | 429 if retry_after.is_some() || quota.is_some_and(|q| q.remaining == 0) => {
                let until = match (retry_after, quota) {
                    (Some(secs), _) => now() + secs,
                    (None, Some(q)) => q.reset,
                    (None, None) => now() + 60,
                };
                pause(until);
                Err(ForgeError::RateLimited {
                    minutes: until.saturating_sub(now()).div_ceil(60),
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

    #[test]
    fn tokens_are_not_printed() {
        let token = Token("gho_secret".into());
        assert_eq!(format!("{token:?}"), "Token(***)");
        assert!(!format!("{:?}", Some(token)).contains("secret"));
    }

    /// Answers every request with the next of `answers`, and keeps the requests.
    struct Fake {
        answers: std::cell::RefCell<Vec<Answer>>,
        asked: std::cell::RefCell<Vec<serde_json::Value>>,
    }

    impl Fake {
        fn new(bodies: &[&str], quota: Option<Quota>) -> Fake {
            Fake {
                answers: std::cell::RefCell::new(
                    bodies
                        .iter()
                        .rev()
                        .map(|b| Answer {
                            body: (*b).to_owned(),
                            quota,
                        })
                        .collect(),
                ),
                asked: std::cell::RefCell::new(Vec::new()),
            }
        }
    }

    impl Api for Fake {
        fn post(&self, body: &str) -> Result<Answer, ForgeError> {
            self.asked
                .borrow_mut()
                .push(serde_json::from_str(body).expect("requests are JSON"));
            self.answers
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
        let found = open_pull_requests(&api, &origin, &["topic".into(), "a\"b".into()]).unwrap();
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
        // Branch names are variables, never part of the query.
        let asked = &api.asked.borrow()[0];
        assert_eq!(asked["variables"]["owner"], "me");
        assert_eq!(asked["variables"]["b1"], "a\"b");
        let query = asked["query"].as_str().unwrap();
        assert!(query.contains("b1:pullRequests(states:OPEN,headRefName:$b1,first:100)"));
        assert!(!query.contains("a\""));
    }

    #[test]
    fn asks_about_a_hundred_branches_at_a_time() {
        let empty = r#"{"data":{"repository":{"nameWithOwner":"o/r","parent":null}}}"#;
        let api = Fake::new(&[empty, empty, empty], None);
        open_pull_requests(&api, &repo("o", "r").unwrap(), &branches(250)).unwrap();
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
        let found = open_pull_requests(
            &Fake::new(&[empty], Some(quota(500))),
            &repo("o", "r").unwrap(),
            &branches(1),
        )
        .unwrap();
        assert_eq!(found.pause_until, None);
        let found = open_pull_requests(
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
            open_pull_requests(&Fake::new(&[missing], None), &origin, &branches(1)),
            Err(ForgeError::NotFound { .. })
        ));
        let odd = open_pull_requests(&Fake::new(&["<html>"], None), &origin, &branches(1));
        assert!(matches!(odd, Err(ForgeError::Parse(_))));
        let other = r#"{"errors":[{"type":"FORBIDDEN","message":"SAML enforcement"}]}"#;
        match open_pull_requests(&Fake::new(&[other], None), &origin, &branches(1)) {
            Err(e) => assert!(e.to_string().contains("SAML enforcement"), "{e}"),
            Ok(_) => panic!("an error was expected"),
        }
    }
}
