use std::sync::mpsc::{self, Receiver, TryRecvError};

use eframe::egui;

/// Runs `work` on a thread of its own, repainting when it's done.
pub fn spawn<T: Send + 'static>(
    ctx: &egui::Context,
    work: impl FnOnce() -> T + Send + 'static,
) -> Receiver<T> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(work());
        ctx.request_repaint();
    });
    rx
}

/// What `rx` has received, if anything; `stopped()` once its worker is gone without sending.
pub fn received<T>(rx: &Receiver<T>, stopped: impl FnOnce() -> T) -> Option<T> {
    match rx.try_recv() {
        Ok(value) => Some(value),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => Some(stopped()),
    }
}
