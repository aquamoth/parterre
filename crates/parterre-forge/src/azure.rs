//! Active pull requests from Azure DevOps (#102), shown as GitHub's open ones are: those of
//! `origin`'s repository, on their head commits where those have been fetched.
//!
//! - **How:** one REST request per 500 active pull requests. Azure DevOps has no GraphQL, and
//!   its repositories tend to have few active pull requests but many branches, so this is
//!   cheaper than asking per branch as on GitHub.
//! - **Signed in as git is:** with the sign-in git's credential helper, Git Credential
//!   Manager's on most machines, has for `origin`'s HTTPS URL, never asking for one. Not `az`:
//!   its account can be another than git's (it was, where this was tried), and it would be a
//!   second program to rely on. Research: `docs/research/github-forks-and-pull-requests.md` §15.
//! - **Forks:** their pull requests are left out; forks are rare on Azure DevOps.

use std::cmp::Reverse;
use std::fmt::Write as _;

use super::{
    Forge, ForgeError, GoneBranch, MERGED_PER_BRANCH, Proposed, PullRequest, PullRequests, Remote,
    RemoteUrl,
};
use parterre_core::git::Git;
use parterre_core::merged::Merged;

mod repositories;
#[cfg(feature = "azure-devops")]
mod sign_in;

pub use repositories::repositories;
#[cfg(feature = "azure-devops")]
use sign_in::{signed_in_to_organisation, signed_in_to_repository};

const API_VERSION: &str = "api-version=7.1";
const PULL_REQUESTS_PER_PAGE: usize = 500;
/// 5,000 active pull requests are more than any repository has.
const MAX_PAGES: usize = 10;
/// What Azure DevOps doesn't take in project and repository names.
const NOT_IN_NAMES: &str = "\\/:*?\"<>|;#${},+=[]%";

/// Blocks: run it on a worker thread.
pub fn load(git: &Git) -> Result<PullRequests, ForgeError> {
    pull_requests(git, |url, origin| {
        load_active_pull_requests(git, url, origin)
    })
}

/// For each of `branches`: the completed pull request from it into `origin`'s repository,
/// whose head was its tip when completed. One request per branch, signed in as git is for
/// `origin`'s URL; 11 took 3.2 s, one after another, on a large organisation (#384).
pub(crate) fn merged(
    git: &Git,
    origin: &AzureRepo,
    branches: &[GoneBranch],
) -> Result<Vec<Option<Merged>>, ForgeError> {
    let url = git
        .query(&["remote", "get-url", "origin"])?
        .ok_or(ForgeError::NoForge)?;
    signed_in_to_repository(git, &url, origin, |api| {
        completed_pull_requests(api, origin, branches)
    })
}

fn completed_pull_requests(
    api: &dyn Api,
    repo: &AzureRepo,
    branches: &[GoneBranch],
) -> Result<Vec<Option<Merged>>, ForgeError> {
    let path = format!(
        "/{}/_apis/git/repositories/{}/pullrequests",
        percent_encoded(&repo.project),
        percent_encoded(&repo.name),
    );
    branches
        .iter()
        .map(|b| {
            let source = percent_encoded(&format!("refs/heads/{}", b.name));
            let body = api.get(&format!(
                "{path}?searchCriteria.sourceRefName={source}&searchCriteria.status=completed\
                 &$top={MERGED_PER_BRANCH}&{API_VERSION}"
            ))?;
            let answer: json::List<json::CompletedPullRequest> =
                super::parse_answer(Forge::AzureDevops, &body)?;
            Ok(answer
                .value
                .into_iter()
                .filter(|pr| pr.fork_source.is_none())
                .find_map(|pr| pr.into_merged(b.tip)))
        })
        .collect()
}

/// A branch of an Azure DevOps remote, whose URL is `remote_url`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RemoteBranch<'a> {
    pub remote_url: &'a str,
    pub repo: &'a AzureRepo,
    pub name: &'a str,
}

/// For each branch: whether an active pull request proposes it. Asks each repository once,
/// signed in as git is there; `canned`, `--pull-requests-from`'s list, is used instead if given.
pub(crate) fn proposals(
    git: &Git,
    branches: &[RemoteBranch],
    canned: Option<&str>,
) -> Vec<Proposed> {
    let mut asked: Vec<(String, Result<Vec<PullRequest>, ForgeError>)> = Vec::new();
    let mut proposed = Vec::with_capacity(branches.len());
    for branch in branches {
        let repo_name = branch.repo.full_name();
        let i = asked
            .iter()
            .position(|(asked_name, _)| asked_name.eq_ignore_ascii_case(&repo_name))
            .unwrap_or_else(|| {
                let found = match canned {
                    Some(json) => load_canned(git, json).map(|p| p.list),
                    None => load_active_pull_requests(git, branch.remote_url, branch.repo),
                };
                asked.push((repo_name.clone(), found));
                asked.len() - 1
            });
        let (_, found) = &asked[i];
        proposed.push(Proposed::among(found, &repo_name, branch.name));
    }
    proposed
}

/// Pull requests from `json` instead of Azure DevOps (`--pull-requests-from`), as
/// [`super::github::load_canned`] reads them.
pub fn load_canned(git: &Git, json: &str) -> Result<PullRequests, ForgeError> {
    pull_requests(git, |_, origin| {
        let web_url = origin.web_url();
        super::canned(git, json, &origin.full_name(), |number| {
            pull_request_url(&web_url, number)
        })
    })
}

/// The pull requests `find` finds for `origin`, given its remote URL, with the remotes and
/// upstreams that place them.
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

fn load_active_pull_requests(
    git: &Git,
    remote_url: &str,
    repo: &AzureRepo,
) -> Result<Vec<PullRequest>, ForgeError> {
    signed_in_to_repository(git, remote_url, repo, |api| active_pull_requests(api, repo))
}

#[cfg(not(feature = "azure-devops"))]
fn signed_in_to_repository<T>(
    _: &Git,
    _: &str,
    _: &AzureRepo,
    _: impl Fn(&dyn Api) -> Result<T, ForgeError>,
) -> Result<T, ForgeError> {
    Err(ForgeError::Unsupported(Forge::AzureDevops))
}

#[cfg(not(feature = "azure-devops"))]
fn signed_in_to_organisation<T>(
    _: &str,
    _: impl Fn(&dyn Api) -> Result<T, ForgeError>,
) -> Result<T, ForgeError> {
    Err(ForgeError::Unsupported(Forge::AzureDevops))
}

/// Requests to an organisation's API; a trait so that tests can answer them.
trait Api {
    /// The body of a successful answer to `GET {organisation}{path}`.
    fn get(&self, path: &str) -> Result<String, ForgeError>;
}

/// Newest first: not from a fork (they are rare on Azure DevOps, and GitHub's other forks' are
/// left out too), and with a head commit (none while a merge is being worked out).
fn active_pull_requests(api: &dyn Api, repo: &AzureRepo) -> Result<Vec<PullRequest>, ForgeError> {
    let repo_name = repo.full_name();
    let web_url = repo.web_url();
    let path = format!(
        "/{}/_apis/git/repositories/{}/pullrequests",
        percent_encoded(&repo.project),
        percent_encoded(&repo.name),
    );
    let mut list = Vec::new();
    for page in 0..MAX_PAGES {
        let skip = page * PULL_REQUESTS_PER_PAGE;
        let body = api.get(&format!(
            "{path}?searchCriteria.status=active&$top={PULL_REQUESTS_PER_PAGE}&$skip={skip}\
             &{API_VERSION}"
        ))?;
        let answer: json::List<json::PullRequest> = super::parse_answer(Forge::AzureDevops, &body)?;
        let is_full = answer.value.len() == PULL_REQUESTS_PER_PAGE;
        list.extend(
            answer
                .value
                .into_iter()
                .filter(|pr| pr.fork_source.is_none())
                .filter_map(|pr| pr.into_pull_request(&repo_name, &web_url)),
        );
        if !is_full {
            break;
        }
    }
    list.sort_by_key(|pr| Reverse(pr.number));
    Ok(list)
}

fn branch_name(full_name: &str) -> &str {
    full_name.strip_prefix("refs/heads/").unwrap_or(full_name)
}

fn pull_request_url(web_url: &str, number: u64) -> String {
    format!("{web_url}/pullrequest/{number}")
}

/// All but letters, digits and `-._~`.
fn percent_encoded(name: &str) -> String {
    let mut encoded = String::with_capacity(name.len());
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            encoded.push(char::from(b));
        } else {
            let _ = write!(encoded, "%{b:02X}");
        }
    }
    encoded
}

fn percent_decoded(text: &str) -> Option<String> {
    fn hex_digit(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let [first, tail @ ..] = rest {
        if *first == b'%' {
            let [high, low, tail @ ..] = tail else {
                return None;
            };
            bytes.push((hex_digit(*high)? << 4) | hex_digit(*low)?);
            rest = tail;
        } else {
            bytes.push(*first);
            rest = tail;
        }
    }
    String::from_utf8(bytes).ok()
}

mod json {
    use serde::Deserialize;
    use serde::de::IgnoredAny;

    use parterre_core::merged::{How, Merged};
    use parterre_core::oid::Oid;

    #[derive(Debug, Deserialize)]
    pub struct List<T> {
        pub value: Vec<T>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PullRequest {
        pull_request_id: u64,
        title: String,
        #[serde(default)]
        is_draft: bool,
        created_by: Option<Identity>,
        source_ref_name: String,
        target_ref_name: String,
        last_merge_source_commit: Option<Commit>,
        pub fork_source: Option<IgnoredAny>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Identity {
        display_name: String,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Commit {
        commit_id: String,
    }

    impl PullRequest {
        /// `None` without a head commit. `repo_name` is its base and head repository, and
        /// `web_url` that repository's page.
        pub fn into_pull_request(
            self,
            repo_name: &str,
            web_url: &str,
        ) -> Option<crate::PullRequest> {
            Some(crate::PullRequest {
                number: self.pull_request_id,
                title: self.title,
                author: self.created_by.map(|a| a.display_name).unwrap_or_default(),
                draft: self.is_draft,
                head: Oid::from_hex(&self.last_merge_source_commit?.commit_id)?,
                head_branch: super::branch_name(&self.source_ref_name).to_owned(),
                head_repo: Some(repo_name.to_owned()),
                base_branch: super::branch_name(&self.target_ref_name).to_owned(),
                base_repo: repo_name.to_owned(),
                url: super::pull_request_url(web_url, self.pull_request_id),
            })
        }
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CompletedPullRequest {
        pull_request_id: u64,
        last_merge_source_commit: Option<Commit>,
        last_merge_commit: Option<Commit>,
        completion_options: Option<CompletionOptions>,
        pub fork_source: Option<IgnoredAny>,
    }

    /// `mergeStrategy` is `GitPullRequestMergeStrategy`, by name or by number (1
    /// `noFastForward`, 2 `squash`, 3 `rebase`, 4 `rebaseMerge`); older pull requests have
    /// only `squashMerge`.
    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct CompletionOptions {
        merge_strategy: Option<serde_json::Value>,
        #[serde(default)]
        squash_merge: bool,
    }

    impl CompletionOptions {
        fn how(&self) -> Option<How> {
            use serde_json::Value;
            match &self.merge_strategy {
                Some(Value::String(s)) if s == "squash" => Some(How::Squash),
                Some(Value::String(s)) if s == "rebase" || s == "rebaseMerge" => Some(How::Rebase),
                Some(Value::Number(n)) if n.as_u64() == Some(2) => Some(How::Squash),
                Some(Value::Number(n)) if matches!(n.as_u64(), Some(3 | 4)) => Some(How::Rebase),
                _ => self.squash_merge.then_some(How::Squash),
            }
        }
    }

    impl CompletedPullRequest {
        /// `None` unless its head was `tip` when it was completed.
        pub fn into_merged(self, tip: Oid) -> Option<Merged> {
            let head = Oid::from_hex(&self.last_merge_source_commit?.commit_id)?;
            let how = self
                .completion_options
                .as_ref()
                .and_then(CompletionOptions::how);
            (head == tip).then(|| Merged {
                how,
                landed: self
                    .last_merge_commit
                    .and_then(|c| Oid::from_hex(&c.commit_id)),
                pull_request: Some(self.pull_request_id),
            })
        }
    }
}

/// Where an Azure DevOps organisation is served: `dev.azure.com/{org}`, or the older
/// `{org}.visualstudio.com`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    DevAzure,
    VisualStudio,
}

impl Host {
    /// The root of its web pages and of its API.
    fn organisation_url(self, org: &str) -> String {
        match self {
            Host::DevAzure => format!("https://dev.azure.com/{org}"),
            Host::VisualStudio => format!("https://{org}.visualstudio.com"),
        }
    }
}

/// `https://{org}@dev.azure.com/{org}`, as the clone URLs Azure DevOps gives start: with the
/// organisation as the user, Git Credential Manager finds it without `credential.useHttpPath`.
fn clone_root(org: &str) -> String {
    format!("https://{org}@dev.azure.com/{org}")
}

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
        let url = RemoteUrl::parse(url)?;
        let transport = match url.scheme.as_deref() {
            Some("https" | "http") => Transport::Http,
            Some("ssh") | None => Transport::Ssh,
            Some(_) => return None,
        };
        let path = url.path.strip_suffix('/').unwrap_or(url.path);
        let segments: Vec<&str> = path.split('/').collect();
        let (org, host, project, name) = match (transport, url.host.as_str(), segments.as_slice()) {
            (Transport::Http, "dev.azure.com", [org, rest @ ..]) => {
                let (project, name) = git_path(rest)?;
                (*org, Host::DevAzure, project, name)
            }
            (Transport::Http, host, segments) => {
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
            (Transport::Ssh, "ssh.dev.azure.com", ["v3", org, project, name]) => {
                (*org, Host::DevAzure, *project, *name)
            }
            (Transport::Ssh, "vs-ssh.visualstudio.com", ["v3", org, project, name]) => {
                (*org, Host::VisualStudio, *project, *name)
            }
            _ => return None,
        };
        if !is_organisation_name(org) {
            return None;
        }
        Some(AzureRepo {
            org: org.to_owned(),
            project: decoded_name(project)?,
            name: decoded_name(name)?,
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
            "{}/{}/_git/{}",
            Host::DevAzure.organisation_url(&self.org),
            percent_encoded(&self.project),
            percent_encoded(&self.name)
        )
    }

    /// The HTTPS URL to clone it from, as Azure DevOps gives it.
    pub fn clone_url(&self) -> String {
        format!(
            "{}/{}/_git/{}",
            clone_root(&self.org),
            percent_encoded(&self.project),
            percent_encoded(&self.name),
        )
    }
}

enum Transport {
    Http,
    Ssh,
}

fn is_organisation_name(org: &str) -> bool {
    !org.is_empty() && org.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// `{project}/_git/{repo}`, or `_git/{repo}` for a repository named as its project.
fn git_path<'a>(segments: &[&'a str]) -> Option<(&'a str, &'a str)> {
    match segments {
        [project, "_git", name] => Some((project, name)),
        ["_git", name] => Some((name, name)),
        _ => None,
    }
}

/// A project or repository name from a URL; `None` if Azure DevOps doesn't take it as one.
fn decoded_name(segment: &str) -> Option<String> {
    let name = percent_decoded(segment)?;
    let is_name = !name.is_empty()
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| c.is_control() || NOT_IN_NAMES.contains(c));
    is_name.then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;
    use parterre_core::merged::How;
    use parterre_core::oid::Oid;

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

    impl Api for Fake<String> {
        fn get(&self, path: &str) -> Result<String, ForgeError> {
            self.answer(Forge::AzureDevops, path)
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

    fn completed(id: u64, head: char, merged: char, extra: &str) -> String {
        format!(
            r#"{{"pullRequestId":{id},"status":"completed",
                "lastMergeSourceCommit":{{"commitId":"{sha}"}},
                "lastMergeCommit":{{"commitId":"{merge}"}}{extra}}}"#,
            sha = head.to_string().repeat(40),
            merge = merged.to_string().repeat(40),
        )
    }

    fn sha(c: char) -> Oid {
        Oid::from_hex(&c.to_string().repeat(40)).unwrap()
    }

    /// #384: one request per branch; only a pull request whose head was the branch's tip.
    #[test]
    fn completed_pull_requests_are_found_by_branch_and_tip() {
        let api = Fake::new(vec![
            page(&[
                // The name used again: a newer pull request from another tip first.
                completed(12, 'e', '2', ""),
                completed(
                    7,
                    'a',
                    '1',
                    r#","completionOptions":{"mergeStrategy":"squash"}"#,
                ),
            ]),
            page(&[completed(
                9,
                'b',
                '3',
                r#","completionOptions":{"mergeStrategy":"rebaseMerge"}"#,
            )]),
            page(&[completed(5, 'x', '5', "")]),
            // The strategy by number, and an older pull request's flag.
            page(&[completed(
                6,
                'd',
                '6',
                r#","completionOptions":{"mergeStrategy":2}"#,
            )]),
            page(&[completed(
                8,
                'f',
                '8',
                r#","completionOptions":{"squashMerge":true}"#,
            )]),
        ]);
        let branches = [
            GoneBranch {
                name: "fix/x".into(),
                tip: sha('a'),
            },
            GoneBranch {
                name: "topic".into(),
                tip: sha('b'),
            },
            GoneBranch {
                name: "abandoned".into(),
                tip: sha('c'),
            },
            GoneBranch {
                name: "numbered".into(),
                tip: sha('d'),
            },
            GoneBranch {
                name: "old".into(),
                tip: sha('f'),
            },
        ];
        let squashed = |id: u64, landed: char| {
            Some(Merged {
                how: Some(How::Squash),
                landed: Some(sha(landed)),
                pull_request: Some(id),
            })
        };
        let found = completed_pull_requests(&api, &cjp(), &branches).unwrap();
        assert_eq!(
            found,
            [
                squashed(7, '1'),
                Some(Merged {
                    how: Some(How::Rebase),
                    landed: Some(sha('3')),
                    pull_request: Some(9),
                }),
                None,
                squashed(6, '6'),
                squashed(8, '8'),
            ]
        );
        assert_eq!(
            api.asked.borrow()[0],
            "/CJP%20Work/_apis/git/repositories/some.repo/pullrequests\
             ?searchCriteria.sourceRefName=refs%2Fheads%2Ffix%2Fx\
             &searchCriteria.status=completed&$top=10&api-version=7.1"
        );
    }
}
