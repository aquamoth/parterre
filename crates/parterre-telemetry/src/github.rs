//! GitHub's releases API, asked without signing in and with nothing of parterre's own: no
//! version, no ID, only the user agent GitHub requires.

use std::time::Duration;

use crate::Release;

const RELEASES: &str = "https://api.github.com/repos/aquamoth/parterre/releases";
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

/// The 30 most recent releases of any kind. At most 8 are canary and nightly builds (#392), so
/// the newest release and release candidate are among them.
pub(crate) fn releases() -> Option<Vec<Release>> {
    use ureq::tls::{RootCerts, TlsConfig};
    // The system's certificate authorities, including any a company adds.
    let tls = TlsConfig::builder()
        .root_certs(RootCerts::PlatformVerifier)
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent("parterre")
        .build()
        .into();
    let body = agent
        .get(&format!("{RELEASES}?per_page=30"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .ok()?
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_string()
        .ok()?;
    parse(&body)
}

#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

/// `releases`' list; drafts are left out.
fn parse(body: &str) -> Option<Vec<Release>> {
    let releases: Vec<GithubRelease> = serde_json::from_str(body).ok()?;
    Some(
        releases
            .into_iter()
            .filter(|r| !r.draft)
            .map(|r| Release {
                tag: r.tag_name,
                prerelease: r.prerelease,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_list() {
        let list = r#"[{"tag_name": "v0.7.0-rc1", "prerelease": true, "draft": false},
            {"tag_name": "v0.7.0", "prerelease": false, "draft": true},
            {"tag_name": "v0.6.0", "prerelease": false, "draft": false}]"#;
        let tags: Vec<_> = parse(list).unwrap().into_iter().map(|r| r.tag).collect();
        assert_eq!(tags, ["v0.7.0-rc1", "v0.6.0"]);
        assert!(parse(r#"{"message": "Not Found"}"#).is_none());
        assert!(parse("<html>").is_none());
    }
}
