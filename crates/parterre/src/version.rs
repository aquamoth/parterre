//! The version string parterre reports, e.g. `0.3.0 (a1b2c3d)` for a release.
//!
//! `build.rs` includes this file and runs [`describe`] at build time; the app only reads the
//! result (`PARTERRE_VERSION`). The app compiles this module just for its tests, so it must not
//! use anything outside `std`. It isn't in `parterre-core` because the build script would then
//! have to compile all of core as a build dependency.

/// Where the sources being built come from.
#[derive(Debug)]
pub enum Source {
    /// A git checkout whose top level is the workspace root.
    Git(GitState),
    /// A crate made by `cargo package`, such as one downloaded from crates.io. Cargo records the
    /// commit it was packaged from in `.cargo_vcs_info.json` (see [`parse_vcs_info`]).
    Package(VcsInfo),
    /// Neither, e.g. a source archive without `.git`.
    Unknown,
}

/// What `git` says about the checkout being built.
#[derive(Debug)]
pub struct GitState {
    /// Abbreviated hash of `HEAD`.
    pub commit: String,
    /// The sources differ from `HEAD` (uncommitted changes).
    pub dirty: bool,
    /// Tags pointing at `HEAD`.
    pub tags: Vec<String>,
    /// The release tag nearest `HEAD` in its history, which the version follows.
    pub nearest: Option<NearestTag>,
}

/// A tag in the history of `HEAD`, from `git describe` (see [`parse_describe`]).
#[derive(Debug, PartialEq, Eq)]
pub struct NearestTag {
    pub tag: String,
    /// Commits in `HEAD`'s history that the tag's doesn't have; 0 when it is `HEAD`.
    pub since: u32,
}

/// The commit a packaged crate was made from, as recorded in its `.cargo_vcs_info.json`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct VcsInfo {
    /// Abbreviated hash, or `None` if it was packaged outside git.
    pub commit: Option<String>,
    /// It was packaged with uncommitted changes (`cargo package --allow-dirty`).
    pub dirty: bool,
}

/// The version string for a build of package version `pkg_version`.
///
/// Versions come from the release tags. A build of exactly the released sources reports the
/// plain version and commit, `0.3.0 (a1b2c3d)`: a clean checkout of tag `v0.3.0`, or a packaged
/// crate such as the one on crates.io (whose `Cargo.toml` has the tag's version). Every other
/// build is a dev build, marked with a `dev` pre-release and the commit as build metadata.
/// Past a release tag it is a pre-release of the next version, numbered by the commits since
/// the tag: `0.3.1-dev.4+a1b2c3d(.dirty)` four commits after `v0.3.0`, or
/// `0.3.0-rc1.dev.4+a1b2c3d` after `v0.3.0-rc1`, so that it sorts after the release it follows
/// and before the next one (in dpkg and rpm too, with `-` as `~`). Only without a release tag
/// in sight does the dev version fall back to `Cargo.toml`'s: `0.3.0-dev+a1b2c3d`, or just
/// `0.3.0-dev` without git.
///
/// The release workflow sets `release_tag`. Its version comes from the tag, and the build
/// fails unless that tag points at the commit being built and the checkout is clean.
pub fn describe(
    pkg_version: &str,
    release_tag: Option<&str>,
    source: &Source,
) -> Result<String, String> {
    if let Some(tag) = release_tag {
        return release(pkg_version, tag, source);
    }
    let (commit, dirty) = match source {
        Source::Git(git) => (Some(&git.commit), git.dirty),
        Source::Package(info) => (info.commit.as_ref(), info.dirty),
        Source::Unknown => (None, false),
    };
    let nearest = match source {
        Source::Git(git) => git
            .nearest
            .as_ref()
            .and_then(|n| Some((parse_release_tag(&n.tag).ok()?, n.since))),
        _ => None,
    };
    let released = match source {
        Source::Git(_) => nearest.and_then(|(version, since)| (since == 0).then_some(version)),
        Source::Package(_) => Some(pkg_version),
        Source::Unknown => None,
    };
    if let Some(version) = released
        && !dirty
    {
        return Ok(plain(version, commit));
    }
    let dev = match nearest {
        Some((version, since)) if version.contains('-') => format!("{version}.dev.{since}"),
        Some((version, since)) => format!("{}-dev.{since}", next_patch(version)),
        None => {
            let sep = if pkg_version.contains('-') { '.' } else { '-' };
            format!("{pkg_version}{sep}dev")
        }
    };
    let Some(commit) = commit else {
        return Ok(dev);
    };
    let dirty = if dirty { ".dirty" } else { "" };
    Ok(format!("{dev}+{commit}{dirty}"))
}

/// The version of a release build for tag `tag`, which must point at the clean commit being
/// built.
fn release(pkg_version: &str, tag: &str, source: &Source) -> Result<String, String> {
    let version = parse_release_tag(tag)?;
    let (commit, dirty, tagged) = match source {
        Source::Git(git) => (
            Some(&git.commit),
            git.dirty,
            git.tags.iter().any(|t| t == tag),
        ),
        Source::Package(info) => {
            if version != pkg_version {
                return Err(format!(
                    "release tag {tag} doesn't match the packaged crate version {pkg_version}"
                ));
            }
            (info.commit.as_ref(), info.dirty, true)
        }
        Source::Unknown => (None, false, false),
    };
    let Some(commit) = commit else {
        return Err(format!(
            "release tag {tag} given, but git can't tell which commit is being built"
        ));
    };
    if !tagged {
        return Err(format!(
            "release tag {tag} doesn't point at the commit being built ({commit})"
        ));
    }
    if dirty {
        return Err(format!(
            "release {tag} is being built from a checkout with local changes"
        ));
    }
    Ok(plain(version, Some(commit)))
}

/// `0.3.0 (a1b2c3d)`, or `0.3.0` when the commit isn't known.
fn plain(version: &str, commit: Option<&String>) -> String {
    match commit {
        Some(commit) => format!("{version} ({commit})"),
        None => version.to_owned(),
    }
}

/// `0.3.1` for `0.3.0`, a version already checked by [`parse_release_tag`].
fn next_patch(version: &str) -> String {
    let (minor, patch) = version.rsplit_once('.').unwrap();
    format!("{minor}.{}", patch.parse::<u64>().unwrap() + 1)
}

/// The tag and the commits since it from `git describe --tags --long`, e.g.
/// `v0.3.0-rc1-4-ga1b2c3d`. `None` for anything else, such as a bare hash.
pub fn parse_describe(described: &str) -> Option<NearestTag> {
    let mut parts = described.rsplitn(3, '-');
    let hash = parts.next()?;
    let since = parts.next()?.parse().ok()?;
    let tag = parts.next()?;
    hash.starts_with('g').then(|| NearestTag {
        tag: tag.to_owned(),
        since,
    })
}

/// Accept `vX.Y.Z` with an optional semver pre-release suffix. Build metadata is left out of
/// release tags so the same version can later be used as a Cargo package version.
pub fn parse_release_tag(tag: &str) -> Result<&str, String> {
    let Some(version) = tag.strip_prefix('v') else {
        return Err(format!(
            "invalid release tag {tag}: expected vX.Y.Z[-prerelease]"
        ));
    };
    let (numbers, pre) = version
        .split_once('-')
        .map_or((version, None), |(n, p)| (n, Some(p)));
    let valid_number = |n: &str| {
        !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) && (n == "0" || !n.starts_with('0'))
    };
    if numbers.split('.').count() != 3
        || !numbers.split('.').all(valid_number)
        || pre.is_some_and(|p| {
            p.split('.').any(|part| {
                part.is_empty()
                    || !part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    || (part.bytes().all(|b| b.is_ascii_digit())
                        && part.len() > 1
                        && part.starts_with('0'))
            })
        })
    {
        return Err(format!(
            "invalid release tag {tag}: expected vX.Y.Z[-prerelease]"
        ));
    }
    Ok(version)
}

/// Reads the commit from the `.cargo_vcs_info.json` that `cargo package` writes, e.g.
/// `{"git": {"sha1": "a1b2c3d…", "dirty": true}, "path_in_vcs": "crates/parterre"}`.
/// `dirty` is only present when true. Hand-parsed, as the build script has no dependencies.
pub fn parse_vcs_info(json: &str) -> VcsInfo {
    let value_after = |key: &str| {
        let rest = &json[json.find(&format!("\"{key}\""))? + key.len() + 2..];
        let rest = rest.trim_start().strip_prefix(':')?.trim_start();
        Some(rest)
    };
    let commit = value_after("sha1").and_then(|rest| {
        let hex = rest.strip_prefix('"')?;
        let hex = &hex[..hex.find('"')?];
        (hex.len() >= 7 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| hex[..7].to_owned())
    });
    let dirty = value_after("dirty").is_some_and(|rest| rest.starts_with("true"));
    VcsInfo { commit, dirty }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(commit: &str, dirty: bool, tags: &[&str]) -> Source {
        Source::Git(GitState {
            commit: commit.to_owned(),
            dirty,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            nearest: None,
        })
    }

    /// A checkout `since` commits past release tag `tag`.
    fn after(tag: &str, since: u32, commit: &str, dirty: bool) -> Source {
        let tags = if since == 0 {
            vec![tag.to_owned()]
        } else {
            vec![]
        };
        Source::Git(GitState {
            commit: commit.to_owned(),
            dirty,
            tags,
            nearest: Some(NearestTag {
                tag: tag.to_owned(),
                since,
            }),
        })
    }

    #[test]
    fn dev_build_is_versioned_after_the_last_release_tag() {
        // Cargo.toml's version isn't bumped for releases; the tags are the versions. A dev build
        // comes after the release it follows and before the next one, in semver, dpkg and rpm.
        let git = after("v0.5.1", 3, "a1b2c3d", false);
        assert_eq!(
            describe("0.4.0", None, &git).unwrap(),
            "0.5.2-dev.3+a1b2c3d"
        );
    }

    #[test]
    fn dev_build_after_a_prerelease_tag_extends_its_prerelease() {
        let git = after("v0.5.0-rc1", 2, "a1b2c3d", false);
        assert_eq!(
            describe("0.4.0", None, &git).unwrap(),
            "0.5.0-rc1.dev.2+a1b2c3d"
        );
    }

    #[test]
    fn clean_checkout_of_a_tag_shows_the_tag_version() {
        let git = after("v0.5.1", 0, "a1b2c3d", false);
        assert_eq!(describe("0.4.0", None, &git).unwrap(), "0.5.1 (a1b2c3d)");
    }

    #[test]
    fn tagged_checkout_with_local_changes_is_a_dev_build_of_the_next_version() {
        let git = after("v0.5.1", 0, "a1b2c3d", true);
        assert_eq!(
            describe("0.4.0", None, &git).unwrap(),
            "0.5.2-dev.0+a1b2c3d.dirty"
        );
    }

    #[test]
    fn nearest_tag_that_isnt_a_version_is_ignored() {
        let git = after("vnext", 1, "a1b2c3d", false);
        assert_eq!(describe("0.4.0", None, &git).unwrap(), "0.4.0-dev+a1b2c3d");
    }

    #[test]
    fn git_describe_gives_tag_and_commits_since() {
        let nearest = |tag: &str, since| {
            Some(NearestTag {
                tag: tag.to_owned(),
                since,
            })
        };
        assert_eq!(parse_describe("v0.5.1-3-ga1b2c3d"), nearest("v0.5.1", 3));
        assert_eq!(
            parse_describe("v0.5.0-rc1-0-ga1b2c3d"),
            nearest("v0.5.0-rc1", 0)
        );
        assert_eq!(parse_describe("a1b2c3d"), None);
    }

    fn package(commit: Option<&str>, dirty: bool) -> Source {
        Source::Package(VcsInfo {
            commit: commit.map(str::to_owned),
            dirty,
        })
    }

    #[test]
    fn dev_build_carries_commit_as_build_metadata() {
        let git = git("a1b2c3d", false, &[]);
        assert_eq!(describe("0.3.0", None, &git).unwrap(), "0.3.0-dev+a1b2c3d");
    }

    #[test]
    fn dev_build_with_local_changes_says_dirty() {
        let git = git("a1b2c3d", true, &[]);
        assert_eq!(
            describe("0.3.0", None, &git).unwrap(),
            "0.3.0-dev+a1b2c3d.dirty"
        );
    }

    #[test]
    fn dev_build_without_git_has_no_build_metadata() {
        // E.g. built from GitHub's source archive, which has no .git directory.
        assert_eq!(
            describe("0.3.0", None, &Source::Unknown).unwrap(),
            "0.3.0-dev"
        );
    }

    #[test]
    fn dev_build_of_a_prerelease_extends_its_prerelease() {
        // Semver allows one pre-release part; "0.3.0-rc.1-dev" would be a single odd identifier.
        let git = git("a1b2c3d", false, &[]);
        assert_eq!(
            describe("0.3.0-rc.1", None, &git).unwrap(),
            "0.3.0-rc.1.dev+a1b2c3d"
        );
    }

    #[test]
    fn release_build_shows_plain_version_and_commit() {
        let git = git("a1b2c3d", false, &["v0.3.0"]);
        assert_eq!(
            describe("0.3.0", Some("v0.3.0"), &git).unwrap(),
            "0.3.0 (a1b2c3d)"
        );
    }

    #[test]
    fn release_version_comes_from_tag() {
        let git = git("a1b2c3d", false, &["v0.5.0-rc1"]);
        assert_eq!(
            describe("0.4.0", Some("v0.5.0-rc1"), &git).unwrap(),
            "0.5.0-rc1 (a1b2c3d)"
        );
    }

    #[test]
    fn release_tag_must_be_a_version() {
        let git = git("a1b2c3d", false, &["vnot-a-version"]);
        assert!(describe("0.4.0", Some("vnot-a-version"), &git).is_err());
        assert!(describe("0.4.0", Some("v0.5.0-"), &git).is_err());
        assert!(describe("0.4.0", Some("v0.5.0-01"), &git).is_err());
    }

    #[test]
    fn release_tag_must_point_at_the_built_commit() {
        let git = git("a1b2c3d", false, &["v0.2.0"]);
        let err = describe("0.3.0", Some("v0.3.0"), &git).unwrap_err();
        assert!(err.contains("v0.3.0") && err.contains("a1b2c3d"), "{err}");
    }

    #[test]
    fn release_build_must_be_clean() {
        let git = git("a1b2c3d", true, &["v0.3.0"]);
        let err = describe("0.3.0", Some("v0.3.0"), &git).unwrap_err();
        assert!(err.contains("changes"), "{err}");
    }

    #[test]
    fn release_build_needs_git() {
        let err = describe("0.3.0", Some("v0.3.0"), &Source::Unknown).unwrap_err();
        assert!(err.contains("git"), "{err}");
    }

    #[test]
    fn packaged_crate_shows_plain_version() {
        // What `cargo install parterre` builds from crates.io.
        let pkg = package(Some("a1b2c3d"), false);
        assert_eq!(describe("0.3.0", None, &pkg).unwrap(), "0.3.0 (a1b2c3d)");
        let pkg = package(None, false);
        assert_eq!(describe("0.3.0", None, &pkg).unwrap(), "0.3.0");
    }

    #[test]
    fn crate_packaged_with_local_changes_is_a_dev_build() {
        let pkg = package(Some("a1b2c3d"), true);
        assert_eq!(
            describe("0.3.0", None, &pkg).unwrap(),
            "0.3.0-dev+a1b2c3d.dirty"
        );
    }

    #[test]
    fn release_workflow_may_build_the_packaged_crate() {
        // `cargo publish` in the release workflow compiles the package it made.
        let pkg = package(Some("a1b2c3d"), false);
        assert_eq!(
            describe("0.3.0", Some("v0.3.0"), &pkg).unwrap(),
            "0.3.0 (a1b2c3d)"
        );
        let err = describe("0.3.0", Some("v0.3.1"), &pkg).unwrap_err();
        assert!(err.contains("v0.3.1"), "{err}");
    }

    #[test]
    fn vcs_info_gives_abbreviated_commit_and_dirty_flag() {
        let clean = r#"{
  "git": {
    "sha1": "e6f2c0c0a1b2c3d4e5f60718293a4b5c6d7e8f90"
  },
  "path_in_vcs": "crates/parterre"
}"#;
        assert_eq!(
            parse_vcs_info(clean),
            VcsInfo {
                commit: Some("e6f2c0c".into()),
                dirty: false
            }
        );
        let dirty = r#"{"git":{"sha1":"e6f2c0c0a1b2c3d4e5f60718293a4b5c6d7e8f90","dirty":true},"path_in_vcs":""}"#;
        assert_eq!(
            parse_vcs_info(dirty),
            VcsInfo {
                commit: Some("e6f2c0c".into()),
                dirty: true
            }
        );
        assert_eq!(parse_vcs_info(r#"{"path_in_vcs": ""}"#), VcsInfo::default());
        assert_eq!(parse_vcs_info("not json"), VcsInfo::default());
    }
}
