//! Sessions in PostHog's sense (#262): server SDKs such as `posthog-rs` set no `$session_id`,
//! so parterre does, as PostHog's own SDKs keep it. A session lasts while the user is active:
//! any input or event. A new one starts
//! - on activity after [`IDLE_TIMEOUT`] without any (PostHog's rule),
//! - on activity [`MAX_LENGTH`] after it started (PostHog's longest session), and
//! - when the user opens another repository than the one opened last.
//!
//! PostHog takes a custom session ID only if it is a UUIDv7 whose time is at or before the
//! session's first event, and less than 24 hours before its last one
//! (<https://posthog.com/docs/data/sessions>). So the ID is made from the time the session
//! starts, and a clock turned back before that time starts a new one.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use uuid::{NoContext, Timestamp, Uuid};

const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

const MAX_LENGTH: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug)]
pub(crate) struct Session {
    id: Uuid,
    started: SystemTime,
    last_active: SystemTime,
    last_repository: Option<PathBuf>,
}

impl Session {
    pub(crate) fn new(now: SystemTime) -> Session {
        Session {
            id: id_at(now),
            started: now,
            last_active: now,
            last_repository: None,
        }
    }

    pub(crate) fn id(&self) -> String {
        self.id.to_string()
    }

    /// Input or an event at `now`. Returns the ID of the session it is in.
    pub(crate) fn active(&mut self, now: SystemTime) -> String {
        let idle = now.duration_since(self.last_active).unwrap_or_default();
        let age = now.duration_since(self.started);
        if idle > IDLE_TIMEOUT || age.is_err() || age.is_ok_and(|age| age >= MAX_LENGTH) {
            self.restart(now);
        }
        self.last_active = now;
        self.id()
    }

    /// The first repository opened is the session's own, whether at launch or later.
    pub(crate) fn opened(&mut self, repository: &Path, now: SystemTime) {
        if self
            .last_repository
            .as_deref()
            .is_some_and(|r| r != repository)
        {
            self.restart(now);
            self.last_active = now;
        } else {
            self.active(now);
        }
        self.last_repository = Some(repository.to_owned());
    }

    fn restart(&mut self, now: SystemTime) {
        self.id = id_at(now);
        self.started = now;
    }
}

fn id_at(now: SystemTime) -> Uuid {
    let since = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    Uuid::new_v7(Timestamp::from_unix(
        NoContext,
        since.as_secs(),
        since.subsec_nanos(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minutes: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_790_000_000) + Duration::from_secs(minutes * 60)
    }

    #[test]
    fn ids_are_uuidv7s_of_the_time_the_session_started() {
        let session = Session::new(at(0));
        let id = Uuid::parse_str(&session.id()).unwrap();
        assert_eq!(id.get_version_num(), 7);
        let (seconds, _) = id.get_timestamp().unwrap().to_unix();
        let start = at(0).duration_since(UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(seconds, start);
        // Another session, even started in the same millisecond, has another ID.
        assert_ne!(Session::new(at(0)).id(), session.id());
    }

    #[test]
    fn activity_keeps_the_session() {
        let mut session = Session::new(at(0));
        let id = session.id();
        // Every 30 minutes, for most of a day.
        for minutes in (30..24 * 60).step_by(30) {
            assert_eq!(session.active(at(minutes)), id, "{minutes} minutes");
        }
    }

    #[test]
    fn a_new_session_starts_after_30_minutes_idle() {
        let mut session = Session::new(at(0));
        let id = session.active(at(10));
        let next = session.active(at(41));
        assert_ne!(next, id);
        // Then it lasts as long as there is activity.
        assert_eq!(session.active(at(60)), next);
        let (seconds, _) = Uuid::parse_str(&next)
            .unwrap()
            .get_timestamp()
            .unwrap()
            .to_unix();
        assert_eq!(
            seconds,
            at(41).duration_since(UNIX_EPOCH).unwrap().as_secs()
        );
    }

    #[test]
    fn a_new_session_starts_after_24_hours_whatever_the_activity() {
        let mut session = Session::new(at(0));
        let id = session.id();
        for minutes in (1..24 * 60).step_by(29) {
            assert_eq!(session.active(at(minutes)), id, "{minutes} minutes");
        }
        let next = session.active(at(24 * 60));
        assert_ne!(next, id);
        assert_eq!(session.active(at(24 * 60 + 29)), next);
    }

    #[test]
    fn a_clock_turned_back_starts_a_new_session() {
        let mut session = Session::new(at(60));
        let id = session.active(at(61));
        // Back before the session started: its ID would be later than the events.
        assert_ne!(session.active(at(50)), id);
        // Back, but not before the start: still the same session.
        let mut session = Session::new(at(60));
        let id = session.active(at(70));
        assert_eq!(session.active(at(65)), id);
    }

    #[test]
    fn opening_another_repository_starts_a_new_session() {
        let mut session = Session::new(at(0));
        let id = session.id();
        // The first repository, at launch or later, is the session's own.
        session.opened(Path::new("/src/a"), at(1));
        assert_eq!(session.id(), id);
        session.opened(Path::new("/src/b"), at(2));
        let next = session.id();
        assert_ne!(next, id);
        // Activity: no new session, nor for the same repository again.
        assert_eq!(session.active(at(3)), next);
        session.opened(Path::new("/src/b"), at(4));
        assert_eq!(session.id(), next);
        // Back to the first one is another again.
        session.opened(Path::new("/src/a"), at(5));
        assert_ne!(session.id(), next);
    }

    #[test]
    fn opening_a_repository_is_activity() {
        let mut session = Session::new(at(0));
        session.opened(Path::new("/src/a"), at(20));
        let id = session.id();
        // 30 minutes after the open, not after the start.
        assert_eq!(session.active(at(45)), id);
        // The same repository after a while idle: a new session, as for any activity.
        session.opened(Path::new("/src/a"), at(80));
        assert_ne!(session.id(), id);
    }

    #[test]
    fn a_session_after_another_repository_lasts_from_its_own_start() {
        let mut session = Session::new(at(0));
        session.opened(Path::new("/src/a"), at(0));
        session.opened(Path::new("/src/b"), at(23 * 60));
        let id = session.id();
        // Past 24 hours since the first session, not since this one.
        for minutes in [25, 50, 75] {
            assert_eq!(
                session.active(at(23 * 60 + minutes)),
                id,
                "{minutes} minutes"
            );
        }
    }
}
