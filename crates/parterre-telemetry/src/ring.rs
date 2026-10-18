//! Rings: which builds an installation hears of (#390, #411). Not the channel, which is how it
//! was installed.

use crate::{Channel, Version};

/// From the narrowest to the widest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ring {
    /// Releases, for everyone.
    Stable,
    /// Release candidates, for testing.
    Rc,
    /// A build of `main` each night it has moved, for bleeding-edge testers.
    Nightly,
    /// A build of every merge to `main`, for the maintainer.
    Canary,
}

impl Ring {
    pub const ALL: [Ring; 4] = [Ring::Stable, Ring::Rc, Ring::Nightly, Ring::Canary];

    pub fn name(self) -> &'static str {
        match self {
            Ring::Stable => "stable",
            Ring::Rc => "rc",
            Ring::Nightly => "nightly",
            Ring::Canary => "canary",
        }
    }

    pub fn named(name: &str) -> Option<Ring> {
        Ring::ALL.into_iter().find(|r| r.name() == name)
    }

    /// The ring a version names: `1.2.3`, `1.2.3-rc` (or the older `1.2.3-rc2`),
    /// `1.2.3-nightly.20261016`, `1.2.3-canary`. `None` for anything else, such as a dev build.
    pub fn of(version: &Version) -> Option<Ring> {
        match version.pre() {
            [] => Some(Ring::Stable),
            [rc] if rc == "rc" => Some(Ring::Rc),
            [rc] if rc.strip_prefix("rc").is_some_and(is_number) => Some(Ring::Rc),
            [nightly, date] if nightly == "nightly" && date.len() == 8 && is_number(date) => {
                Some(Ring::Nightly)
            }
            [canary] if canary == "canary" => Some(Ring::Canary),
            _ => None,
        }
    }

    /// Whether an installation in this ring hears of builds of `build`'s: only its own, but
    /// release candidates hear of the release too (#397).
    pub fn hears_of(self, build: Ring) -> bool {
        self == build || (self == Ring::Rc && build == Ring::Stable)
    }

    /// The ring an installation keeps once a build of `running` (`None` for a dev build) has
    /// started, from the one `stored` before. Unset, it takes the running build's ring, stable for
    /// a dev build. A published build of a later ring widens it, a canary build only to nightly:
    /// canary is chosen in Settings. Nothing narrows it but the user.
    pub fn kept(stored: Option<Ring>, running: Option<Ring>) -> Ring {
        let running = running.map(|r| r.min(Ring::Nightly));
        match (stored, running) {
            (Some(stored), Some(running)) => stored.max(running),
            (Some(stored), None) => stored,
            (None, running) => running.unwrap_or(Ring::Stable),
        }
    }

    /// What an installation in this ring is offered on `channel`: stable releases only on a
    /// channel that can't take any other build.
    pub fn on(self, channel: Channel) -> Ring {
        if channel.is_stable_only() {
            Ring::Stable
        } else {
            self
        }
    }
}

fn is_number(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(version: &str) -> Option<Ring> {
        Ring::of(&Version::parse(version).unwrap())
    }

    #[test]
    fn the_version_names_the_ring() {
        assert_eq!(ring("1.2.3+a1b2c3d"), Some(Ring::Stable));
        assert_eq!(ring("1.0.0"), Some(Ring::Stable));
        assert_eq!(ring("1.2.3-rc+a1b2c3d"), Some(Ring::Rc));
        assert_eq!(ring("1.0.0-rc1"), Some(Ring::Rc));
        assert_eq!(ring("1.2.3-nightly.20261016+a1b2c3d"), Some(Ring::Nightly));
        assert_eq!(ring("1.2.3-canary+a1b2c3d"), Some(Ring::Canary));
        for other in [
            "1.2.3-dev+a1b2c3d.dirty",
            "0.0.0-dev",
            "0.6.1-dev.3+a1b2c3d",
            "0.5.0-rc1.dev.2",
            "1.2.3-nightly",
            "1.2.3-nightly.2026",
            "1.2.3-canary.1",
            "1.2.3-beta",
        ] {
            assert_eq!(ring(other), None, "{other}");
        }
    }

    #[test]
    fn rings_hear_of_their_own_builds_and_rc_of_releases_too() {
        use Ring::*;
        for (ring, hears) in [
            (Stable, vec![Stable]),
            (Rc, vec![Stable, Rc]),
            (Nightly, vec![Nightly]),
            (Canary, vec![Canary]),
        ] {
            for build in Ring::ALL {
                assert_eq!(
                    ring.hears_of(build),
                    hears.contains(&build),
                    "{ring:?} {build:?}"
                );
            }
        }
    }

    #[test]
    fn a_later_ring_widens_and_nothing_narrows() {
        use Ring::*;
        // The first run, and Reset: the running build's ring.
        assert_eq!(Ring::kept(None, Some(Rc)), Rc);
        assert_eq!(Ring::kept(None, Some(Nightly)), Nightly);
        assert_eq!(Ring::kept(None, None), Stable);
        // Canary is chosen in Settings only.
        assert_eq!(Ring::kept(None, Some(Canary)), Nightly);
        assert_eq!(Ring::kept(Some(Stable), Some(Canary)), Nightly);
        assert_eq!(Ring::kept(Some(Canary), Some(Canary)), Canary);
        // An rc installation that gets the release stays rc.
        assert_eq!(Ring::kept(Some(Rc), Some(Stable)), Rc);
        assert_eq!(Ring::kept(Some(Stable), Some(Rc)), Rc);
        assert_eq!(Ring::kept(Some(Canary), Some(Stable)), Canary);
        // A dev build changes nothing.
        assert_eq!(Ring::kept(Some(Nightly), None), Nightly);
    }

    #[test]
    fn a_stable_only_channel_is_offered_stable() {
        assert_eq!(Ring::Nightly.on(Channel::MsiMachine), Ring::Stable);
        assert_eq!(Ring::Rc.on(Channel::Cargo), Ring::Stable);
        assert_eq!(Ring::Nightly.on(Channel::Msi), Ring::Nightly);
    }

    #[test]
    fn names_go_both_ways() {
        for ring in Ring::ALL {
            assert_eq!(Ring::named(ring.name()), Some(ring));
        }
        assert_eq!(Ring::named("beta"), None);
    }
}
