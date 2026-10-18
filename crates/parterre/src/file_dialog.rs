//! rfd's blocking dialogs hold up the event loop until they close, and after a few seconds the
//! desktop (GNOME's Mutter, for one) offers to close parterre as "not responding". So the async
//! dialog runs to completion on a worker thread, and the app picks up the answer in a later frame.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::task::{Context, Poll, Wake, Waker};

use eframe::egui;

use crate::worker;

/// An open dialog, and what its answer is for.
#[derive(Debug)]
pub struct Pending<T> {
    pub what: T,
    answer: Receiver<Option<PathBuf>>,
}

impl<T> Pending<T> {
    pub fn start(
        what: T,
        dialog: impl Future<Output = Option<rfd::FileHandle>> + Send + 'static,
        ctx: &egui::Context,
    ) -> Pending<T> {
        let answer = worker::spawn(ctx, || block_on(dialog).map(|file| file.path().to_owned()));
        Pending { what, answer }
    }

    /// `None` while the dialog is open, then `Some` of what was picked.
    pub fn answer(&self) -> Option<Option<PathBuf>> {
        worker::received(&self.answer, || None)
    }
}

/// The system's folder picker, opened at `near` or the nearest folder above it that exists.
pub fn pick_folder(ctx: &egui::Context, title: &str, near: &str) -> Pending<()> {
    let mut dialog = rfd::AsyncFileDialog::new().set_title(title);
    if let Some(dir) = Path::new(near).ancestors().find(|a| a.is_dir()) {
        dialog = dialog.set_directory(dir);
    }
    Pending::start((), dialog.pick_folder(), ctx)
}

fn block_on<F: Future>(future: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::park(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn block_on_waits_for_a_wake_from_another_thread() {
        let (send, recv) = mpsc::channel::<u32>();
        let mut waker_sent = false;
        let (wake_send, wake_recv) = mpsc::channel::<Waker>();
        std::thread::spawn(move || {
            let waker = wake_recv.recv().unwrap();
            send.send(7).unwrap();
            waker.wake();
        });
        let value = block_on(std::future::poll_fn(|cx| match recv.try_recv() {
            Ok(v) => Poll::Ready(v),
            Err(_) => {
                if !waker_sent {
                    waker_sent = true;
                    wake_send.send(cx.waker().clone()).unwrap();
                }
                Poll::Pending
            }
        }));
        assert_eq!(value, 7);
    }
}
