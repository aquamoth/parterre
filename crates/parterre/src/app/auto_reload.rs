use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use eframe::egui;
use parterre_core::Repo;
use parterre_core::watch::RefStorage;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
// How long the refs must stay unchanged before loading: a rebase or a fetch of many branches
// is loaded once it is done, not halfway.
const SETTLE_TIME: Duration = Duration::from_millis(300);

#[derive(Debug)]
pub struct Watcher {
    path: PathBuf,
    loaded: Receiver<Repo>,
    _stop_on_drop: Sender<()>,
}

impl Watcher {
    pub fn start(path: &Path, ctx: &egui::Context) -> Watcher {
        let (stop, stopped) = mpsc::channel();
        let (send, loaded) = mpsc::channel();
        let (dir, ctx) = (path.to_owned(), ctx.clone());
        std::thread::spawn(move || watch(&dir, &stopped, &send, &ctx));
        Watcher {
            path: path.to_owned(),
            loaded,
            _stop_on_drop: stop,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn take(&self) -> Option<Repo> {
        self.loaded.try_iter().last()
    }
}

fn watch(dir: &Path, stopped: &Receiver<()>, send: &Sender<Repo>, ctx: &egui::Context) {
    let sleep_unless_stopped =
        |time| matches!(stopped.recv_timeout(time), Err(RecvTimeoutError::Timeout));
    let Ok(storage) = RefStorage::locate(dir) else {
        return;
    };
    let mut seen = storage.fingerprint();
    while sleep_unless_stopped(POLL_INTERVAL) {
        let mut now = storage.fingerprint();
        if now == seen {
            continue;
        }
        loop {
            if !sleep_unless_stopped(SETTLE_TIME) {
                return;
            }
            let later = storage.fingerprint();
            if later == now {
                break;
            }
            now = later;
        }
        seen = now;
        // Failing halfway through a git command is retried at the next change.
        if let Ok(repo) = parterre_core::git::load_repo(dir) {
            if send.send(repo).is_err() {
                return;
            }
            ctx.request_repaint();
        }
    }
}
