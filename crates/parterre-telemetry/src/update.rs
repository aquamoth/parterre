//! Whether a release is newer than this build, and where this build's channel gets it. No
//! network here, so all of it is tested without one.

use std::cmp::Ordering;

use crate::Channel;

/// Where the releases are.
const REPOSITORY: &str = "https://github.com/aquamoth/parterre";

/// What *Download* copies on the cargo channel.
pub const CARGO_INSTALL: &str = "cargo install --locked parterre";

/// A version as parterre's tags and its `--version` write it: `v0.6.0`, `0.6.0-rc1`,
/// `0.6.0 (a1b2c3d)` or `0.6.1-dev.3+a1b2c3d.dirty`. The commit and build metadata are left
/// out; they don't order versions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    numbers: [u64; 3],
    /// The pre-release's dot-separated identifiers: none for a release.
    pre: Vec<String>,
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text.split([' ', '+']).next()?;
        let (numbers, pre) = match text.split_once('-') {
            Some((numbers, pre)) => (numbers, Some(pre)),
            None => (text, None),
        };
        let mut parts = numbers.split('.').map(|n| {
            n.bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| n.parse().ok())
                .flatten()
        });
        let numbers = [parts.next()??, parts.next()??, parts.next()??];
        if parts.next().is_some() {
            return None;
        }
        let pre: Vec<String> =
            pre.map_or_else(Vec::new, |p| p.split('.').map(str::to_owned).collect());
        let identifier = |id: &String| {
            !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        };
        pre.iter()
            .all(identifier)
            .then_some(Version { numbers, pre })
    }

    /// A release candidate, or a dev build: it hears of pre-releases too.
    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [major, minor, patch] = self.numbers;
        write!(f, "{major}.{minor}.{patch}")?;
        if self.is_prerelease() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

impl Ord for Version {
    /// Semver's order, except that identifiers mixing letters and digits compare their digits
    /// as numbers: `rc2` before `rc10`, as dpkg and rpm sort them.
    fn cmp(&self, other: &Version) -> Ordering {
        self.numbers.cmp(&other.numbers).then_with(|| {
            match (self.is_prerelease(), other.is_prerelease()) {
                (false, false) => Ordering::Equal,
                (false, true) => Ordering::Greater,
                (true, false) => Ordering::Less,
                (true, true) => {
                    let pairs = self.pre.iter().zip(&other.pre);
                    pairs
                        .map(|(a, b)| identifier_order(a, b))
                        .find(|o| o.is_ne())
                        .unwrap_or_else(|| self.pre.len().cmp(&other.pre.len()))
                }
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Numbers before words, as in semver; words by their runs of letters and of digits.
fn identifier_order(a: &str, b: &str) -> Ordering {
    let number = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    match (number(a), number(b)) {
        (true, true) => runs_order(a, b),
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => {
            let (mut a, mut b) = (runs(a), runs(b));
            loop {
                match (a.next(), b.next()) {
                    (Some(x), Some(y)) => match runs_order(x, y) {
                        Ordering::Equal => {}
                        o => return o,
                    },
                    (x, y) => return x.is_some().cmp(&y.is_some()),
                }
            }
        }
    }
}

/// Two runs: digits by their value, anything else by its bytes.
fn runs_order(a: &str, b: &str) -> Ordering {
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if digits(a) && digits(b) {
        let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
        a.len().cmp(&b.len()).then_with(|| a.cmp(b))
    } else {
        a.cmp(b)
    }
}

/// `rc10` as `rc`, `10`.
fn runs(s: &str) -> impl Iterator<Item = &str> {
    let mut rest = s;
    std::iter::from_fn(move || {
        let first = rest.bytes().next()?;
        let end = rest
            .bytes()
            .position(|b| b.is_ascii_digit() != first.is_ascii_digit())
            .unwrap_or(rest.len());
        let (run, after) = rest.split_at(end);
        rest = after;
        Some(run)
    })
}

/// A published release, as GitHub lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// `v0.6.0`.
    pub tag: String,
    pub prerelease: bool,
}

/// The release among `releases` that a build of version `current` hears of, if it is newer
/// than `current`: the newest release for a release build, and the newest of any kind for a
/// release candidate or a dev build, so that it hears of the next candidate and the final.
/// Tags that aren't versions are passed over.
pub fn newer<'a>(current: &Version, releases: &'a [Release]) -> Option<&'a Release> {
    releases
        .iter()
        .filter(|r| current.is_prerelease() || !r.prerelease)
        .filter_map(|r| Some((Version::parse(&r.tag)?, r)))
        .filter(|(v, _)| current.is_prerelease() || !v.is_prerelease())
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .filter(|(v, _)| v > current)
        .map(|(_, r)| r)
}

/// A newer release, and what *Download* does for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    /// `0.6.0`.
    pub version: String,
    pub download: Download,
}

/// What *Download* does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Download {
    /// Open this file of the release in the browser.
    Open(String),
    /// Copy this command to the clipboard.
    Copy(&'static str),
}

impl Update {
    /// Release `tag` (`v0.6.0`, or `0.6.0`) for `channel` on `target`, the Rust target triple
    /// the release files are named after. `None` on Snap and Flatpak, whose stores update
    /// parterre, and for a tag that isn't a version.
    pub fn of(tag: &str, channel: Channel, target: &str) -> Option<Update> {
        let version = Version::parse(tag)?.to_string();
        let arch = target.split('-').next().unwrap_or(target);
        let file = match channel {
            Channel::Msi => format!("parterre-{version}-{target}.msi"),
            Channel::Zip => format!("parterre-{version}-{target}.zip"),
            Channel::Tarball => format!("parterre-{version}-{target}.tar.gz"),
            Channel::Dmg => format!("parterre-{version}-{target}.dmg"),
            Channel::Deb => {
                let arch = match arch {
                    "x86_64" => "amd64",
                    "aarch64" => "arm64",
                    other => other,
                };
                format!("parterre_{version}_{arch}.deb")
            }
            Channel::Rpm => format!("parterre-{version}-1.{arch}.rpm"),
            Channel::Cargo => {
                return Some(Update {
                    version,
                    download: Download::Copy(CARGO_INSTALL),
                });
            }
            Channel::Snap | Channel::Flatpak => return None,
        };
        let url = format!("{REPOSITORY}/releases/download/v{version}/{file}");
        Some(Update {
            version,
            download: Download::Open(url),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|| panic!("{text} should parse"))
    }

    fn release(tag: &str) -> Release {
        Release {
            tag: tag.to_owned(),
            prerelease: tag.contains('-'),
        }
    }

    fn newer_tag(current: &str, tags: &[&str]) -> Option<String> {
        let releases: Vec<_> = tags.iter().map(|t| release(t)).collect();
        newer(&v(current), &releases).map(|r| r.tag.clone())
    }

    #[test]
    fn versions_as_tags_and_builds_write_them() {
        assert_eq!(v("v0.6.0").to_string(), "0.6.0");
        assert_eq!(v("0.6.0 (a1b2c3d)").to_string(), "0.6.0");
        assert_eq!(v("0.6.0-rc1 (a1b2c3d)").to_string(), "0.6.0-rc1");
        assert_eq!(v("0.6.1-dev.3+a1b2c3d.dirty").to_string(), "0.6.1-dev.3");
        assert!(!v("0.6.0 (a1b2c3d)").is_prerelease());
        assert!(v("0.6.0-rc1").is_prerelease());
        assert!(v("0.6.1-dev.3+a1b2c3d").is_prerelease());
    }

    #[test]
    fn malformed_versions_are_none() {
        for text in [
            "",
            "v",
            "next",
            "vnext",
            "0.6",
            "0.6.0.1",
            "0.6.x",
            "0..6",
            "-0.6.0",
            "0.6.0-",
            "0.6.0-rc..1",
            "0.6.0-rc_1",
            "99999999999999999999.0.0",
        ] {
            assert_eq!(Version::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn versions_order_as_releases_follow_each_other() {
        let order = [
            "0.5.1",
            "0.6.0-dev.3",
            "0.6.0-rc1",
            "0.6.0-rc1.dev.2",
            "0.6.0-rc2",
            "0.6.0-rc10",
            "0.6.0",
            "0.6.1-dev.1",
            "0.6.1-dev.12",
            "0.10.0",
            "1.0.0",
        ];
        for pair in order.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        }
        assert_eq!(v("0.6.0").cmp(&v("v0.6.0 (a1b2c3d)")), Ordering::Equal);
        assert!(v("1.0.0-1") < v("1.0.0-alpha"));
    }

    #[test]
    fn a_release_build_hears_of_newer_releases_only() {
        assert_eq!(
            newer_tag("0.5.1 (a1b2c3d)", &["v0.6.0"]),
            Some("v0.6.0".into())
        );
        assert_eq!(newer_tag("0.6.0 (a1b2c3d)", &["v0.6.0"]), None);
        assert_eq!(newer_tag("0.6.1 (a1b2c3d)", &["v0.6.0"]), None);
        assert_eq!(newer_tag("0.6.0", &["v0.7.0-rc1", "v0.6.0"]), None);
    }

    #[test]
    fn a_release_candidate_hears_of_the_next_candidate_and_the_final() {
        let tags = ["v0.6.0-rc2", "v0.6.0-rc1", "v0.5.1"];
        assert_eq!(
            newer_tag("0.6.0-rc1 (a1b2c3d)", &tags),
            Some("v0.6.0-rc2".into())
        );
        assert_eq!(newer_tag("0.6.0-rc2 (a1b2c3d)", &tags), None);
        let tags = ["v0.6.0", "v0.6.0-rc2", "v0.6.0-rc1"];
        assert_eq!(newer_tag("0.6.0-rc2", &tags), Some("v0.6.0".into()));
        // Listed newest first by date, which needn't be by version.
        let tags = ["v0.5.2", "v0.6.0-rc10", "v0.6.0-rc9"];
        assert_eq!(newer_tag("0.6.0-rc9", &tags), Some("v0.6.0-rc10".into()));
    }

    #[test]
    fn a_dev_build_hears_of_what_follows_its_release() {
        let tags = ["v0.6.1-rc1", "v0.6.0"];
        assert_eq!(
            newer_tag("0.6.1-dev.4+a1b2c3d", &tags),
            Some("v0.6.1-rc1".into())
        );
        assert_eq!(newer_tag("0.6.1-dev.4+a1b2c3d", &["v0.6.0"]), None);
    }

    #[test]
    fn tags_that_are_not_versions_are_passed_over() {
        let tags = ["nightly", "v0.7", "v0.6.0"];
        assert_eq!(newer_tag("0.5.1", &tags), Some("v0.6.0".into()));
        assert_eq!(newer_tag("0.5.1", &["nightly"]), None);
        assert_eq!(newer_tag("0.5.1", &[]), None);
    }

    #[test]
    fn download_opens_the_channels_own_file() {
        let url = |channel, target| match Update::of("v0.6.0-rc1", channel, target) {
            Some(Update {
                version,
                download: Download::Open(url),
            }) => {
                assert_eq!(version, "0.6.0-rc1");
                url
            }
            other => panic!("{other:?}"),
        };
        let release = "https://github.com/aquamoth/parterre/releases/download/v0.6.0-rc1";
        // As in the release v0.6.0-rc1, and the .dmg as the release workflow names it since.
        for (channel, target, file) in [
            (
                Channel::Msi,
                "x86_64-pc-windows-msvc",
                "parterre-0.6.0-rc1-x86_64-pc-windows-msvc.msi",
            ),
            (
                Channel::Zip,
                "x86_64-pc-windows-msvc",
                "parterre-0.6.0-rc1-x86_64-pc-windows-msvc.zip",
            ),
            (
                Channel::Tarball,
                "x86_64-unknown-linux-gnu",
                "parterre-0.6.0-rc1-x86_64-unknown-linux-gnu.tar.gz",
            ),
            (
                Channel::Tarball,
                "aarch64-apple-darwin",
                "parterre-0.6.0-rc1-aarch64-apple-darwin.tar.gz",
            ),
            (
                Channel::Dmg,
                "x86_64-apple-darwin",
                "parterre-0.6.0-rc1-x86_64-apple-darwin.dmg",
            ),
            (
                Channel::Deb,
                "x86_64-unknown-linux-gnu",
                "parterre_0.6.0-rc1_amd64.deb",
            ),
            (
                Channel::Rpm,
                "x86_64-unknown-linux-gnu",
                "parterre-0.6.0-rc1-1.x86_64.rpm",
            ),
        ] {
            assert_eq!(url(channel, target), format!("{release}/{file}"));
        }
    }

    #[test]
    fn download_copies_the_command_on_the_cargo_channel() {
        let update = Update::of("0.6.0", Channel::Cargo, "x86_64-unknown-linux-gnu").unwrap();
        assert_eq!(update.version, "0.6.0");
        assert_eq!(update.download, Download::Copy(CARGO_INSTALL));
    }

    #[test]
    fn snap_and_flatpak_are_offered_nothing() {
        for channel in [Channel::Snap, Channel::Flatpak] {
            assert_eq!(
                Update::of("v0.6.0", channel, "x86_64-unknown-linux-gnu"),
                None
            );
        }
        assert_eq!(
            Update::of("next", Channel::Deb, "x86_64-unknown-linux-gnu"),
            None
        );
    }
}
