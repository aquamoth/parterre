//! The repositories of the organisations git knows of, to clone one. Azure DevOps can't be
//! asked which a user belongs to: a personal access token for one organisation can't see the
//! profile (research §15).

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;

use super::{API_VERSION, Api, AzureRepo, Host, json, signed_in_to_organisation};
use crate::repositories::{Listed, Repositories};
use crate::{Forge, ForgeError};
use parterre_core::git::Git;

/// Git Credential Manager's settings for an organisation, `credential.azrepos:org/{org}.*`,
/// which `git credential-manager azure-repos bind` writes.
const ORGANISATION_SETTINGS: &str = "credential.azrepos:org/";
const ORGANISATION_SETTINGS_PATTERN: &str = r"^credential\.azrepos:org/";
const GIT_CONFIG_TIMEOUT: Duration = Duration::from_secs(5);

/// Each organisation's, by name, signed in as git is there. One without a sign-in, or that Azure
/// DevOps doesn't find, is left out quietly: it may be named only by an SSH remote, or long
/// gone. Another failure is the answer only if no organisation listed any. Blocks: run it on a
/// worker thread.
pub fn repositories(known: &[PathBuf]) -> Result<Repositories, ForgeError> {
    if !cfg!(feature = "azure-devops") {
        return Err(ForgeError::Unsupported(Forge::AzureDevops));
    }
    let mut list = Vec::new();
    let mut failed = None;
    for org in organisations(known) {
        match signed_in_to_organisation(&org, |api| org_repositories(api, &org)) {
            Ok(repositories) => list.extend(repositories),
            Err(e) if e.needs_sign_in() || matches!(e, ForgeError::NotFound { .. }) => {}
            Err(e) => {
                failed.get_or_insert(e);
            }
        }
    }
    match failed {
        Some(e) if list.is_empty() => Err(e),
        _ => Ok(Repositories { list }),
    }
}

/// Those Git Credential Manager has settings for, then those of the Azure DevOps remotes of
/// `known` repositories.
fn organisations(known: &[PathBuf]) -> Vec<String> {
    let mut cmd = Command::new(parterre_core::git::program());
    parterre_core::shell_path::apply(&mut cmd)
        .args(["config", "--get-regexp", ORGANISATION_SETTINGS_PATTERN])
        // The user's and the system's settings, not those of a repository parterre was
        // started in.
        .current_dir(std::env::temp_dir());
    // None set is an exit code of 1.
    let config = crate::output_within(&mut cmd, "", GIT_CONFIG_TIMEOUT)
        .ok()
        .flatten()
        .unwrap_or_default();
    let remote_urls: Vec<String> = known
        .iter()
        .flat_map(|dir| crate::remote_urls(&Git::new(dir)).unwrap_or_default())
        .map(|(_, url)| url)
        .collect();
    organisations_from(&config, remote_urls.iter().map(String::as_str))
}

/// From `git config --get-regexp`'s lines and remote URLs: each once, whatever its case, as
/// first named.
fn organisations_from<'a>(
    config: &str,
    remote_urls: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let bound = config.lines().filter_map(|line| {
        let key = line.split_once(' ').map_or(line, |(key, _)| key);
        let (org, _setting) = key.strip_prefix(ORGANISATION_SETTINGS)?.rsplit_once('.')?;
        super::is_organisation_name(org).then(|| org.to_owned())
    });
    let of_remotes = remote_urls
        .into_iter()
        .filter_map(|url| Some(AzureRepo::from_url(url)?.org));
    let mut orgs: Vec<String> = Vec::new();
    for org in bound.chain(of_remotes) {
        if !orgs.iter().any(|o| o.eq_ignore_ascii_case(&org)) {
            orgs.push(org);
        }
    }
    orgs
}

/// Those that can be cloned (not disabled), by name, with the HTTPS clone URLs Azure DevOps
/// gives.
fn org_repositories(api: &dyn Api, org: &str) -> Result<Vec<Listed>, ForgeError> {
    let body = api.get(&format!("/_apis/git/repositories?{API_VERSION}"))?;
    let answer: json::List<Repository> = crate::parse_answer(Forge::AzureDevops, &body)?;
    let mut list: Vec<Listed> = answer
        .value
        .into_iter()
        .filter(|r| !r.is_disabled)
        .map(|r| r.into_listed(org))
        .collect();
    list.sort_by_cached_key(|l| l.name.to_lowercase());
    Ok(list)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repository {
    name: String,
    project: Project,
    #[serde(default)]
    is_disabled: bool,
    #[serde(default)]
    is_fork: bool,
}

#[derive(Debug, Deserialize)]
struct Project {
    name: String,
    /// `private` or `public`.
    #[serde(default)]
    visibility: String,
}

impl Repository {
    fn into_listed(self, org: &str) -> Listed {
        let repo = AzureRepo {
            org: org.to_owned(),
            project: self.project.name,
            name: self.name,
            host: Host::DevAzure,
        };
        Listed {
            forge: Forge::AzureDevops,
            name: repo.full_name(),
            description: String::new(),
            private: self.project.visibility != "public",
            fork: self.is_fork,
            archived: false,
            url: repo.clone_url(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    #[test]
    fn organisations_are_those_git_knows_of() {
        // `git config --get-regexp`: Git Credential Manager's settings per organisation.
        let config = "credential.azrepos:org/cosmojpn.username me@cosmo.example\n\
                      credential.azrepos:org/cosmojpn.azureauthority https://login.example/x\n\
                      credential.azrepos:org/Other-Org.username them\n";
        let remotes = [
            "https://dev.azure.com/third/p/_git/r",
            "https://github.com/o/r",
            "git@ssh.dev.azure.com:v3/COSMOJPN/p/r",
            "https://other-org.visualstudio.com/p/_git/r",
        ];
        assert_eq!(
            organisations_from(config, remotes),
            ["cosmojpn", "Other-Org", "third"]
        );
        assert!(organisations_from("", []).is_empty());
    }

    #[test]
    fn lists_an_organisations_repositories_to_clone() {
        let api = Fake::new(vec![Ok(r#"{"count":4,"value":[
            {"name":"Keycloak","project":{"name":"CJP Work","visibility":"public"},
                "isDisabled":false,"size":10},
            {"name":"apps","project":{"name":"Apps","visibility":"private"},"isDisabled":false},
            {"name":"old","project":{"name":"Apps"},"isDisabled":true},
            {"name":"Copy","project":{"name":"Apps"},"isFork":true}
        ]}"#
        .into())]);
        let list = org_repositories(&api, "o").unwrap();
        assert_eq!(
            *api.asked.borrow(),
            ["/_apis/git/repositories?api-version=7.1"]
        );
        let got: Vec<(&str, &str, bool, bool)> = list
            .iter()
            .map(|l| (l.name.as_str(), l.url.as_str(), l.fork, l.private))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "o/Apps/apps",
                    "https://o@dev.azure.com/o/Apps/_git/apps",
                    false,
                    true
                ),
                (
                    "o/Apps/Copy",
                    "https://o@dev.azure.com/o/Apps/_git/Copy",
                    true,
                    true
                ),
                (
                    "o/CJP Work/Keycloak",
                    "https://o@dev.azure.com/o/CJP%20Work/_git/Keycloak",
                    false,
                    false
                ),
            ],
            "by name, a disabled one left out, private unless its project is public"
        );
        assert!(list.iter().all(|l| l.forge == Forge::AzureDevops));
        // What it clones is found again.
        let repos = crate::Repositories { list };
        assert!(repos.find_url(&repos.list[2].url).is_some());
    }
}
