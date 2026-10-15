//! Active pull requests from Azure DevOps (#102), as GitHub's open ones are shown: those of the
//! repository `origin` points at, on the commits they propose, where those have been fetched.
//!
//! - **How:** one REST request lists the repository's active pull requests (500 a page).
//!   Azure DevOps has no GraphQL, and its repositories tend to have few active pull requests
//!   but many branches, so this is cheaper than asking per branch as on GitHub.
//! - **Signed in as git is:** with the sign-in git's credential helper has for `origin`'s
//!   HTTPS URL (`git credential fill`), Git Credential Manager's on most machines, never
//!   asking for one. Not `az`: its account can be another than git's (it was, where this was
//!   tried), and it would be a second program to rely on. Research:
//!   `docs/research/github-forks-and-pull-requests.md` §15.
//! - **Forks:** pull requests from forks are left out; forks are rare on Azure DevOps.

#[cfg(feature = "azure-devops")]
use std::fmt;
use std::process::Command;
use std::time::Duration;

use super::{Forge, ForgeError, PullRequest, PullRequests, Remote, Token};
use parterre_core::git::Git;
use parterre_core::oid::Oid;

const WEB: &str = "https://dev.azure.com/";

/// Loads the active pull requests of the repository `origin` points at (see the module docs):
/// one request per 500, signed in as git is. Run it on a worker thread.
pub fn load(git: &Git) -> Result<PullRequests, ForgeError> {
    pull_requests(git, |url, origin| {
        with_api(git, url, origin, |api| active_pull_requests(api, origin))
    })
}

/// Pull requests from `json` instead of Azure DevOps (`--pull-requests-from`), as
/// [`super::github::load_canned`] reads them.
pub fn load_canned(git: &Git, json: &str) -> Result<PullRequests, ForgeError> {
    pull_requests(git, |_, origin| {
        super::canned(git, json, &origin.full_name(), |number| {
            format!("{}/pullrequest/{number}", origin.web_url())
        })
    })
}

/// The pull requests `find` finds for `origin` (given its URL), with the remotes and upstreams
/// that place them.
fn pull_requests(
    git: &Git,
    find: impl FnOnce(&str, &AzureRepo) -> Result<Vec<PullRequest>, ForgeError>,
) -> Result<PullRequests, ForgeError> {
    let urls = super::remote_urls(git)?;
    let (url, origin) = urls
        .iter()
        .find(|(name, _)| name == "origin")
        .and_then(|(_, url)| Some((url, AzureRepo::from_url(url)?)))
        .ok_or(ForgeError::NoForge)?;
    let upstreams = super::upstreams(git)?;
    let list = find(url, &origin)?;
    let remotes = urls
        .iter()
        .filter_map(|(name, url)| {
            Some(Remote {
                name: name.clone(),
                repo: AzureRepo::from_url(url)?.full_name(),
            })
        })
        .collect();
    Ok(PullRequests {
        list,
        remotes,
        upstreams,
    })
}

/// Where to ask git for a sign-in to `repo`, whose remote URL is `url`, in order, each with the
/// root of the API that sign-in is sent to: the HTTPS URL git fetches from, then the same
/// repository on `dev.azure.com`, which Git Credential Manager may know instead of
/// `{org}.visualstudio.com` (it did where this was tried).
#[cfg_attr(not(feature = "azure-devops"), allow(dead_code))]
fn sign_in_urls(url: &str, repo: &AzureRepo) -> Vec<(String, String)> {
    let mut urls = Vec::new();
    let url = url.trim();
    if url
        .get(..8)
        .is_some_and(|s| s.eq_ignore_ascii_case("https://"))
    {
        let root = match repo.host {
            Host::DevAzure => format!("{WEB}{}", repo.org),
            Host::VisualStudio => format!("https://{}.visualstudio.com", repo.org),
        };
        urls.push((url.to_owned(), root));
    }
    let canonical = repo.web_url();
    if urls
        .iter()
        .all(|(u, _)| u.trim_end_matches('/') != canonical)
    {
        urls.push((canonical, format!("{WEB}{}", repo.org)));
    }
    urls
}

/// A sign-in git gave. Never printed: its `Debug` shows stars.
#[cfg_attr(not(feature = "azure-devops"), allow(dead_code))]
#[derive(Debug)]
pub(crate) struct Credential {
    password: Token,
}

#[cfg_attr(not(feature = "azure-devops"), allow(dead_code))]
impl Credential {
    /// The `Authorization` header: a Microsoft Entra token (a JWT) as a bearer token, anything
    /// else, such as a personal access token, as the password of Basic authentication.
    pub(crate) fn authorization(&self) -> String {
        let Token(password) = &self.password;
        if password.starts_with("eyJ") {
            format!("Bearer {password}")
        } else {
            format!("Basic {}", base64(format!(":{password}").as_bytes()))
        }
    }
}

/// The sign-in git's credential helpers have for `url`, as git would fetch with it, without
/// ever asking the user: [`ForgeError::NoGitSignIn`] if there is none, or it isn't given within
/// 20 s (Git Credential Manager may renew it over the network).
#[cfg_attr(not(feature = "azure-devops"), allow(dead_code))]
pub(crate) fn sign_in(git: &Git, url: &str) -> Result<Credential, ForgeError> {
    let mut cmd = Command::new(parterre_core::git::program());
    parterre_core::shell_path::apply(&mut cmd)
        .arg("-C")
        .arg(git.dir())
        .args(["-c", "credential.interactive=false", "credential", "fill"])
        // No prompt in a terminal, and no askpass program (an empty one stops git from falling
        // back to `core.askPass` and `SSH_ASKPASS`).
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("GCM_INTERACTIVE", "never");
    let no_sign_in = || ForgeError::NoGitSignIn {
        url: url.to_owned(),
    };
    let out = super::output_within(&mut cmd, &format!("url={url}\n\n"), Duration::from_secs(20))
        .map_err(|e| ForgeError::Git(parterre_core::git::GitError::Spawn(e)))?
        .ok_or_else(no_sign_in)?;
    let password = out
        .lines()
        .find_map(|line| line.strip_prefix("password="))
        .filter(|p| !p.is_empty())
        .ok_or_else(no_sign_in)?;
    Ok(Credential {
        password: Token(password.to_owned()),
    })
}

/// `bytes` in standard Base64, with padding.
#[cfg_attr(not(feature = "azure-devops"), allow(dead_code))]
fn base64(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(DIGITS[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Runs `calls` against the API, signed in with the first sign-in git has of
/// [`sign_in_urls`] that Azure DevOps takes; without one, Azure DevOps is not asked.
#[cfg(feature = "azure-devops")]
fn with_api<T>(
    git: &Git,
    url: &str,
    repo: &AzureRepo,
    calls: impl Fn(&dyn Api) -> Result<T, ForgeError>,
) -> Result<T, ForgeError> {
    let mut failed = None;
    for (url, root) in sign_in_urls(url, repo) {
        let answer = sign_in(git, &url).and_then(|credential| {
            calls(&Http {
                agent: super::agent(),
                root,
                authorization: Token(credential.authorization()),
                url,
                repo: repo.full_name(),
            })
        });
        match answer {
            Err(e @ (ForgeError::NoGitSignIn { .. } | ForgeError::GitSignInRejected { .. })) => {
                failed = Some(e);
            }
            answer => return answer,
        }
    }
    Err(failed.unwrap_or(ForgeError::NoForge))
}

#[cfg(not(feature = "azure-devops"))]
fn with_api<T>(
    _: &Git,
    _: &str,
    _: &AzureRepo,
    _: impl Fn(&dyn Api) -> Result<T, ForgeError>,
) -> Result<T, ForgeError> {
    Err(ForgeError::Unsupported(Forge::AzureDevops))
}

/// The API over HTTPS, with ureq: one organisation's, the only place its sign-in is sent.
#[cfg(feature = "azure-devops")]
struct Http {
    agent: ureq::Agent,
    /// `https://dev.azure.com/{org}` or `https://{org}.visualstudio.com`.
    root: String,
    authorization: Token,
    /// What the sign-in is git's for, and the repository asked about: for errors.
    url: String,
    repo: String,
}

#[cfg(feature = "azure-devops")]
impl fmt::Debug for Http {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Http")
            .field("root", &self.root)
            .field("authorization", &self.authorization)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "azure-devops")]
impl Api for Http {
    fn get(&self, path: &str) -> Result<String, ForgeError> {
        let Token(authorization) = &self.authorization;
        let network = |e: ureq::Error| ForgeError::Network(Forge::AzureDevops, e.to_string());
        let mut response = self
            .agent
            .get(&format!("{}{path}", self.root))
            .header("Authorization", authorization)
            .header("Accept", "application/json")
            // A 401, not a redirect to the sign-in page, for a sign-in it doesn't take.
            .header("X-TFS-FedAuthRedirect", "Suppress")
            .call()
            .map_err(network)?;
        let retry_after: Option<u64> = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse().ok());
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_to_string()
            .map_err(network)?;
        match status {
            200 => Ok(body),
            // 203 is the sign-in page, as are redirects; 403 a sign-in that may not read code.
            203 | 300..=399 | 401 | 403 => Err(ForgeError::GitSignInRejected {
                url: self.url.clone(),
            }),
            404 => Err(ForgeError::NotFound {
                forge: Forge::AzureDevops,
                repo: self.repo.clone(),
            }),
            // A minute if it doesn't say how long, or says it as a date.
            429 => Err(ForgeError::Throttled {
                minutes: retry_after.unwrap_or(60).div_ceil(60).max(1),
            }),
            503 if retry_after.is_some() => Err(ForgeError::Throttled {
                minutes: retry_after.unwrap_or(60).div_ceil(60).max(1),
            }),
            _ => {
                #[derive(serde::Deserialize)]
                struct Message {
                    message: String,
                }
                let message = serde_json::from_str::<Message>(&body)
                    .map(|m| m.message)
                    .unwrap_or_else(|_| body.chars().take(200).collect());
                Err(ForgeError::Status {
                    forge: Forge::AzureDevops,
                    status,
                    message,
                })
            }
        }
    }
}

/// Requests to an organisation's API; a trait so that tests can answer them.
pub(crate) trait Api {
    /// The body of a successful answer to `GET {organisation}{path}`.
    fn get(&self, path: &str) -> Result<String, ForgeError>;
}

/// Pull requests asked for per request.
const PER_PAGE: usize = 500;
/// Requests at most: 5,000 active pull requests are more than any repository has.
const PAGES: usize = 10;

/// The active pull requests of `repo` that can be placed, newest first: not from a fork (they
/// are rare on Azure DevOps, and GitHub's other forks' are left out too), with a head commit
/// (none while a merge is being worked out).
pub(crate) fn active_pull_requests(
    api: &dyn Api,
    repo: &AzureRepo,
) -> Result<Vec<PullRequest>, ForgeError> {
    let base = repo.full_name();
    let mut list = Vec::new();
    for page in 0..PAGES {
        let path = format!(
            "/{}/_apis/git/repositories/{}/pullrequests?searchCriteria.status=active\
             &$top={PER_PAGE}&$skip={}&api-version=7.1",
            encoded(&repo.project),
            encoded(&repo.name),
            page * PER_PAGE,
        );
        let answer: json::List<json::PullRequest> = serde_json::from_str(&api.get(&path)?)
            .map_err(|e| ForgeError::Parse(Forge::AzureDevops, e.to_string()))?;
        let full = answer.value.len() == PER_PAGE;
        list.extend(answer.value.into_iter().filter_map(|pr| {
            if pr.fork_source.is_some() {
                return None;
            }
            Some(PullRequest {
                number: pr.pull_request_id,
                title: pr.title,
                author: pr.created_by.map(|a| a.display_name).unwrap_or_default(),
                draft: pr.is_draft,
                head: Oid::from_hex(&pr.last_merge_source_commit?.commit_id)?,
                head_branch: branch(&pr.source_ref_name).to_owned(),
                head_repo: Some(base.clone()),
                base_branch: branch(&pr.target_ref_name).to_owned(),
                base_repo: base.clone(),
                url: format!("{}/pullrequest/{}", repo.web_url(), pr.pull_request_id),
            })
        }));
        if !full {
            break;
        }
    }
    list.sort_by_key(|pr| std::cmp::Reverse(pr.number));
    Ok(list)
}

/// `main` of `refs/heads/main`.
fn branch(full_name: &str) -> &str {
    full_name.strip_prefix("refs/heads/").unwrap_or(full_name)
}

/// `name` percent-encoded for a URL's path, all but letters, digits and `-._~`.
fn encoded(name: &str) -> String {
    name.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// The API's JSON, as far as it is read.
mod json {
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    pub struct List<T> {
        pub value: Vec<T>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PullRequest {
        pub pull_request_id: u64,
        pub title: String,
        #[serde(default)]
        pub is_draft: bool,
        pub created_by: Option<Identity>,
        pub source_ref_name: String,
        pub target_ref_name: String,
        pub last_merge_source_commit: Option<Commit>,
        pub fork_source: Option<serde::de::IgnoredAny>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Identity {
        pub display_name: String,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Commit {
        pub commit_id: String,
    }
}

/// Where an Azure DevOps organisation is served: `dev.azure.com/{org}`, or the older
/// `{org}.visualstudio.com`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    DevAzure,
    VisualStudio,
}

/// A repository on Azure DevOps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AzureRepo {
    pub org: String,
    /// Decoded: `My Project`, not `My%20Project`.
    pub project: String,
    pub name: String,
    pub host: Host,
}

impl AzureRepo {
    /// The repository a remote URL points at, if it is on Azure DevOps: the HTTPS forms
    /// `https://[{org}@]dev.azure.com/{org}/{project}/_git/{repo}` and
    /// `https://{org}.visualstudio.com/[DefaultCollection/]{project}/_git/{repo}` (with no
    /// `{project}/` for a repository named as its project), and the SSH forms
    /// `git@ssh.dev.azure.com:v3/{org}/{project}/{repo}` and
    /// `{org}@vs-ssh.visualstudio.com:v3/{org}/{project}/{repo}`.
    pub fn from_url(url: &str) -> Option<AzureRepo> {
        let url = url.trim();
        let (ssh, host, path) = match url.split_once("://") {
            Some((scheme, rest)) => {
                let ssh = match scheme.to_ascii_lowercase().as_str() {
                    "https" | "http" => false,
                    "ssh" => true,
                    _ => return None,
                };
                let (authority, path) = rest.split_once('/')?;
                let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
                let host = host.split_once(':').map_or(host, |(h, _)| h);
                (ssh, host, path)
            }
            // scp-like: [user@]host:path, with no slash before the colon.
            None => {
                let (authority, path) = url.split_once(':')?;
                if authority.contains('/') {
                    return None;
                }
                let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
                (true, host, path)
            }
        };
        let host = host.to_ascii_lowercase();
        let segments: Vec<&str> = path.strip_suffix('/').unwrap_or(path).split('/').collect();
        let (org, host, project, name) = match (ssh, host.as_str(), segments.as_slice()) {
            (false, "dev.azure.com", [org, rest @ ..]) => {
                let (project, name) = git_path(rest)?;
                (*org, Host::DevAzure, project, name)
            }
            (false, _, segments) => {
                let org = host.strip_suffix(".visualstudio.com")?;
                let rest = match segments {
                    [collection, rest @ ..]
                        if collection.eq_ignore_ascii_case("DefaultCollection") =>
                    {
                        rest
                    }
                    rest => rest,
                };
                let (project, name) = git_path(rest)?;
                (org, Host::VisualStudio, project, name)
            }
            (true, "ssh.dev.azure.com", ["v3", org, project, name]) => {
                (*org, Host::DevAzure, *project, *name)
            }
            (true, "vs-ssh.visualstudio.com", ["v3", org, project, name]) => {
                (*org, Host::VisualStudio, *project, *name)
            }
            _ => return None,
        };
        let org_ok = !org.is_empty() && org.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        let project = decoded_name(project)?;
        let name = decoded_name(name)?;
        org_ok.then(|| AzureRepo {
            org: org.to_owned(),
            project,
            name,
            host,
        })
    }

    /// `org/project/name`, as [`crate::Remote::repo`] and [`PullRequest::base_repo`] hold it.
    pub fn full_name(&self) -> String {
        format!("{}/{}/{}", self.org, self.project, self.name)
    }

    /// Its page, on `dev.azure.com` whichever host its remote names: every organisation is
    /// served there, and the browser opens nothing else of Azure DevOps.
    pub fn web_url(&self) -> String {
        format!(
            "{WEB}{}/{}/_git/{}",
            self.org,
            encoded(&self.project),
            encoded(&self.name)
        )
    }
}

/// `{project}/_git/{repo}`, or `_git/{repo}` for a repository named as its project.
fn git_path<'a>(segments: &[&'a str]) -> Option<(&'a str, &'a str)> {
    match segments {
        [project, "_git", name] => Some((project, name)),
        ["_git", name] => Some((name, name)),
        _ => None,
    }
}

/// A project or repository name from a URL, percent-decoded; `None` if it can't be one.
fn decoded_name(segment: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(segment.len());
    let mut rest = segment.as_bytes();
    while let [first, tail @ ..] = rest {
        if *first == b'%' {
            let hex = std::str::from_utf8(tail.get(..2)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            rest = &tail[2..];
        } else {
            bytes.push(*first);
            rest = tail;
        }
    }
    let name = String::from_utf8(bytes).ok()?;
    // What Azure DevOps doesn't take in project and repository names.
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| c.is_control() || "\\/:*?\"<>|;#${},+=[]%".contains(c));
    ok.then_some(name)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn repo(org: &str, project: &str, name: &str, host: Host) -> Option<AzureRepo> {
        Some(AzureRepo {
            org: org.into(),
            project: project.into(),
            name: name.into(),
            host,
        })
    }

    #[test]
    fn azure_remote_urls() {
        use Host::*;
        for (url, expected) in [
            (
                "https://dev.azure.com/cosmojpn/Apps/_git/Apps",
                repo("cosmojpn", "Apps", "Apps", DevAzure),
            ),
            (
                "https://cosmojpn@dev.azure.com/cosmojpn/Apps/_git/CNet6",
                repo("cosmojpn", "Apps", "CNet6", DevAzure),
            ),
            (
                "https://dev.azure.com/o/CJP%20Work/_git/some.repo",
                repo("o", "CJP Work", "some.repo", DevAzure),
            ),
            (
                "HTTPS://Dev.Azure.com/o/p/_git/r/",
                repo("o", "p", "r", DevAzure),
            ),
            // A repository named as its project.
            (
                "https://dev.azure.com/o/_git/r",
                repo("o", "r", "r", DevAzure),
            ),
            (
                "https://o.visualstudio.com/p/_git/r",
                repo("o", "p", "r", VisualStudio),
            ),
            (
                "https://o.visualstudio.com/DefaultCollection/p/_git/r",
                repo("o", "p", "r", VisualStudio),
            ),
            (
                "https://o.visualstudio.com/_git/r",
                repo("o", "r", "r", VisualStudio),
            ),
            (
                "git@ssh.dev.azure.com:v3/o/My%20Project/r",
                repo("o", "My Project", "r", DevAzure),
            ),
            (
                "ssh://git@ssh.dev.azure.com/v3/o/p/r",
                repo("o", "p", "r", DevAzure),
            ),
            (
                "ssh://git@ssh.dev.azure.com:22/v3/o/p/r",
                repo("o", "p", "r", DevAzure),
            ),
            (
                "o@vs-ssh.visualstudio.com:v3/o/p/r",
                repo("o", "p", "r", VisualStudio),
            ),
            (
                "ssh://o@vs-ssh.visualstudio.com:22/v3/o/p/r",
                repo("o", "p", "r", VisualStudio),
            ),
            (
                " https://dev.azure.com/o/p/_git/r \n",
                repo("o", "p", "r", DevAzure),
            ),
        ] {
            assert_eq!(AzureRepo::from_url(url), expected, "{url}");
        }
    }

    #[test]
    fn other_remote_urls() {
        for url in [
            "https://github.com/o/r",
            "git@github.com:o/r.git",
            "https://dev.azure.com/o/p",
            "https://dev.azure.com/o/p/_git",
            "https://dev.azure.com/o/p/_git/r/pullrequest/1",
            "https://dev.azure.com/o/p/q/_git/r",
            "https://dev.azure.com.evil.example/o/p/_git/r",
            "https://evil.example/dev.azure.com/o/p/_git/r",
            "https://o.p.visualstudio.com/p/_git/r",
            "https://visualstudio.com/p/_git/r",
            "https://dev.azure.com/o/p/_git/a%2Fb",
            "https://dev.azure.com/o/p/_git/%",
            "https://dev.azure.com/o%20x/p/_git/r",
            "https://dev.azure.com/o/p/_git/r?x=1",
            "git@ssh.dev.azure.com:v3/o/p",
            "git@ssh.dev.azure.com:v3/o/p/r/x",
            "git@ssh.dev.azure.com:v2/o/p/r",
            "file:///srv/dev.azure.com/o/p/_git/r",
            "C:\\src\\r",
            "",
        ] {
            assert_eq!(AzureRepo::from_url(url), None, "{url}");
        }
    }

    /// Answers every request with the next of `answers`, and keeps the requests.
    struct Fake {
        answers: RefCell<Vec<Result<String, ForgeError>>>,
        asked: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(answers: Vec<Result<String, ForgeError>>) -> Fake {
            Fake {
                answers: RefCell::new(answers.into_iter().rev().collect()),
                asked: RefCell::new(Vec::new()),
            }
        }
    }

    impl Api for Fake {
        fn get(&self, path: &str) -> Result<String, ForgeError> {
            self.asked.borrow_mut().push(path.to_owned());
            self.answers.borrow_mut().pop().unwrap_or_else(|| {
                Err(ForgeError::Network(
                    Forge::AzureDevops,
                    "no more answers".into(),
                ))
            })
        }
    }

    /// An active pull request as Azure DevOps lists it, trimmed to what is read and a little.
    fn pr(id: u64, head: char, extra: &str) -> String {
        format!(
            r#"{{"pullRequestId":{id},"title":"PR {id}","isDraft":{draft},"status":"active",
                "createdBy":{{"displayName":"Some One","uniqueName":"some@one.example"}},
                "sourceRefName":"refs/heads/topic/{id}","targetRefName":"refs/heads/main",
                "lastMergeSourceCommit":{{"commitId":"{sha}"}}{extra}}}"#,
            draft = id == 8,
            sha = head.to_string().repeat(40),
        )
    }

    fn page(prs: &[String]) -> Result<String, ForgeError> {
        Ok(format!(
            r#"{{"value":[{}],"count":{}}}"#,
            prs.join(","),
            prs.len()
        ))
    }

    fn cjp() -> AzureRepo {
        AzureRepo::from_url("https://dev.azure.com/o/CJP%20Work/_git/some.repo").unwrap()
    }

    #[test]
    fn lists_the_active_pull_requests_of_the_repository() {
        let api = Fake::new(vec![page(&[
            pr(7, 'a', ""),
            pr(8, 'b', ""),
            // From a fork: left out, as on GitHub.
            pr(9, 'c', r#","forkSource":{"name":"refs/heads/x"}"#),
            pr(12, 'd', ""),
        ])]);
        let list = active_pull_requests(&api, &cjp()).unwrap();
        assert_eq!(
            *api.asked.borrow(),
            ["/CJP%20Work/_apis/git/repositories/some.repo/pullrequests\
              ?searchCriteria.status=active&$top=500&$skip=0&api-version=7.1"]
        );
        let numbers: Vec<u64> = list.iter().map(|p| p.number).collect();
        assert_eq!(numbers, [12, 8, 7], "newest first");
        let eight = &list[1];
        assert_eq!(
            *eight,
            PullRequest {
                number: 8,
                title: "PR 8".into(),
                author: "Some One".into(),
                draft: true,
                head: Oid::from_hex(&"b".repeat(40)).unwrap(),
                head_branch: "topic/8".into(),
                head_repo: Some("o/CJP Work/some.repo".into()),
                base_branch: "main".into(),
                base_repo: "o/CJP Work/some.repo".into(),
                url: "https://dev.azure.com/o/CJP%20Work/_git/some.repo/pullrequest/8".into(),
            }
        );
        assert_eq!(eight.head_label(), "topic/8");
    }

    #[test]
    fn asks_page_after_page() {
        let full: Vec<String> = (1..=500).map(|id| pr(id, 'a', "")).collect();
        let api = Fake::new(vec![page(&full), page(&[pr(501, 'b', "")])]);
        let list = active_pull_requests(&api, &cjp()).unwrap();
        assert_eq!(list.len(), 501);
        let asked = api.asked.borrow();
        assert_eq!(asked.len(), 2);
        assert!(asked[1].contains("&$top=500&$skip=500&"), "{}", asked[1]);
    }

    #[test]
    fn leaves_out_what_cant_be_placed() {
        let api = Fake::new(vec![page(&[
            // A merge still being worked out has no source commit yet.
            r#"{"pullRequestId":1,"title":"t","sourceRefName":"refs/heads/a",
                "targetRefName":"refs/heads/main"}"#
                .to_owned(),
            pr(2, 'z', ""),
            // Whoever opened it is gone.
            pr(3, 'a', "").replace(
                r#"{"displayName":"Some One","uniqueName":"some@one.example"}"#,
                "null",
            ),
        ])]);
        let list = active_pull_requests(&api, &cjp()).unwrap();
        let got: Vec<(u64, &str)> = list.iter().map(|p| (p.number, p.author.as_str())).collect();
        assert_eq!(got, [(3, "")]);
    }

    /// A repository whose git has `helper` as its only credential helper, none for `None`.
    fn with_helper(helper: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let ok = std::process::Command::new(parterre_core::git::program())
            .args(["init", "-q"])
            .current_dir(dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git init");
        // An empty helper first: not the system's or the user's, where Git Credential Manager
        // would answer.
        let mut config = "[credential]\n\thelper =\n".to_owned();
        if let Some(helper) = helper {
            let quoted = helper.replace('\\', "\\\\").replace('"', "\\\"");
            config.push_str(&format!("\thelper = \"{quoted}\"\n"));
        }
        let path = dir.path().join(".git/config");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(&config);
        std::fs::write(&path, text).unwrap();
        dir
    }

    const URL: &str = "https://dev.azure.com/o/p/_git/r";

    #[test]
    fn signs_in_with_what_git_has() {
        let dir = with_helper(Some(
            "!f() { test \"$1\" = get && echo username=me && echo password=secret-pat; }; f",
        ));
        let credential = sign_in(&Git::new(dir.path()), URL).unwrap();
        // A personal access token goes as a password, with no user name.
        assert_eq!(credential.authorization(), "Basic OnNlY3JldC1wYXQ=");
        assert!(!format!("{credential:?}").contains("secret"));
    }

    #[test]
    fn a_microsoft_entra_token_goes_as_a_bearer_token() {
        let dir = with_helper(Some(
            "!f() { echo username=me; echo password=eyJ0eXAi.eyJhdWQi.c2ln; }; f",
        ));
        let credential = sign_in(&Git::new(dir.path()), URL).unwrap();
        assert_eq!(credential.authorization(), "Bearer eyJ0eXAi.eyJhdWQi.c2ln");
    }

    #[test]
    fn without_a_sign_in_git_isnt_left_asking() {
        let dir = with_helper(None);
        match sign_in(&Git::new(dir.path()), URL) {
            Err(ForgeError::NoGitSignIn { url }) => assert_eq!(url, URL),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn signs_in_where_git_fetches_then_on_dev_azure_com() {
        let urls = |url: &str| sign_in_urls(url, &AzureRepo::from_url(url).unwrap());
        assert_eq!(
            urls("https://o.visualstudio.com/My%20Project/_git/r"),
            [
                (
                    "https://o.visualstudio.com/My%20Project/_git/r".to_owned(),
                    "https://o.visualstudio.com".to_owned()
                ),
                (
                    "https://dev.azure.com/o/My%20Project/_git/r".to_owned(),
                    "https://dev.azure.com/o".to_owned()
                ),
            ]
        );
        assert_eq!(
            urls(URL),
            [(URL.to_owned(), "https://dev.azure.com/o".to_owned())]
        );
        // Over SSH git signs in to nothing; HTTPS is asked as the clone over it would be.
        assert_eq!(
            urls("git@ssh.dev.azure.com:v3/o/p/r"),
            [(URL.to_owned(), "https://dev.azure.com/o".to_owned())]
        );
    }

    #[test]
    fn errors_are_passed_on() {
        let api = Fake::new(vec![Ok("<html>".into())]);
        assert!(matches!(
            active_pull_requests(&api, &cjp()),
            Err(ForgeError::Parse(Forge::AzureDevops, _))
        ));
        let api = Fake::new(vec![Err(ForgeError::GitSignInRejected { url: "u".into() })]);
        assert!(matches!(
            active_pull_requests(&api, &cjp()),
            Err(ForgeError::GitSignInRejected { .. })
        ));
    }
}
