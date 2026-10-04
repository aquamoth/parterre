//! Cancellation handles for parterre's child processes, shared by the crates that run them
//! (#214): [`Cancel`] for a git command or the syntax colouring's child, which holds the one
//! process running under it and kills it when cancelled; [`CancelTree`] for a git operation,
//! which kills its whole process group or tree, hooks included.

use std::process::{Child, ExitStatus};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Stops a child process run on another thread, one at a time. [`Cancel::cancel`] kills the
/// one held and waits for it, and a process offered after that is refused at once. Closing
/// the child's output would not do: git notices only when it next writes, which can be after
/// walking the whole history.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<Mutex<Running>>);

#[derive(Debug, Default)]
struct Running {
    cancelled: bool,
    child: Option<Child>,
}

/// The handle was cancelled before the process could be held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

/// What [`Cancel::poll`] found of the held child.
#[derive(Debug)]
pub enum Poll {
    /// No child is held: cancelled and killed, released, or its state unreadable (then it was
    /// killed and waited for).
    Gone,
    Running,
    /// Exited, and released.
    Exited(ExitStatus),
}

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }

    /// Kills the child held, if any, and waits for it; from here on, none can be held.
    pub fn cancel(&self) {
        let mut running = self.lock();
        running.cancelled = true;
        if let Some(mut child) = running.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.lock().cancelled
    }

    /// Holds `child` as the process running under this handle, for [`Cancel::cancel`] to
    /// kill. Once cancelled, the child is killed and waited for instead.
    pub fn hold(&self, mut child: Child) -> Result<(), Cancelled> {
        let mut running = self.lock();
        if running.cancelled {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Cancelled);
        }
        running.child = Some(child);
        Ok(())
    }

    /// Whether the held child has exited. One that has is released, as is one whose state
    /// can't be read, killed and waited for first.
    pub fn poll(&self) -> Poll {
        let mut running = self.lock();
        let Some(child) = running.child.as_mut() else {
            return Poll::Gone;
        };
        match child.try_wait() {
            Ok(None) => Poll::Running,
            Ok(Some(status)) => {
                running.child = None;
                Poll::Exited(status)
            }
            Err(_) => {
                if let Some(mut child) = running.child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                Poll::Gone
            }
        }
    }

    /// Takes the held child back, to wait for it once its output has ended. `None` once
    /// cancelled, or if it was never held.
    pub fn release(&self) -> Option<Child> {
        self.lock().child.take()
    }

    /// Kills the held child and waits for it, without cancelling the handle: for a child
    /// over its time.
    pub fn kill(&self) {
        if let Some(mut child) = self.release() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn lock(&self) -> MutexGuard<'_, Running> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Cancels a git operation: its whole process group on Unix, or process tree on Windows,
/// hooks included. One operation at a time.
#[derive(Clone, Debug, Default)]
pub struct CancelTree(Arc<Mutex<(bool, Option<u32>)>>);

/// Why [`CancelTree::start`] gave no process.
#[derive(Debug)]
pub enum Start {
    Cancelled,
    Spawn(std::io::Error),
}

impl CancelTree {
    pub fn new() -> CancelTree {
        CancelTree::default()
    }

    pub fn cancel(&self) {
        let mut state = self.lock();
        if state.0 {
            return;
        }
        state.0 = true;
        if let Some(pid) = state.1 {
            #[cfg(unix)]
            {
                signal_group(pid, "-TERM");
                let pending = self.0.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    let state = pending.lock().unwrap_or_else(PoisonError::into_inner);
                    // Keep the group registered until its output pipes close, even if Git
                    // has already exited. Hooks can ignore TERM and keep those pipes open.
                    if state.1 == Some(pid) {
                        signal_group(pid, "-KILL");
                    }
                });
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .creation_flags(0x0800_0000)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.lock().0
    }

    /// Starts the operation's process with `spawn` and registers it, under the lock, so that
    /// a cancel can't miss it. Refused once cancelled.
    pub fn start(&self, spawn: impl FnOnce() -> std::io::Result<Child>) -> Result<Child, Start> {
        let mut state = self.lock();
        if state.0 {
            return Err(Start::Cancelled);
        }
        let child = spawn().map_err(Start::Spawn)?;
        state.1 = Some(child.id());
        Ok(child)
    }

    /// Forgets the process, which has exited. True if cancelled meanwhile.
    pub fn finish(&self) -> bool {
        let mut state = self.lock();
        state.1 = None;
        state.0
    }

    fn lock(&self) -> MutexGuard<'_, (bool, Option<u32>)> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(unix)]
fn signal_group(pid: u32, signal: &str) {
    let _ = std::process::Command::new("kill")
        .args([signal, "--", &format!("-{pid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn sleeper() -> Child {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "ping -n 30 127.0.0.1 > NUL"]);
            c
        } else {
            let mut c = Command::new("sleep");
            c.arg("30");
            c
        };
        cmd.stdout(Stdio::null())
            .spawn()
            .expect("a process to hold")
    }

    #[test]
    fn a_held_child_is_killed_by_cancel_and_none_is_held_after() {
        let cancel = Cancel::new();
        cancel.hold(sleeper()).unwrap();
        assert!(matches!(cancel.poll(), Poll::Running));
        cancel.cancel();
        assert!(matches!(cancel.poll(), Poll::Gone));
        assert!(cancel.is_cancelled());
        assert_eq!(cancel.hold(sleeper()), Err(Cancelled));
        assert!(cancel.release().is_none());
    }

    #[test]
    fn an_exited_child_is_released_by_poll() {
        let cancel = Cancel::new();
        let mut child = Command::new(if cfg!(windows) { "cmd" } else { "true" });
        if cfg!(windows) {
            child.args(["/C", "exit 0"]);
        }
        cancel.hold(child.spawn().unwrap()).unwrap();
        let status = loop {
            match cancel.poll() {
                Poll::Running => std::thread::sleep(std::time::Duration::from_millis(5)),
                Poll::Exited(status) => break status,
                Poll::Gone => panic!("gone before exiting"),
            }
        };
        assert!(status.success());
        assert!(matches!(cancel.poll(), Poll::Gone));
        assert!(!cancel.is_cancelled());
    }

    #[test]
    fn a_tree_refuses_to_start_once_cancelled() {
        let tree = CancelTree::new();
        tree.cancel();
        assert!(matches!(tree.start(sleeper_result), Err(Start::Cancelled)));
        let tree = CancelTree::new();
        let mut child = tree.start(sleeper_result).unwrap();
        assert!(!tree.finish());
        let _ = child.kill();
        let _ = child.wait();
    }

    fn sleeper_result() -> std::io::Result<Child> {
        Ok(sleeper())
    }
}
