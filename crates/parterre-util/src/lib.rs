use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Kills a child process run on another thread, one at a time; once cancelled, none starts.
/// Closing the child's output would not do: git notices only when it next writes, which can be
/// after walking the whole history.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<Mutex<Running>>);

#[derive(Debug, Default)]
struct Running {
    cancelled: bool,
    child: Option<Child>,
}

#[derive(Debug)]
pub struct Pipes {
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
}

#[derive(Debug)]
pub enum Poll {
    /// Cancelled, released, or killed because its state couldn't be read.
    Gone,
    Running,
    Exited(ExitStatus),
}

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }

    pub fn cancel(&self) {
        let mut running = self.lock();
        running.cancelled = true;
        if let Some(mut child) = running.child.take() {
            kill_and_wait(&mut child);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.lock().cancelled
    }

    /// Starts `cmd` and holds the child; `None`, without starting it, once cancelled. It starts
    /// under the lock, so a concurrent [`cancel`](Cancel::cancel) kills the child rather than
    /// returning while it runs (#213).
    pub fn spawn(&self, cmd: &mut Command) -> std::io::Result<Option<Pipes>> {
        let mut running = self.lock();
        if running.cancelled {
            return Ok(None);
        }
        debug_assert!(running.child.is_none(), "one child at a time");
        let mut child = cmd.spawn()?;
        let pipes = Pipes {
            stdin: child.stdin.take(),
            stdout: child.stdout.take(),
            stderr: child.stderr.take(),
        };
        running.child = Some(child);
        Ok(Some(pipes))
    }

    /// Releases the held child once it has exited.
    pub fn poll(&self) -> Poll {
        let mut running = self.lock();
        let Some(child) = running.child.as_mut() else {
            return Poll::Gone;
        };
        let poll = match child.try_wait() {
            Ok(None) => return Poll::Running,
            Ok(Some(status)) => Poll::Exited(status),
            Err(_) => {
                kill_and_wait(child);
                Poll::Gone
            }
        };
        running.child = None;
        poll
    }

    /// Takes the held child back, to wait for it; `None` once cancelled.
    pub fn release(&self) -> Option<Child> {
        self.lock().child.take()
    }

    /// Kills the held child without cancelling the handle.
    pub fn kill(&self) {
        if let Some(mut child) = self.release() {
            kill_and_wait(&mut child);
        }
    }

    fn lock(&self) -> MutexGuard<'_, Running> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn kill_and_wait(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Cancels a git operation: its whole process group on Unix, or process tree on Windows, hooks
/// included. One operation at a time.
#[derive(Clone, Debug, Default)]
pub struct CancelTree(Arc<Mutex<Operation>>);

#[derive(Debug, Default)]
struct Operation {
    cancelled: bool,
    pid: Option<u32>,
}

#[derive(Debug)]
pub enum StartError {
    Cancelled,
    Spawn(std::io::Error),
}

impl CancelTree {
    pub fn cancel(&self) {
        let mut operation = self.lock();
        if operation.cancelled {
            return;
        }
        operation.cancelled = true;
        if let Some(pid) = operation.pid {
            #[cfg(unix)]
            {
                signal_group(pid, "-TERM");
                let pending = self.0.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    let operation = pending.lock().unwrap_or_else(PoisonError::into_inner);
                    // Still registered: its output is still open, maybe held by a hook that
                    // ignores TERM, even if git itself has exited.
                    if operation.pid == Some(pid) {
                        signal_group(pid, "-KILL");
                    }
                });
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .creation_flags(CREATE_NO_WINDOW)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.lock().cancelled
    }

    /// Starts and registers the operation's process under the lock, so that a cancel can't miss
    /// it. On Unix it leads a process group of its own, which the hooks git starts join.
    pub fn start(&self, cmd: &mut Command) -> Result<Child, StartError> {
        let mut operation = self.lock();
        if operation.cancelled {
            return Err(StartError::Cancelled);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let child = cmd.spawn().map_err(StartError::Spawn)?;
        operation.pid = Some(child.id());
        Ok(child)
    }

    /// Forgets the process; call it once the process has exited and its output pipes have
    /// closed. Returns whether it was cancelled.
    pub fn finish(&self) -> bool {
        let mut operation = self.lock();
        operation.pid = None;
        operation.cancelled
    }

    fn lock(&self) -> MutexGuard<'_, Operation> {
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
    use std::process::Stdio;

    fn sleeper_command() -> Command {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "ping -n 30 127.0.0.1 > NUL"]);
            c
        } else {
            let mut c = Command::new("sleep");
            c.arg("30");
            c
        };
        cmd.stdout(Stdio::null());
        cmd
    }

    #[test]
    fn a_started_child_is_killed_by_cancel_and_none_starts_after() {
        let cancel = Cancel::new();
        assert!(cancel.spawn(&mut sleeper_command()).unwrap().is_some());
        assert!(matches!(cancel.poll(), Poll::Running));
        cancel.cancel();
        assert!(matches!(cancel.poll(), Poll::Gone));
        assert!(cancel.is_cancelled());
        assert!(cancel.spawn(&mut sleeper_command()).unwrap().is_none());
        assert!(cancel.release().is_none());
    }

    #[test]
    fn an_exited_child_is_released_by_poll() {
        let cancel = Cancel::new();
        let mut child = Command::new(if cfg!(windows) { "cmd" } else { "true" });
        if cfg!(windows) {
            child.args(["/C", "exit 0"]);
        }
        assert!(cancel.spawn(&mut child).unwrap().is_some());
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
        let tree = CancelTree::default();
        tree.cancel();
        assert!(matches!(
            tree.start(&mut sleeper_command()),
            Err(StartError::Cancelled)
        ));
        let tree = CancelTree::default();
        let mut child = tree.start(&mut sleeper_command()).unwrap();
        assert!(!tree.finish());
        kill_and_wait(&mut child);
    }
}
