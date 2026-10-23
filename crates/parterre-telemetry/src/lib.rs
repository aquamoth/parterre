//! Everything parterre asks or sends over the network, so that a service is swapped by
//! replacing this crate (#225). Without the `send` feature nothing is asked or sent.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, SystemTime};

mod channel;
mod crash;
#[cfg(feature = "send")]
mod download;
mod feature;
#[cfg(feature = "send")]
mod github;
#[cfg(feature = "send")]
mod posthog;
mod ring;
// No session is started without `send`.
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
pub use ring::Ring;
pub use update::{CARGO_INSTALL, Download, Install, Release, Update, Version, newer};
pub use usage::{
    Build, Choices, Lifecycle, app_version, asks_first_run_prompt, do_not_track,
    has_usage_statistics, launch, may_check_for_updates, new_install_id, sends_usage_statistics,
};

pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

pub const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// What every event of a launch says besides its own name.
#[derive(Clone, Debug)]
pub struct Context {
    pub install_id: String,
    /// As `--version` prints it: `0.6.0 (a1b2c3d)`.
    pub version: String,
    /// In points.
    pub screen: Option<[f32; 2]>,
    /// Called on the sending thread, so that the start doesn't wait for git.
    pub git_version: fn() -> Option<String>,
}

/// Usage statistics on their way to PostHog, until [`close`](Usage::close)d, or dropped when
/// the user unticks them.
#[derive(Debug)]
pub struct Usage {
    #[cfg(feature = "send")]
    sender: posthog::Sender,
    /// Shared with the features' sink: recording one is activity too.
    session: Arc<Mutex<session::Session>>,
}

impl Usage {
    /// Starts sending, with `events` at once, if [`sends_usage_statistics`] allows; from then on
    /// the features [`record`]ed are sent too.
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

    fn start_with(
        build: Build,
        do_not_track: bool,
        host: &str,
        choices: Option<Choices>,
        context: Context,
        events: Vec<Lifecycle>,
    ) -> Option<Usage> {
        if !sends_usage_statistics(build, choices, do_not_track) {
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

    /// Sends `Application Backgrounded` and what is queued, waiting at most [`CLOSE_TIMEOUT`].
    pub fn close(self) {
        feature::sink(None);
        let session = lock(&self.session).active(SystemTime::now());
        #[cfg(feature = "send")]
        self.sender.close(session, CLOSE_TIMEOUT);
        #[cfg(not(feature = "send"))]
        let _ = session;
    }

    /// The user gave input in any of parterre's windows. Cheap: call it for every frame that has
    /// some.
    pub fn input(&mut self) {
        lock(&self.session).active(SystemTime::now());
    }

    /// `repository` is a folder all its worktrees share. It is never sent.
    pub fn opened(&mut self, repository: &Path) {
        lock(&self.session).opened(repository, SystemTime::now());
    }
}

impl Drop for Usage {
    fn drop(&mut self) {
        feature::sink(None);
    }
}

/// Ignores poisoning: the session is valid at every step.
fn lock(session: &Mutex<session::Session>) -> MutexGuard<'_, session::Session> {
    session.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where the features [`record`]ed go: each is activity in `session` at `now`, and is handed to
/// `send` with the session's ID. Waits only for the session's lock.
#[cfg(any(feature = "send", test))]
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

/// Sends every panic from now on as a crash report, if [`sends_crash_reports`] allows. Decided
/// once: unticking them takes effect at the next start. The panic hook already installed still
/// runs, after a panic is sent. Returns whether they were turned on.
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

static PUBLISHED: OnceLock<bool> = OnceLock::new();

/// Tells, once and before anything asks [`Channel::current`], whether the pipeline wrote this
/// build's version, i.e. published it. Unset, it wasn't.
pub fn set_published(published: bool) {
    let _ = PUBLISHED.set(published);
}

impl Channel {
    pub fn current() -> Channel {
        static CURRENT: OnceLock<Channel> = OnceLock::new();
        *CURRENT.get_or_init(|| {
            let exe = std::env::current_exe().ok();
            let marker = exe
                .as_deref()
                .and_then(channel::marker_path)
                .and_then(|path| channel::read_marker(&path));
            let published = PUBLISHED.get().copied().unwrap_or(false);
            Channel::of(
                marker.as_deref(),
                published,
                |name| std::env::var(name).ok(),
                exe.as_deref(),
            )
        })
    }
}

/// Built with `send`, and not on Snap or Flatpak, whose stores update parterre.
pub fn has_update_check() -> bool {
    cfg!(feature = "send") && !matches!(Channel::current(), Channel::Snap | Channel::Flatpak)
}

/// Why [`download`] gave up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadError {
    Cancelled,
    /// What went wrong, for the user: "couldn't download …: …".
    Failed(String),
}

/// Downloads `install`'s file into `dir` for *Update now* (#431), and checks it against its
/// release's `SHA256SUMS`: the path of the checked file. A partial or mismatched file is
/// removed. `progress` is told the bytes so far and the size, if the server gives it, and
/// `cancelled` is asked between reads. Blocks: call it on a thread of its own.
pub fn download(
    install: &Install,
    dir: &Path,
    progress: &dyn Fn(u64, Option<u64>),
    cancelled: &dyn Fn() -> bool,
) -> Result<std::path::PathBuf, DownloadError> {
    #[cfg(feature = "send")]
    return download::download(install, dir, progress, cancelled);
    #[cfg(not(feature = "send"))]
    {
        let _ = (install, dir, progress, cancelled);
        Err(DownloadError::Failed(
            "this build makes no requests of its own (built without `send`)".to_owned(),
        ))
    }
}

/// Asks GitHub whether a newer release is out, at once and then every
/// [`UPDATE_CHECK_INTERVAL`], on a thread of its own until dropped. Failures are silent: it asks
/// again next time.
#[derive(Debug)]
pub struct UpdateCheck {
    found: Receiver<Update>,
    newer: Option<Update>,
    _stops_thread_on_drop: Sender<()>,
}

impl UpdateCheck {
    /// `current` is this build's version: `1.2.3-rc+a1b2c3d`; `ring` the installation's, which
    /// is narrowed to what this copy's channel can take ([`Ring::on`]). `found` is called on the
    /// checking thread when a newer build turns up. `None`, asking nothing, without
    /// [`has_update_check`].
    pub fn start(
        current: &str,
        ring: Ring,
        found: impl Fn() + Send + 'static,
    ) -> Option<UpdateCheck> {
        if !has_update_check() {
            return None;
        }
        let current = Version::parse(current)?;
        let ring = ring.on(Channel::current());
        let (stop, stopped) = mpsc::channel();
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            check_for_updates(&current, ring, &stopped, &send, &found);
        });
        Some(UpdateCheck {
            found: receive,
            newer: None,
            _stops_thread_on_drop: stop,
        })
    }

    pub fn newer(&mut self) -> Option<&Update> {
        if let Some(update) = self.found.try_iter().last() {
            self.newer = Some(update);
        }
        self.newer.as_ref()
    }
}

#[cfg(feature = "send")]
fn check_for_updates(
    current: &Version,
    ring: Ring,
    stopped: &Receiver<()>,
    send: &Sender<Update>,
    found: &dyn Fn(),
) {
    use std::sync::mpsc::RecvTimeoutError;
    loop {
        let releases = github::releases().unwrap_or_default();
        if let Some(update) = newer(current, ring, &releases).and_then(|r| Update::to(&r.tag)) {
            if send.send(update).is_err() {
                return;
            }
            found();
        }
        if !matches!(
            stopped.recv_timeout(UPDATE_CHECK_INTERVAL),
            Err(RecvTimeoutError::Timeout)
        ) {
            return;
        }
    }
}

/// Never started: [`has_update_check`] is false without `send`.
#[cfg(not(feature = "send"))]
fn check_for_updates(_: &Version, _: Ring, _: &Receiver<()>, _: &Sender<Update>, _: &dyn Fn()) {}

#[cfg(test)]
mod tests {
    use super::*;

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
}
