//! Usage statistics (#261): whether anything may be sent to PostHog, and which lifecycle events
//! a launch sends. No network here, so all of it is tested without one; the sending is in
//! `posthog.rs`.

use crate::Version;

/// What the first-run prompt and Settings › Privacy choose to send to PostHog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choices {
    /// Installs, launches and features, with the install ID. Ticked unless the user unticks it.
    pub usage_statistics: bool,
    /// Panics, without the install ID. Unticked unless the user ticks it; read at start (#263).
    pub crash_reports: bool,
}

impl Default for Choices {
    /// As the first-run prompt starts: usage statistics ticked, crash reports not (#223).
    fn default() -> Self {
        Choices {
            usage_statistics: true,
            crash_reports: false,
        }
    }
}

/// What a build of parterre can send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Build {
    /// Built with the `send` feature.
    pub send: bool,
    /// A debug build, which never sends.
    pub debug: bool,
}

impl Build {
    /// This build.
    pub const THIS: Build = Build {
        send: cfg!(feature = "send"),
        debug: cfg!(debug_assertions),
    };
}

/// Whether this build has usage statistics and crash reports: built with `send`. Without it,
/// there is no first-run prompt and Settings › Privacy shows them off and greyed out.
pub fn has_usage_statistics() -> bool {
    Build::THIS.send
}

/// Whether `DO_NOT_TRACK` is set (consoledonottrack.com): to anything but nothing or `0`. Then
/// nothing goes to PostHog, whatever the user chose.
pub fn do_not_track() -> bool {
    is_set(std::env::var("DO_NOT_TRACK").ok().as_deref())
}

/// The convention for `DO_NOT_TRACK`'s value: set, and neither empty nor `0`.
pub(crate) fn is_set(value: Option<&str>) -> bool {
    value
        .map(str::trim)
        .is_some_and(|v| !v.is_empty() && v != "0")
}

/// Whether to ask the first-run prompt: in a build with `send`, while it hasn't been answered
/// (`answered`), unless `DO_NOT_TRACK` is set, which has answered for the user.
pub fn asks(build: Build, answered: bool, do_not_track: bool) -> bool {
    build.send && !answered && !do_not_track
}

/// Whether usage statistics are sent: a release build with `send`, the first-run prompt
/// answered (`choices`) with usage statistics ticked, and `DO_NOT_TRACK` not set.
pub fn sends(build: Build, choices: Option<Choices>, do_not_track: bool) -> bool {
    build.send && !build.debug && !do_not_track && choices.is_some_and(|c| c.usage_statistics)
}

/// Whether the update check may ask GitHub: not before the first-run prompt is answered, as
/// nothing is sent before then (#223). The update check's own switch is the caller's.
pub fn may_check_for_updates(prompting: bool) -> bool {
    !prompting
}

/// A random install ID, made once at the first run and never changed. It is the `distinct_id`
/// of the usage statistics.
pub fn new_install_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// A version as the usage statistics name it, `$app_version`: `0.6.0` for `0.6.0 (a1b2c3d)`,
/// `0.6.1-dev.3` for `0.6.1-dev.3+a1b2c3d.dirty`. Kept as it is if it isn't a version.
pub fn app_version(version: &str) -> String {
    Version::parse(version).map_or_else(|| version.trim().to_owned(), |v| v.to_string())
}

/// An event of the app's life, named as PostHog's mobile SDKs name them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    /// The first run, once the first-run prompt is answered.
    Installed,
    /// The first run of a version other than the one that ran last.
    Updated { previous_version: String },
    /// Every launch.
    Opened,
    /// The window closing.
    Backgrounded,
}

impl Lifecycle {
    pub fn name(&self) -> &'static str {
        match self {
            Lifecycle::Installed => "Application Installed",
            Lifecycle::Updated { .. } => "Application Updated",
            Lifecycle::Opened => "Application Opened",
            Lifecycle::Backgrounded => "Application Backgrounded",
        }
    }
}

/// The events a launch of `version` sends: on the first run `Application Installed`, on the
/// first run of a version other than `last_version` (as [`app_version`] names them)
/// `Application Updated`, and then `Application Opened`.
pub fn launch(first_run: bool, last_version: Option<&str>, version: &str) -> Vec<Lifecycle> {
    let version = app_version(version);
    let mut events = Vec::new();
    if first_run {
        events.push(Lifecycle::Installed);
    } else if let Some(last) = last_version.filter(|last| app_version(last) != version) {
        events.push(Lifecycle::Updated {
            previous_version: app_version(last),
        });
    }
    events.push(Lifecycle::Opened);
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE: Build = Build {
        send: true,
        debug: false,
    };
    const ON: Option<Choices> = Some(Choices {
        usage_statistics: true,
        crash_reports: false,
    });
    const OFF: Option<Choices> = Some(Choices {
        usage_statistics: false,
        crash_reports: true,
    });

    #[test]
    fn the_prompt_starts_with_usage_statistics_ticked_and_crash_reports_not() {
        assert_eq!(Choices::default(), ON.unwrap());
    }

    #[test]
    fn do_not_track_is_set_to_anything_but_nothing_or_zero() {
        for value in ["1", "true", "yes", " 1 ", "false"] {
            assert!(is_set(Some(value)), "{value:?}");
        }
        for value in [None, Some(""), Some("0"), Some(" "), Some(" 0 ")] {
            assert!(!is_set(value), "{value:?}");
        }
    }

    #[test]
    fn the_prompt_is_asked_once_in_builds_that_can_send() {
        assert!(asks(RELEASE, false, false));
        assert!(!asks(RELEASE, true, false));
        // Debug builds ask too, so the prompt can be seen; they send nothing.
        let debug = Build {
            send: true,
            debug: true,
        };
        assert!(asks(debug, false, false));
        let without_send = Build {
            send: false,
            debug: false,
        };
        assert!(!asks(without_send, false, false));
        // DO_NOT_TRACK has answered.
        assert!(!asks(RELEASE, false, true));
    }

    #[test]
    fn nothing_is_sent_before_the_prompt_is_answered() {
        assert!(!sends(RELEASE, None, false));
        assert!(sends(RELEASE, ON, false));
    }

    #[test]
    fn unticking_usage_statistics_sends_nothing() {
        assert!(!sends(RELEASE, OFF, false));
    }

    #[test]
    fn do_not_track_sends_nothing_whatever_was_chosen() {
        assert!(!sends(RELEASE, ON, true));
    }

    #[test]
    fn debug_builds_and_builds_without_send_never_send() {
        for build in [
            Build {
                send: true,
                debug: true,
            },
            Build {
                send: false,
                debug: false,
            },
        ] {
            assert!(!sends(build, ON, false), "{build:?}");
        }
        // The tests are a debug build, or a build without `send`.
        if cfg!(debug_assertions) || !cfg!(feature = "send") {
            assert!(!sends(Build::THIS, ON, false));
        }
    }

    #[test]
    fn the_update_check_waits_for_the_prompt() {
        assert!(!may_check_for_updates(true));
        assert!(may_check_for_updates(false));
    }

    #[test]
    fn install_ids_are_random_uuids() {
        let (a, b) = (new_install_id(), new_install_id());
        assert_ne!(a, b);
        let id = uuid::Uuid::parse_str(&a).unwrap();
        assert_eq!(id.get_version_num(), 4);
    }

    #[test]
    fn the_first_run_is_installed_and_opened() {
        assert_eq!(
            launch(true, None, "0.6.0 (a1b2c3d)"),
            [Lifecycle::Installed, Lifecycle::Opened]
        );
    }

    #[test]
    fn a_new_version_is_updated_from_the_previous_one() {
        assert_eq!(
            launch(false, Some("0.5.1"), "0.6.0 (a1b2c3d)"),
            [
                Lifecycle::Updated {
                    previous_version: "0.5.1".into()
                },
                Lifecycle::Opened
            ]
        );
        // Older, too: a downgrade is a change of version.
        assert_eq!(
            launch(false, Some("0.6.0"), "0.5.1 (a1b2c3d)")[0],
            Lifecycle::Updated {
                previous_version: "0.6.0".into()
            }
        );
    }

    #[test]
    fn the_same_version_is_only_opened() {
        assert_eq!(
            launch(false, Some("0.6.0"), "0.6.0 (a1b2c3d)"),
            [Lifecycle::Opened]
        );
        assert_eq!(launch(false, None, "0.6.0"), [Lifecycle::Opened]);
    }

    #[test]
    fn events_are_named_as_posthogs_mobile_sdks_name_them() {
        let names = [
            Lifecycle::Installed,
            Lifecycle::Updated {
                previous_version: String::new(),
            },
            Lifecycle::Opened,
            Lifecycle::Backgrounded,
        ]
        .map(|e| e.name());
        assert_eq!(
            names,
            [
                "Application Installed",
                "Application Updated",
                "Application Opened",
                "Application Backgrounded"
            ]
        );
    }

    #[test]
    fn app_versions_leave_the_commit_out() {
        assert_eq!(app_version("0.6.0 (a1b2c3d)"), "0.6.0");
        assert_eq!(app_version("0.6.1-dev.3+a1b2c3d.dirty"), "0.6.1-dev.3");
        assert_eq!(app_version("next"), "next");
    }
}
