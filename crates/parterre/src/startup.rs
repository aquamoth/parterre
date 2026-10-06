//! What a normal start costs, milestone by milestone, printed to stderr when
//! `PARTERRE_STARTUP_TIMING` is set: `startup: graph shown at 1180 ms`. A normal start, the
//! one the user does, settings read and all, not a scripted one that skips some of the work
//! (#309). The clock starts at `main`, so the time Windows takes to load the executable isn't
//! in it.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static MARKED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// Starts the clock; the first thing `main` does.
pub fn begin() {
    START.get_or_init(Instant::now);
}

/// Whether the milestones are wanted.
pub fn enabled() -> bool {
    std::env::var_os("PARTERRE_STARTUP_TIMING").is_some()
}

/// Reports reaching `what`, the first time only.
pub fn mark(what: &'static str) {
    if !enabled() {
        return;
    }
    let mut marked = MARKED.lock().unwrap_or_else(|e| e.into_inner());
    if marked.contains(&what) {
        return;
    }
    marked.push(what);
    let ms = START.get().map_or(0, |start| start.elapsed().as_millis());
    eprintln!("startup: {what} at {ms} ms");
}
