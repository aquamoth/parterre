use std::cmp::Ordering;

use crate::{Channel, Ring};

const REPOSITORY_URL: &str = "https://github.com/aquamoth/parterre";

/// The Rust target triple the release files are named after.
const TARGET: &str = env!("PARTERRE_TARGET");

pub const CARGO_INSTALL: &str = "cargo install --locked parterre";

/// A version as parterre's tags and its `--version` write it: `v1.2.3`, `1.2.3-rc+a1b2c3d`,
/// `1.2.3-dev+a1b2c3d.dirty`, or older ones' `0.6.0 (a1b2c3d)`. The commit and build metadata
/// are left out: they don't order versions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    numbers: [u64; 3],
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
        let mut parts = numbers
            .split('.')
            .map(|n| is_digits(n).then(|| n.parse().ok()).flatten());
        let numbers = [parts.next()??, parts.next()??, parts.next()??];
        if parts.next().is_some() {
            return None;
        }
        let pre: Vec<String> =
            pre.map_or_else(Vec::new, |p| p.split('.').map(str::to_owned).collect());
        let is_identifier = |id: &String| {
            !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        };
        pre.iter()
            .all(is_identifier)
            .then_some(Version { numbers, pre })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }

    /// The pre-release identifiers: `["nightly", "20261016"]`.
    pub fn pre(&self) -> &[String] {
        &self.pre
    }

    /// `[1, 2, 3]`: the commit, within a minor (#397).
    pub fn numbers(&self) -> [u64; 3] {
        self.numbers
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
    match (is_digits(a), is_digits(b)) {
        (true, true) => runs_order(a, b),
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => runs(a)
            .zip(runs(b))
            .map(|(x, y)| runs_order(x, y))
            .find(|o| o.is_ne())
            .unwrap_or_else(|| runs(a).count().cmp(&runs(b).count())),
    }
}

/// Digits by their value, however many, and anything else by its bytes.
fn runs_order(a: &str, b: &str) -> Ordering {
    if is_digits(a) && is_digits(b) {
        let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
        a.len().cmp(&b.len()).then_with(|| a.cmp(b))
    } else {
        a.cmp(b)
    }
}

fn is_digits(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit())
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub prerelease: bool,
}

/// The newest of `releases` that `ring` hears of ([`Ring::hears_of`]), if it is of a later
/// commit than `current`: of higher numbers, so never a build of the commit running, as a
/// release of the release candidate running. Tags that aren't versions are passed over.
pub fn newer<'a>(current: &Version, ring: Ring, releases: &'a [Release]) -> Option<&'a Release> {
    releases
        .iter()
        .filter_map(|r| Some((Version::parse(&r.tag)?, r)))
        .filter(|(v, _)| Ring::of(v).is_some_and(|of| ring.hears_of(of)))
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .filter(|(v, _)| v.numbers > current.numbers)
        .map(|(_, r)| r)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    /// Without the `v`: `1.2.3-nightly.20261016`.
    pub version: String,
    /// The ring of the build: what the dialog calls it.
    pub ring: Ring,
    pub download: Download,
}

/// What the *Download* button does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Download {
    /// Opens this release file's URL in the browser.
    Open(String),
    /// Copies this command to the clipboard.
    Copy(&'static str),
}

impl Update {
    /// Release `tag` (`v0.6.0`, or `0.6.0`) for this copy's channel.
    pub fn to(tag: &str) -> Option<Update> {
        Update::of(tag, Channel::current(), TARGET)
    }

    pub fn notes(&self) -> String {
        format!("{REPOSITORY_URL}/releases/tag/v{}", self.version)
    }

    /// `None` on Snap and Flatpak, whose stores update parterre, and for a tag that isn't the
    /// version of a ring's build.
    pub fn of(tag: &str, channel: Channel, target: &str) -> Option<Update> {
        let parsed = Version::parse(tag)?;
        let ring = Ring::of(&parsed)?;
        let version = parsed.to_string();
        let arch = target.split('-').next().unwrap_or(target);
        let file = match channel {
            Channel::Msi | Channel::MsiMachine => format!("parterre-{version}-{target}.msi"),
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
                    ring,
                    download: Download::Copy(CARGO_INSTALL),
                });
            }
            Channel::Snap | Channel::Flatpak => return None,
        };
        let url = format!("{REPOSITORY_URL}/releases/download/v{version}/{file}");
        Some(Update {
            version,
            ring,
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

    fn newer_tag(current: &str, ring: Ring, tags: &[&str]) -> Option<String> {
        let releases: Vec<_> = tags.iter().map(|t| release(t)).collect();
        newer(&v(current), ring, &releases).map(|r| r.tag.clone())
    }

    #[test]
    fn versions_as_tags_and_builds_write_them() {
        assert_eq!(v("v1.2.3").to_string(), "1.2.3");
        assert_eq!(v("1.2.3+a1b2c3d").to_string(), "1.2.3");
        assert_eq!(
            v("1.2.3-nightly.20261016+a1b2c3d").to_string(),
            "1.2.3-nightly.20261016"
        );
        assert_eq!(v("0.6.0 (a1b2c3d)").to_string(), "0.6.0");
        assert_eq!(v("0.6.0-rc1 (a1b2c3d)").to_string(), "0.6.0-rc1");
        assert_eq!(v("1.2.3-dev+a1b2c3d.dirty").to_string(), "1.2.3-dev");
        assert!(!v("1.2.3+a1b2c3d").is_prerelease());
        assert!(v("1.2.3-rc").is_prerelease());
        assert_eq!(v("1.2.3-nightly.20261016").pre(), ["nightly", "20261016"]);
        assert_eq!(v("1.2.3-canary").numbers(), [1, 2, 3]);
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
    fn versions_order_as_builds_follow_each_other() {
        let order = [
            "0.5.1",
            "0.6.0-rc1",
            "0.6.0-rc2",
            "0.6.0-rc10",
            "0.6.0",
            "1.2.3-canary",
            "1.2.3-dev",
            "1.2.3-nightly.20261016",
            "1.2.3-nightly.20261017",
            "1.2.3-rc",
            "1.2.3",
            "1.2.4-canary",
            "1.10.0-rc",
        ];
        for pair in order.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        }
        assert_eq!(v("1.2.3").cmp(&v("v1.2.3+a1b2c3d")), Ordering::Equal);
        assert!(v("1.0.0-1") < v("1.0.0-alpha"));
    }

    const PUBLISHED: [&str; 9] = [
        "v1.2.9-canary",
        "v1.2.8-nightly.20261018",
        "v1.2.6-nightly.20261017",
        "v1.2.5-rc",
        "v1.2.4",
        "v1.2.2-nightly.20261016",
        "v1.2.2-rc",
        "v1.1.7",
        "v1.0.0-rc1",
    ];

    #[test]
    fn each_ring_hears_of_its_own_builds() {
        let newest = |current, ring| newer_tag(current, ring, &PUBLISHED);
        assert_eq!(newest("1.1.7+a1b2c3d", Ring::Stable), Some("v1.2.4".into()));
        assert_eq!(
            newest("1.2.2-rc+a1b2c3d", Ring::Rc),
            Some("v1.2.5-rc".into())
        );
        assert_eq!(
            newest("1.2.2-nightly.20261016+a1b2c3d", Ring::Nightly),
            Some("v1.2.8-nightly.20261018".into())
        );
        assert_eq!(
            newest("1.2.3-canary+a1b2c3d", Ring::Canary),
            Some("v1.2.9-canary".into())
        );
        assert_eq!(newest("1.2.4+a1b2c3d", Ring::Stable), None);
        assert_eq!(newest("1.2.9-canary+a1b2c3d", Ring::Canary), None);
    }

    #[test]
    fn a_release_candidate_hears_of_the_release() {
        let tags = ["v1.2.7", "v1.2.5-rc"];
        assert_eq!(
            newer_tag("1.2.5-rc+a1b2c3d", Ring::Rc, &tags),
            Some("v1.2.7".into())
        );
        // Not when it is a release of the commit running.
        let tags = ["v1.2.5", "v1.2.5-rc"];
        assert_eq!(newer_tag("1.2.5-rc+a1b2c3d", Ring::Rc, &tags), None);
        assert_eq!(newer_tag("1.2.5-rc+a1b2c3d", Ring::Stable, &tags), None);
    }

    #[test]
    fn a_ring_switched_to_hears_of_later_commits_only() {
        // A nightly installation switched to stable waits for a release of a later commit.
        assert_eq!(
            newer_tag("1.2.8-nightly.20261018+a1b2c3d", Ring::Stable, &PUBLISHED),
            None
        );
        assert_eq!(
            newer_tag("1.2.4+a1b2c3d", Ring::Nightly, &PUBLISHED),
            Some("v1.2.8-nightly.20261018".into())
        );
        // A dev build's version against a ring's builds: later commits only. (Its channel is
        // cargo, which the check narrows to stable, Ring::on.)
        assert_eq!(
            newer_tag("1.2.5-dev+a1b2c3d.dirty", Ring::Nightly, &PUBLISHED),
            Some("v1.2.8-nightly.20261018".into())
        );
        assert_eq!(
            newer_tag("1.2.8-dev+a1b2c3d", Ring::Nightly, &PUBLISHED),
            None
        );
    }

    #[test]
    fn tags_that_are_not_ring_builds_are_passed_over() {
        let tags = ["nightly", "v0.7", "v1.3.0-beta", "v1.3.0-dev", "v1.2.0"];
        assert_eq!(
            newer_tag("1.1.0", Ring::Stable, &tags),
            Some("v1.2.0".into())
        );
        assert_eq!(newer_tag("1.1.0", Ring::Rc, &["nightly"]), None);
        assert_eq!(newer_tag("1.1.0", Ring::Rc, &[]), None);
    }

    #[test]
    fn download_opens_the_channels_own_file() {
        let url = |channel, target| match Update::of("v0.6.0-rc1", channel, target) {
            Some(Update {
                version,
                download: Download::Open(url),
                ..
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
    fn the_notes_are_on_the_releases_page() {
        let update = Update::of("v0.8.0", Channel::Dmg, "aarch64-apple-darwin").unwrap();
        assert_eq!(
            update.notes(),
            "https://github.com/aquamoth/parterre/releases/tag/v0.8.0"
        );
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
