//! When the stuck banner shows. Git can be mid-operation for a moment without anything being
//! wrong: a slow operation of parterre's own, or a rebase run in a terminal. So:
//!
//! - While parterre's own operation runs, and until the worktree has been looked at after it,
//!   the banner stays as it was: it neither appears nor goes.
//! - A look right after parterre's own operation, F5 or opening the repository shows it at
//!   once.
//! - After a change from outside, it shows once the worktree has stayed stuck for [`WAIT`]
//!   seconds, by looks at least that far apart. It goes at once when a look finds the
//!   worktree no longer stuck.

/// How long a worktree must stay stuck, after a change from outside, before the banner shows.
pub const WAIT: f64 = 1.5;

/// The banner's state: fed looks at the worktree, asked whether it shows.
#[derive(Clone, Debug, Default)]
pub struct BannerTimer {
    /// When looks first and last found the worktree stuck, since it last wasn't.
    stuck: Option<(f64, f64)>,
    /// A look that shows the banner at once found it stuck.
    at_once: bool,
    shown: bool,
}

impl BannerTimer {
    /// A look at the worktree at `now`. `at_once` for a look right after parterre's own
    /// operation, F5 or opening the repository.
    pub fn look(&mut self, stuck: bool, now: f64, at_once: bool) {
        if !stuck {
            self.stuck = None;
            self.at_once = false;
            return;
        }
        let first = self.stuck.map_or(now, |(first, _)| first);
        self.stuck = Some((first, now));
        self.at_once |= at_once;
    }

    /// Whether the banner shows. While `frozen` (parterre's own operation runs, or hasn't been
    /// looked at since), it stays as it was.
    pub fn update(&mut self, frozen: bool) -> bool {
        if !frozen {
            self.shown = self
                .stuck
                .is_some_and(|(first, last)| self.at_once || last - first >= WAIT);
        }
        self.shown
    }

    /// The worktree was last found stuck: look again, to show the banner or see it go.
    pub fn watching(&self) -> bool {
        self.stuck.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_stuck_shows_nothing() {
        let mut t = BannerTimer::default();
        t.look(false, 0.0, true);
        assert!(!t.update(false));
        assert!(!t.watching());
    }

    #[test]
    fn opening_a_stuck_repository_shows_it_at_once() {
        let mut t = BannerTimer::default();
        t.look(true, 0.0, true);
        assert!(t.update(false));
    }

    #[test]
    fn a_change_from_outside_shows_it_only_once_it_lasts() {
        let mut t = BannerTimer::default();
        t.look(true, 10.0, false);
        assert!(!t.update(false));
        assert!(t.watching());
        t.look(true, 11.0, false);
        assert!(!t.update(false));
        t.look(true, 11.5, false);
        assert!(t.update(false));
    }

    #[test]
    fn a_terminal_operation_that_finishes_in_time_never_shows_it() {
        let mut t = BannerTimer::default();
        t.look(true, 10.0, false);
        t.look(true, 11.0, false);
        t.look(false, 12.0, false);
        assert!(!t.update(false));
        // Stuck again later starts the wait over.
        t.look(true, 20.0, false);
        assert!(!t.update(false));
    }

    #[test]
    fn it_goes_at_once_when_git_is_done() {
        let mut t = BannerTimer::default();
        t.look(true, 0.0, true);
        assert!(t.update(false));
        t.look(false, 0.5, false);
        assert!(!t.update(false));
    }

    #[test]
    fn a_slow_operation_of_parterre_s_own_never_flashes_it() {
        let mut t = BannerTimer::default();
        // Looks while it runs find git mid-operation, for longer than the wait.
        t.look(true, 0.0, false);
        t.look(true, 3.0, false);
        assert!(!t.update(true));
        // It ended cleanly: the look after it finds nothing.
        t.look(false, 4.0, true);
        assert!(!t.update(false));
    }

    #[test]
    fn an_operation_that_stops_shows_it_as_soon_as_it_is_looked_at() {
        let mut t = BannerTimer::default();
        assert!(!t.update(true));
        t.look(true, 1.0, true);
        assert!(t.update(false));
    }

    #[test]
    fn an_operation_while_it_shows_keeps_it_shown() {
        let mut t = BannerTimer::default();
        t.look(true, 0.0, true);
        assert!(t.update(false));
        // Keeping a file: the banner neither goes nor comes back while git runs.
        t.look(false, 1.0, false);
        assert!(t.update(true));
        t.look(true, 1.2, true);
        assert!(t.update(false));
    }
}
