//! Azure DevOps' API over HTTPS, signed in with what git's credential helpers have for it,
//! never asking the user.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use super::{Api, AzureRepo, Host, clone_root};
use crate::http::{self, Token};
use crate::{Forge, ForgeError};
use parterre_core::git::{Git, GitError};

/// Git Credential Manager may renew the sign-in over the network.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(20);
/// `{"` in Base64: how every JWT starts.
const JWT_START: &str = "eyJ";
/// Of a `Retry-After` that isn't there, or is a date.
const UNSAID_RETRY_AFTER_SECS: u64 = 60;

pub(super) fn signed_in_to_repository<T>(
    git: &Git,
    remote_url: &str,
    repo: &AzureRepo,
    calls: impl Fn(&dyn Api) -> Result<T, ForgeError>,
) -> Result<T, ForgeError> {
    let urls = sign_in_urls(remote_url, repo);
    signed_in(Some(git.dir()), urls, &repo.full_name(), calls)
}

pub(super) fn signed_in_to_organisation<T>(
    org: &str,
    calls: impl Fn(&dyn Api) -> Result<T, ForgeError>,
) -> Result<T, ForgeError> {
    let url = SignInUrl {
        url: clone_root(org),
        api_root: Host::DevAzure.organisation_url(org),
    };
    signed_in(None, vec![url], org, calls)
}

/// A URL to ask git for a sign-in for, and the root of the API that sign-in is sent to: the
/// only place it goes.
#[derive(Debug, PartialEq, Eq)]
struct SignInUrl {
    url: String,
    api_root: String,
}

/// In order: the HTTPS URL git fetches from, then the URL Azure DevOps gives to clone it, which
/// Git Credential Manager may know instead of `{org}.visualstudio.com` (it did where this was
/// tried), or of a URL with no user.
fn sign_in_urls(remote_url: &str, repo: &AzureRepo) -> Vec<SignInUrl> {
    const HTTPS: &str = "https://";
    let remote_url = remote_url.trim();
    let mut urls = Vec::new();
    if remote_url
        .get(..HTTPS.len())
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case(HTTPS))
    {
        urls.push(SignInUrl {
            url: remote_url.to_owned(),
            api_root: repo.host.organisation_url(&repo.org),
        });
    }
    let clone_url = repo.clone_url();
    if urls
        .iter()
        .all(|u| u.url.trim_end_matches('/') != clone_url)
    {
        urls.push(SignInUrl {
            url: clone_url,
            api_root: Host::DevAzure.organisation_url(&repo.org),
        });
    }
    urls
}

/// Runs `calls` against the API, signed in with the first sign-in git has of `urls` that Azure
/// DevOps takes; without one, Azure DevOps is not asked. `dir` is the repository whose settings
/// count too, if any, and `asked_about` what Azure DevOps may not find.
fn signed_in<T>(
    dir: Option<&Path>,
    urls: Vec<SignInUrl>,
    asked_about: &str,
    calls: impl Fn(&dyn Api) -> Result<T, ForgeError>,
) -> Result<T, ForgeError> {
    let agent = http::agent();
    let mut failed = None;
    for SignInUrl { url, api_root } in urls {
        let answer = sign_in(dir, &url).and_then(|credential| {
            calls(&Http {
                agent: agent.clone(),
                api_root,
                authorization: Token(credential.authorization()),
                sign_in_url: url,
                asked_about,
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

/// The sign-in git's credential helpers have for `url`, as git would fetch with it:
/// [`ForgeError::NoGitSignIn`] if there is none, or it isn't given within [`SIGN_IN_TIMEOUT`].
/// `dir` is the repository whose settings count too, if any.
fn sign_in(dir: Option<&Path>, url: &str) -> Result<Credential, ForgeError> {
    let mut cmd = Command::new(parterre_core::git::program());
    parterre_core::shell_path::apply(&mut cmd);
    match dir {
        Some(dir) => cmd.arg("-C").arg(dir),
        // Not wherever parterre was started, whose repository's settings would count.
        None => cmd.current_dir(std::env::temp_dir()),
    };
    cmd.args(["-c", "credential.interactive=false", "credential", "fill"])
        // No prompt in a terminal, and no askpass program (an empty one stops git from falling
        // back to `core.askPass` and `SSH_ASKPASS`).
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("GCM_INTERACTIVE", "never");
    let no_sign_in = || ForgeError::NoGitSignIn {
        url: url.to_owned(),
    };
    let out = crate::output_within(&mut cmd, &format!("url={url}\n\n"), SIGN_IN_TIMEOUT)
        .map_err(GitError::Spawn)?
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

#[derive(Debug)]
struct Credential {
    password: Token,
}

impl Credential {
    /// The `Authorization` header: a Microsoft Entra token (a JWT) as a bearer token, anything
    /// else, such as a personal access token, as the password of Basic authentication.
    fn authorization(&self) -> String {
        let Token(password) = &self.password;
        if password.starts_with(JWT_START) {
            format!("Bearer {password}")
        } else {
            format!("Basic {}", base64(format!(":{password}").as_bytes()))
        }
    }
}

/// Standard Base64, with padding.
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

/// One organisation's API, the only place its sign-in is sent.
#[derive(Debug)]
struct Http<'a> {
    agent: ureq::Agent,
    /// `https://dev.azure.com/{org}` or `https://{org}.visualstudio.com`.
    api_root: String,
    authorization: Token,
    sign_in_url: String,
    asked_about: &'a str,
}

impl Api for Http<'_> {
    fn get(&self, path: &str) -> Result<String, ForgeError> {
        let Token(authorization) = &self.authorization;
        let mut response = self
            .agent
            .get(&format!("{}{path}", self.api_root))
            .header("Authorization", authorization)
            .header("Accept", "application/json")
            // A 401, not a redirect to the sign-in page, for a sign-in it doesn't take.
            .header("X-TFS-FedAuthRedirect", "Suppress")
            .call()
            .map_err(http::network_error(Forge::AzureDevops))?;
        let retry_after = http::number_header(&response, "retry-after");
        let status = response.status().as_u16();
        let body = http::read_body(&mut response, Forge::AzureDevops)?;
        match status {
            200 => Ok(body),
            // 203 is the sign-in page, as are redirects; 403 a sign-in that may not read code.
            203 | 300..=399 | 401 | 403 => Err(ForgeError::GitSignInRejected {
                url: self.sign_in_url.clone(),
            }),
            404 => Err(ForgeError::NotFound {
                forge: Forge::AzureDevops,
                repo: self.asked_about.to_owned(),
            }),
            429 => Err(throttled(retry_after)),
            503 if retry_after.is_some() => Err(throttled(retry_after)),
            _ => Err(http::status_error(Forge::AzureDevops, status, &body)),
        }
    }
}

fn throttled(retry_after_secs: Option<u64>) -> ForgeError {
    let secs = retry_after_secs.unwrap_or(UNSAID_RETRY_AFTER_SECS);
    ForgeError::Throttled {
        minutes: secs.div_ceil(60).max(1),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    /// A repository whose git has `helper` as its only credential helper, none for `None`.
    fn with_helper(helper: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let ok = Command::new(parterre_core::git::program())
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
        std::fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join(".git/config"))
            .and_then(|mut file| file.write_all(config.as_bytes()))
            .unwrap();
        dir
    }

    const URL: &str = "https://dev.azure.com/o/p/_git/r";

    #[test]
    fn signs_in_with_what_git_has() {
        let dir = with_helper(Some(
            "!f() { test \"$1\" = get && echo username=me && echo password=secret-pat; }; f",
        ));
        let credential = sign_in(Some(dir.path()), URL).unwrap();
        // A personal access token goes as a password, with no user name.
        assert_eq!(credential.authorization(), "Basic OnNlY3JldC1wYXQ=");
        assert!(!format!("{credential:?}").contains("secret"));
    }

    #[test]
    fn a_microsoft_entra_token_goes_as_a_bearer_token() {
        let dir = with_helper(Some(
            "!f() { echo username=me; echo password=eyJ0eXAi.eyJhdWQi.c2ln; }; f",
        ));
        let credential = sign_in(Some(dir.path()), URL).unwrap();
        assert_eq!(credential.authorization(), "Bearer eyJ0eXAi.eyJhdWQi.c2ln");
    }

    #[test]
    fn without_a_sign_in_git_isnt_left_asking() {
        let dir = with_helper(None);
        match sign_in(Some(dir.path()), URL) {
            Err(ForgeError::NoGitSignIn { url }) => assert_eq!(url, URL),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn signs_in_where_git_fetches_then_on_dev_azure_com() {
        let urls = |url: &str| sign_in_urls(url, &AzureRepo::from_url(url).unwrap());
        let sign_in_url = |url: &str, api_root: &str| SignInUrl {
            url: url.to_owned(),
            api_root: api_root.to_owned(),
        };
        assert_eq!(
            urls("https://o.visualstudio.com/My%20Project/_git/r"),
            [
                sign_in_url(
                    "https://o.visualstudio.com/My%20Project/_git/r",
                    "https://o.visualstudio.com"
                ),
                sign_in_url(
                    "https://o@dev.azure.com/o/My%20Project/_git/r",
                    "https://dev.azure.com/o"
                ),
            ]
        );
        // The clone URL Azure DevOps gives names the organisation as the user, so Git
        // Credential Manager finds it without `credential.useHttpPath`.
        let cloned = "https://o@dev.azure.com/o/p/_git/r";
        assert_eq!(
            urls(cloned),
            [sign_in_url(cloned, "https://dev.azure.com/o")]
        );
        assert_eq!(urls(&format!("{cloned}/")).len(), 1);
        // Over SSH git signs in to nothing; HTTPS is asked as the clone over it would be.
        assert_eq!(
            urls("git@ssh.dev.azure.com:v3/o/p/r"),
            [sign_in_url(cloned, "https://dev.azure.com/o")]
        );
    }

    #[test]
    fn throttled_for_at_least_a_minute() {
        let minutes = |retry_after| match throttled(retry_after) {
            ForgeError::Throttled { minutes } => minutes,
            other => panic!("{other:?}"),
        };
        assert_eq!(minutes(None), 1);
        assert_eq!(minutes(Some(0)), 1);
        assert_eq!(minutes(Some(61)), 2);
    }
}
