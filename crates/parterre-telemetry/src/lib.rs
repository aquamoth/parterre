//! What parterre asks and sends over the network, behind one small interface, so that the
//! services behind it can be swapped by replacing this crate (#175, #225):
//! - the update check (#226, #258): GitHub's releases API, asked at start and then once a day,
//!   and nothing of parterre's own sent with it;
//! - the usage statistics (#261): installs, updates and launches, sent to PostHog with the
//!   install ID while the user leaves them ticked, through [`Usage`], in sessions (#262);
//! - feature events (#264): which windows, dialogs, menus and actions are used, through
//!   [`record`], with the settings [`register`]ed, sent while [`Usage`] is and in its sessions;
//! - the crash reports (#263): panics, sent to PostHog the moment they happen, without the
//!   install ID, the session or the registered settings, once the user has ticked them
//!   ([`start_crash_reports`]).
//!
//! The requests sit behind the `send` feature. Without it nothing is asked or sent: there is no
//! update check ([`UpdateCheck::start`] gives `None`), no usage statistics ([`Usage::start`]
//! gives `None`) and no crash reports.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, SystemTime};

mod channel;
mod crash;
mod feature;
#[cfg(feature = "send")]
mod github;
#[cfg(feature = "send")]
mod posthog;
// Without `send` there is no usage statistics, so no session is started.
#[cfg_attr(not(feature = "send"), allow(dead_code))]
mod session;
mod update;
mod usage;

pub use channel::Channel;
pub use crash::sends_crash_reports;
pub use feature::{
    Action, DiffForm, DragModel, Feature, GraphDirection, GraphMode, LogLayout, MAX_REPOSITORIES,
    Menu, Properties, Range, Screen, Theme, Value, record, recording, register,
};
pub use update::{CARGO_INSTALL, Download, Release, Update, Version, newer};
pub use usage::{
    Build, Choices, Lifecycle, app_version, asks, do_not_track, has_usage_statistics, launch,
    may_check_for_updates, new_install_id, sends,
};

/// The longest closing waits for the usage statistics to be sent.
pub const CLOSE: Duration = Duration::from_secs(2);

/// What every event of a launch says besides its own name.
#[derive(Clone, Debug)]
pub struct Context {
    /// The install ID: the `distinct_id`.
    pub install_id: String,
    /// This build's version (`0.6.0 (a1b2c3d)`).
    pub version: String,
    /// The screen's size in points (logical pixels), if known.
    pub screen: Option<[f32; 2]>,
    /// The version of the git parterre runs, asked on the sending thread.
    pub git_version: fn() -> Option<String>,
}

/// Usage statistics on their way to PostHog, from start until the window closes ([`close`]) or
/// the user unticks them (drop it).
///
/// Every event carries the session it is part of, as `$session_id`, the features [`record`]ed
/// included. One starts with the usage statistics, and a new one after 30 minutes without
/// activity (the user's [`input`], or an event such as a feature recorded), 24 hours after the
/// last one started, and when the user [`opened`] another repository.
///
/// [`close`]: Usage::close
/// [`input`]: Usage::input
/// [`opened`]: Usage::opened
#[derive(Debug)]
pub struct Usage {
    #[cfg(feature = "send")]
    sender: posthog::Sender,
    /// Shared with the features' sink, as recording one is activity too.
    session: Arc<Mutex<session::Session>>,
}

impl Usage {
    /// Starts sending usage statistics, with `events` at once ([`launch`] for a launch, none
    /// when ticked again), if the user's `choices` allow it ([`sends`]): `None` while the
    /// first-run prompt is unanswered (`choices` `None`), when usage statistics are unticked,
    /// `DO_NOT_TRACK` is set, and in debug builds and builds without `send`.
    ///
    /// While it is, the features [`record`]ed are sent too.
    pub fn start(
        choices: Option<Choices>,
        context: Context,
        events: Vec<Lifecycle>,
    ) -> Option<Usage> {
        #[cfg(feature = "send")]
        let host = posthog::HOST;
        #[cfg(not(feature = "send"))]
        let host = "";
        Usage::start_with(Build::THIS, do_not_track(), host, choices, context, events)
    }

    /// [`Usage::start`] for `build`, sending to `host`.
    fn start_with(
        build: Build,
        do_not_track: bool,
        host: &str,
        choices: Option<Choices>,
        context: Context,
        events: Vec<Lifecycle>,
    ) -> Option<Usage> {
        if !sends(build, choices, do_not_track) {
            return None;
        }
        #[cfg(feature = "send")]
        {
            let session = session::Session::new(SystemTime::now());
            let sender = posthog::Sender::start(host, context, session.id(), events);
            let session = Arc::new(Mutex::new(session));
            let sink = feature_sink(Arc::clone(&session), sender.sink(), SystemTime::now);
            feature::sink(Some(Box::new(sink)));
            Some(Usage { sender, session })
        }
        #[cfg(not(feature = "send"))]
        {
            let _ = (host, context, events);
            None
        }
    }

    /// The window closes: `Application Backgrounded`, and what is queued is sent, waiting at
    /// most [`CLOSE`].
    pub fn close(self) {
        feature::sink(None);
        let session = lock(&self.session).active(SystemTime::now());
        #[cfg(feature = "send")]
        self.sender.close(session, CLOSE);
        #[cfg(not(feature = "send"))]
        let _ = session;
    }

    /// The user gave input: a key, the pointer or the wheel, in any of parterre's windows.
    /// Cheap: call it for every frame that has some.
    pub fn input(&mut self) {
        lock(&self.session).active(SystemTime::now());
    }

    /// The user opened `repository`, named by a folder all its worktrees share: a new session
    /// if it is another than the one opened last. The folder is never sent.
    pub fn opened(&mut self, repository: &Path) {
        lock(&self.session).opened(repository, SystemTime::now());
    }
}

impl Drop for Usage {
    /// Features recorded from now on go nowhere.
    fn drop(&mut self) {
        feature::sink(None);
    }
}

/// The session, even if a thread panicked while holding it: it is valid at every step.
fn lock(session: &Mutex<session::Session>) -> MutexGuard<'_, session::Session> {
    session.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where the features [`record`]ed go while usage statistics are sent: each is activity in
/// `session` at `now`, as any event is, and is handed to `send` with the session's ID. Never
/// waits but for the session's lock, held only while the session rule runs.
// Without `send` there is no usage statistics, so no sink.
#[cfg_attr(not(feature = "send"), allow(dead_code))]
fn feature_sink(
    session: Arc<Mutex<session::Session>>,
    send: impl Fn(Feature, String) + Send + 'static,
    now: impl Fn() -> SystemTime + Send + 'static,
) -> impl Fn(Feature) + Send + 'static {
    move |feature| {
        let id = lock(&session).active(now());
        send(feature, id);
    }
}

/// Sends every panic from now on as a crash report, if the user's `choices` allow it
/// ([`sends_crash_reports`]): not while the first-run prompt is unanswered (`choices` `None`),
/// when crash reports are unticked, `DO_NOT_TRACK` is set, nor in debug builds and builds
/// without `send`. Decided once: unticking them later takes effect at the next start. The panic
/// hook already installed is still called after a panic is sent. `version` is this build's
/// (`0.6.0 (a1b2c3d)`); `git_version` is asked on a thread of its own. True if turned on.
pub fn start_crash_reports(
    choices: Option<Choices>,
    version: &str,
    git_version: fn() -> Option<String>,
) -> bool {
    if !sends_crash_reports(Build::THIS, choices, do_not_track()) {
        return false;
    }
    #[cfg(feature = "send")]
    return posthog::capture_panics(posthog::HOST, version, git_version, crash::homes());
    #[cfg(not(feature = "send"))]
    {
        let _ = (version, git_version);
        false
    }
}

/// How long the update check waits before asking again.
pub const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// The Rust target this was built for, which names the release files.
const TARGET: &str = env!("PARTERRE_TARGET");

impl Channel {
    /// This copy's channel: Snap or Flatpak when running in parterre's own, else the one its
    /// packaging job stamped into it, else `cargo install`.
    pub fn current() -> Channel {
        static CURRENT: OnceLock<Channel> = OnceLock::new();
        *CURRENT.get_or_init(|| {
            Channel::of(option_env!("PARTERRE_CHANNEL"), |name| {
                std::env::var(name).ok()
            })
        })
    }

    /// The channel of a build stamped `stamp`, given the environment `var`. Snap and Flatpak
    /// set the name of the app they run, and leak it to what is started from a terminal there,
    /// so only parterre's own counts.
    fn of(stamp: Option<&str>, var: impl Fn(&str) -> Option<String>) -> Channel {
        if var("SNAP_NAME").as_deref() == Some("parterre") {
            Channel::Snap
        } else if var("FLATPAK_ID").as_deref() == Some("se.trustfall.parterre") {
            Channel::Flatpak
        } else {
            stamp.and_then(Channel::stamped).unwrap_or(Channel::Cargo)
        }
    }
}

impl Update {
    /// Release `tag` (`v0.6.0`, or `0.6.0`) for this copy's channel, as the update check would
    /// offer it. `None` on Snap and Flatpak.
    pub fn to(tag: &str) -> Option<Update> {
        Update::of(tag, Channel::current(), TARGET)
    }
}

/// Whether this copy has an update check: built with `send`, and not on Snap or Flatpak, whose
/// stores update parterre.
pub fn has_update_check() -> bool {
    cfg!(feature = "send") && !matches!(Channel::current(), Channel::Snap | Channel::Flatpak)
}

/// Asks GitHub whether a newer release is out, at once and then every [`INTERVAL`], on a
/// thread of its own until dropped. Failures are silent: it asks again next time.
#[derive(Debug)]
pub struct UpdateCheck {
    found: Receiver<Update>,
    newer: Option<Update>,
    /// Dropping it stops the thread.
    _stop: Sender<()>,
}

impl UpdateCheck {
    /// Starts checking for releases newer than `current`, this build's version
    /// (`0.6.0 (a1b2c3d)`). `found` is called on the checking thread when one turns up. `None`
    /// where there is no update check ([`has_update_check`]): nothing is asked then.
    pub fn start(current: &str, found: impl Fn() + Send + 'static) -> Option<UpdateCheck> {
        let current = Version::parse(current).filter(|_| has_update_check())?;
        let (stop, stopped) = mpsc::channel();
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            ask(&current, &stopped, &send, &found);
        });
        Some(UpdateCheck {
            found: receive,
            newer: None,
            _stop: stop,
        })
    }

    /// The newer release found last, if any.
    pub fn newer(&mut self) -> Option<&Update> {
        if let Some(update) = self.found.try_iter().last() {
            self.newer = Some(update);
        }
        self.newer.as_ref()
    }
}

#[cfg(feature = "send")]
fn ask(current: &Version, stopped: &Receiver<()>, send: &Sender<Update>, found: &dyn Fn()) {
    use std::sync::mpsc::RecvTimeoutError;
    loop {
        let releases = github::releases(current.is_prerelease()).unwrap_or_default();
        if let Some(update) = newer(current, &releases).and_then(|r| Update::to(&r.tag)) {
            if send.send(update).is_err() {
                return;
            }
            found();
        }
        if !matches!(
            stopped.recv_timeout(INTERVAL),
            Err(RecvTimeoutError::Timeout)
        ) {
            return;
        }
    }
}

/// Never started: [`has_update_check`] is false without `send`.
#[cfg(not(feature = "send"))]
fn ask(_: &Version, _: &Receiver<()>, _: &Sender<Update>, _: &dyn Fn()) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<String> + 'a {
        |name| {
            vars.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn a_recorded_feature_is_activity_in_the_current_session() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000);
        let at = |minutes: u64| start + Duration::from_secs(minutes * 60);
        let session = Arc::new(Mutex::new(session::Session::new(start)));
        let first = lock(&session).id();
        let clock = Arc::new(Mutex::new(start));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let sink = feature_sink(
            Arc::clone(&session),
            {
                let sent = Arc::clone(&sent);
                move |feature, id| sent.lock().unwrap().push((feature, id))
            },
            {
                let clock = Arc::clone(&clock);
                move || *clock.lock().unwrap()
            },
        );
        let fit = Feature::Action(Action::Fit);
        // A feature every 20 minutes keeps the session, without any input.
        for minutes in [20, 40, 60] {
            *clock.lock().unwrap() = at(minutes);
            sink(fit);
        }
        // 30 minutes idle: the next feature starts a new one, which the usage statistics are
        // in from then on.
        *clock.lock().unwrap() = at(91);
        sink(fit);
        let sent = sent.lock().unwrap();
        let ids: Vec<_> = sent.iter().map(|(_, id)| id.as_str()).collect();
        assert_eq!(ids[..3], [first.as_str(); 3]);
        assert_ne!(ids[3], first);
        assert_eq!(lock(&session).id(), ids[3]);
        assert!(sent.iter().all(|(feature, _)| *feature == fit));
    }

    #[test]
    fn the_stamp_names_the_channel_and_an_unstamped_build_is_cargo() {
        for channel in Channel::STAMPED {
            assert_eq!(Channel::of(Some(channel.name()), env(&[])), channel);
        }
        assert_eq!(Channel::of(None, env(&[])), Channel::Cargo);
        assert_eq!(Channel::of(Some(""), env(&[])), Channel::Cargo);
    }

    #[test]
    fn snap_and_flatpak_are_told_by_their_own_app() {
        let snap = [("SNAP_NAME", "parterre")];
        assert_eq!(Channel::of(None, env(&snap)), Channel::Snap);
        let flatpak = [("FLATPAK_ID", "se.trustfall.parterre")];
        assert_eq!(Channel::of(None, env(&flatpak)), Channel::Flatpak);
        // Started from a terminal in another app's snap or flatpak.
        let code = [
            ("SNAP_NAME", "code"),
            ("FLATPAK_ID", "com.visualstudio.code"),
        ];
        assert_eq!(Channel::of(None, env(&code)), Channel::Cargo);
        assert_eq!(Channel::of(Some("deb"), env(&code)), Channel::Deb);
    }
}
