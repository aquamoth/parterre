//! The clock starts at `main`, so the time the OS takes to load the executable isn't in it.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static MARKED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

pub fn begin() {
    START.get_or_init(Instant::now);
}

pub fn enabled() -> bool {
    std::env::var_os("PARTERRE_STARTUP_TIMING").is_some()
}

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
