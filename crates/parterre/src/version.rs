//! The version string a build of parterre reports, e.g. `1.2.3-dev+a1b2c3d` for a dev build.
//!
//! `build.rs` includes this file and runs [`describe`]; so does `parterre-stamp`, which the
//! pipeline runs for the version it writes into a published build ([`published`]). The app
//! compiles this module for its tests only, so it must not use anything outside `std`. It isn't
//! in `parterre-core` because the build script would then have to compile all of core as a
//! build dependency.
//!
//! The maintainer chooses major and minor by tagging `vX.Y-rcN` or `vX.Y`; the patch is counted:
//! the commits on the first-parent line since the oldest tag of that minor (#397). So every
//! commit on `main` has a patch of its own, and the builds of one commit are told apart by the
//! pre-release word naming the ring: `1.2.1-canary`, `1.2.1-nightly.20261016`, `1.2.1-rc`,
//! `1.2.1`, and `1.2.1-dev` for a build nobody published.

#[derive(Debug)]
pub enum Source {
    /// A checkout whose top level is the workspace root.
    Git(GitState),
    /// A crate made by `cargo package`, such as one downloaded from crates.io.
    Package(VcsInfo),
    /// Neither, e.g. a source archive without `.git`.
    Unknown,
}

#[derive(Debug)]
pub struct GitState {
    pub short_hash: String,
    pub dirty: bool,
    /// `None` without a version tag in its history, as in a shallow clone.
    pub base: Option<Base>,
}

/// Where a commit stands: its minor, and its patch within it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Base {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

/// The commit a packaged crate was made from, as its `.cargo_vcs_info.json` records it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct VcsInfo {
    /// `None` if it was packaged outside git.
    pub short_hash: Option<String>,
    pub dirty: bool,
}

/// What a build is published as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ring {
    Canary,
    /// With the date it was built on, `20261016`.
    Nightly(u32),
    Rc,
    Stable,
}

/// The version of a build that hasn't been published: `1.2.3-dev+a1b2c3d`, with `.dirty` when
/// the sources have local changes. A packaged crate, such as the one on crates.io (whose
/// `Cargo.toml` has the release's version), says `1.2.3+a1b2c3d`. Without a version tag in
/// sight the version falls back to `Cargo.toml`'s, `0.0.0-dev+a1b2c3d`, or just `0.0.0-dev`
/// without git.
pub fn describe(pkg_version: &str, source: &Source) -> String {
    let (short_hash, dirty) = match source {
        Source::Git(git) => (Some(git.short_hash.as_str()), git.dirty),
        Source::Package(info) => (info.short_hash.as_deref(), info.dirty),
        Source::Unknown => (None, false),
    };
    let version = match source {
        Source::Git(GitState {
            base: Some(base), ..
        }) => format!("{}-dev", numbers(*base)),
        Source::Package(_) if !dirty => pkg_version.to_owned(),
        _ => {
            let sep = if pkg_version.contains('-') { '.' } else { '-' };
            format!("{pkg_version}{sep}dev")
        }
    };
    let Some(short_hash) = short_hash else {
        return version;
    };
    let dirty = if dirty { ".dirty" } else { "" };
    format!("{version}+{short_hash}{dirty}")
}

/// The version a build of the commit at `base` is published under in `ring`:
/// `1.2.1-canary+a1b2c3d`, `1.2.1-nightly.20261016+a1b2c3d`, `1.2.1-rc+a1b2c3d` or
/// `1.2.1+a1b2c3d`. Its release's tag is the same without the commit, after a `v`.
pub fn published(base: Base, ring: Ring, short_hash: &str) -> String {
    let pre = match ring {
        Ring::Canary => "-canary".to_owned(),
        Ring::Nightly(date) => format!("-nightly.{date}"),
        Ring::Rc => "-rc".to_owned(),
        Ring::Stable => String::new(),
    };
    format!("{}{pre}+{short_hash}", numbers(base))
}

fn numbers(base: Base) -> String {
    let Base {
        major,
        minor,
        patch,
    } = base;
    format!("{major}.{minor}.{patch}")
}

/// The minor of a tag the maintainer pushed: `v1.2` and `v1.2-rc3`, or one made before the
/// patch was counted, such as `v1.0.0` and `v1.0.0-rc1`. `None` for the tags the pipeline
/// makes for each ring, `v1.2.1-canary`, `v1.2.1-nightly.20261016` and `v1.2.1-rc`, so that no
/// build is described from them. (A stable's own `v1.2.1` is taken, but its minor already has
/// the maintainer's older `v1.2` tag.)
pub fn tag_minor(tag: &str) -> Option<(u64, u64)> {
    let version = tag.strip_prefix('v')?;
    let (numbers, pre) = version
        .split_once('-')
        .map_or((version, None), |(n, p)| (n, Some(p)));
    if let Some(pre) = pre {
        let n = pre.strip_prefix("rc")?;
        if n.is_empty() || !is_number(n) {
            return None;
        }
    }
    let parts: Vec<&str> = numbers.split('.').collect();
    if !(2..=3).contains(&parts.len()) || !parts.iter().all(|p| is_number(p)) {
        return None;
    }
    Some((parts[0].parse().ok()?, parts[1].parse().ok()?))
}

fn is_number(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && !(s.len() > 1 && s.starts_with('0'))
}

/// The [`Base`] of `rev`, asking `git` (which runs git with the arguments given and returns its
/// trimmed output, `None` if it failed). The minor is the highest of the maintainer's tags in
/// its history ([`tag_minor`]); the patch counts the first-parent commits since the oldest tag
/// of that minor, i.e. the most since any of them.
pub fn base(git: impl Fn(&[&str]) -> Option<String>, rev: &str) -> Option<Base> {
    let tags = git(&["tag", "--merged", rev, "--list", "v[0-9]*"])?;
    let tagged: Vec<(&str, (u64, u64))> = tags
        .lines()
        .filter_map(|t| Some((t, tag_minor(t)?)))
        .collect();
    let (major, minor) = tagged.iter().map(|(_, m)| *m).max()?;
    let patch = tagged
        .iter()
        .filter(|(_, m)| *m == (major, minor))
        .map(|(tag, _)| {
            let range = format!("refs/tags/{tag}..{rev}");
            git(&["rev-list", "--count", "--first-parent", &range])?
                .parse::<u64>()
                .ok()
        })
        .collect::<Option<Vec<u64>>>()?
        .into_iter()
        .max()?;
    Some(Base {
        major,
        minor,
        patch,
    })
}

/// Reads `.cargo_vcs_info.json`, e.g.
/// `{"git": {"sha1": "a1b2c3d…", "dirty": true}, "path_in_vcs": "crates/parterre"}`, where
/// `dirty` is only present when true. Hand-parsed, as the build script has no dependencies.
pub fn parse_vcs_info(json: &str) -> VcsInfo {
    let value_after = |key: &str| {
        let rest = &json[json.find(&format!("\"{key}\""))? + key.len() + 2..];
        let rest = rest.trim_start().strip_prefix(':')?.trim_start();
        Some(rest)
    };
    let short_hash = value_after("sha1").and_then(|rest| {
        let hex = rest.strip_prefix('"')?;
        let hex = &hex[..hex.find('"')?];
        (hex.len() >= 7 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| hex[..7].to_owned())
    });
    let dirty = value_after("dirty").is_some_and(|rest| rest.starts_with("true"));
    VcsInfo { short_hash, dirty }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: Base = Base {
        major: 1,
        minor: 2,
        patch: 3,
    };

    fn git(base: Option<Base>, dirty: bool) -> Source {
        Source::Git(GitState {
            short_hash: "a1b2c3d".to_owned(),
            dirty,
            base,
        })
    }

    fn package(commit: Option<&str>, dirty: bool) -> Source {
        Source::Package(VcsInfo {
            short_hash: commit.map(str::to_owned),
            dirty,
        })
    }

    #[test]
    fn a_dev_build_has_its_commits_patch_and_the_commit() {
        assert_eq!(
            describe("0.0.0", &git(Some(BASE), false)),
            "1.2.3-dev+a1b2c3d"
        );
        assert_eq!(
            describe("0.0.0", &git(Some(BASE), true)),
            "1.2.3-dev+a1b2c3d.dirty"
        );
    }

    #[test]
    fn without_a_tag_in_sight_the_version_is_cargo_tomls() {
        assert_eq!(describe("0.0.0", &git(None, false)), "0.0.0-dev+a1b2c3d");
        // E.g. built from GitHub's source archive, which has no .git directory.
        assert_eq!(describe("0.0.0", &Source::Unknown), "0.0.0-dev");
        // Semver allows one pre-release part.
        assert_eq!(
            describe("0.3.0-rc.1", &git(None, false)),
            "0.3.0-rc.1.dev+a1b2c3d"
        );
    }

    #[test]
    fn a_packaged_crate_has_its_own_version() {
        // What `cargo install parterre` builds from crates.io.
        assert_eq!(
            describe("1.2.3", &package(Some("a1b2c3d"), false)),
            "1.2.3+a1b2c3d"
        );
        assert_eq!(describe("1.2.3", &package(None, false)), "1.2.3");
        assert_eq!(
            describe("1.2.3", &package(Some("a1b2c3d"), true)),
            "1.2.3-dev+a1b2c3d.dirty"
        );
    }

    #[test]
    fn each_ring_has_a_word_of_its_own() {
        let v = |ring| published(BASE, ring, "a1b2c3d");
        assert_eq!(v(Ring::Canary), "1.2.3-canary+a1b2c3d");
        assert_eq!(v(Ring::Nightly(20261016)), "1.2.3-nightly.20261016+a1b2c3d");
        assert_eq!(v(Ring::Rc), "1.2.3-rc+a1b2c3d");
        assert_eq!(v(Ring::Stable), "1.2.3+a1b2c3d");
    }

    #[test]
    fn only_the_maintainers_tags_have_a_minor() {
        for (tag, minor) in [
            ("v1.2", Some((1, 2))),
            ("v1.2-rc3", Some((1, 2))),
            ("v1.0.0", Some((1, 0))),
            ("v1.0.0-rc1", Some((1, 0))),
            ("v0.10.4", Some((0, 10))),
            ("v1.2.1-canary", None),
            ("v1.2.1-nightly.20261016", None),
            ("v1.2.1-rc", None),
            ("v1.2-rc", None),
            ("v1", None),
            ("v1.2.3.4", None),
            ("v01.2", None),
            ("vnext", None),
            ("1.2", None),
        ] {
            assert_eq!(tag_minor(tag), minor, "{tag}");
        }
    }

    /// Answers `tag --merged` with `tags`, and `rev-list --count` with `counts` per tag.
    fn fake_git<'a>(
        tags: &'a [&str],
        counts: &'a [(&str, u64)],
    ) -> impl Fn(&[&str]) -> Option<String> + 'a {
        move |args| match args {
            ["tag", "--merged", "HEAD", ..] => Some(tags.join("\n")),
            ["rev-list", "--count", "--first-parent", range] => {
                let tag = range.strip_prefix("refs/tags/")?.strip_suffix("..HEAD")?;
                let (_, n) = counts.iter().find(|(t, _)| *t == tag)?;
                Some(n.to_string())
            }
            _ => None,
        }
    }

    #[test]
    fn the_patch_counts_from_the_oldest_tag_of_the_highest_minor() {
        let tags = [
            "v1.0.0-rc1",
            "v1.0.0",
            "v1.1-rc1",
            "v1.1-rc2",
            "v1.1.4-rc",
            "v1.1.9-canary",
        ];
        let counts = [("v1.1-rc1", 12), ("v1.1-rc2", 8), ("v1.0.0", 40)];
        assert_eq!(
            base(fake_git(&tags, &counts), "HEAD"),
            Some(Base {
                major: 1,
                minor: 1,
                patch: 12
            })
        );
        let tags = ["v0.9.0", "v1.0.0-rc1", "v1.0.0"];
        let counts = [("v1.0.0-rc1", 41), ("v1.0.0", 30)];
        assert_eq!(
            base(fake_git(&tags, &counts), "HEAD"),
            Some(Base {
                major: 1,
                minor: 0,
                patch: 41
            })
        );
    }

    #[test]
    fn no_maintainer_tag_no_base() {
        assert_eq!(base(fake_git(&[], &[]), "HEAD"), None);
        assert_eq!(base(fake_git(&["v1.1.9-canary"], &[]), "HEAD"), None);
        assert_eq!(base(|_: &[&str]| None, "HEAD"), None);
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
                short_hash: Some("e6f2c0c".into()),
                dirty: false
            }
        );
        let dirty = r#"{"git":{"sha1":"e6f2c0c0a1b2c3d4e5f60718293a4b5c6d7e8f90","dirty":true},"path_in_vcs":""}"#;
        assert_eq!(
            parse_vcs_info(dirty),
            VcsInfo {
                short_hash: Some("e6f2c0c".into()),
                dirty: true
            }
        );
        assert_eq!(parse_vcs_info(r#"{"path_in_vcs": ""}"#), VcsInfo::default());
        assert_eq!(parse_vcs_info("not json"), VcsInfo::default());
    }
}
