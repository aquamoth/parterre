//! GitHub's releases API over HTTPS, with ureq. Asked without signing in, and with nothing of
//! parterre's own: no version, no ID, only the user agent GitHub requires.

use std::time::Duration;

use crate::Release;

const RELEASES: &str = "https://api.github.com/repos/aquamoth/parterre/releases";

/// The latest release, or with `prereleases` the most recent releases of any kind (GitHub
/// leaves pre-releases out of the latest).
pub(crate) fn releases(prereleases: bool) -> Result<Vec<Release>, String> {
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
    let url = if prereleases {
        format!("{RELEASES}?per_page=30")
    } else {
        format!("{RELEASES}/latest")
    };
    let body = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    parse(&body, prereleases)
}

#[derive(serde::Deserialize)]
struct Json {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

/// A release (`releases/latest`) or a list of them (`releases`); drafts are left out.
fn parse(body: &str, list: bool) -> Result<Vec<Release>, String> {
    let json: Vec<Json> = if list {
        serde_json::from_str(body)
    } else {
        serde_json::from_str(body).map(|one| vec![one])
    }
    .map_err(|e| e.to_string())?;
    Ok(json
        .into_iter()
        .filter(|r| !r.draft)
        .map(|r| Release {
            tag: r.tag_name,
            prerelease: r.prerelease,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_latest_release_and_the_list() {
        let latest = r#"{"tag_name": "v0.6.0", "name": "parterre 0.6.0", "prerelease": false,
            "draft": false, "assets": []}"#;
        assert_eq!(
            parse(latest, false).unwrap(),
            [Release {
                tag: "v0.6.0".into(),
                prerelease: false
            }]
        );
        let list = r#"[{"tag_name": "v0.7.0-rc1", "prerelease": true, "draft": false},
            {"tag_name": "v0.7.0", "prerelease": false, "draft": true},
            {"tag_name": "v0.6.0", "prerelease": false, "draft": false}]"#;
        let tags: Vec<_> = parse(list, true)
            .unwrap()
            .into_iter()
            .map(|r| r.tag)
            .collect();
        assert_eq!(tags, ["v0.7.0-rc1", "v0.6.0"]);
        assert!(parse(r#"{"message": "Not Found"}"#, false).is_err());
        assert!(parse("<html>", true).is_err());
    }
}
