use std::io::Read;
use std::path::Path;

use parterre_util::CancelTree;

use crate::Oid;
use crate::branches::{Attention, Catalog, Error, Report, Step};
use crate::git::{Git, GitError};
use crate::repo::DEFAULT_ABBREV_LEN;
use crate::text::plural;

const FINISH_OR_ABORT: &str = "Finish or abort it with git, or go to another worktree.";

pub(crate) fn git_args(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| (*w).to_owned()).collect()
}

/// Runs an operation's git command, recording it in `report`: whether git succeeded.
pub(crate) fn run(
    git: &Git,
    args: Vec<String>,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<bool, Error> {
    run_with(git, args, &[], cancel, report)
}

/// [`run`], failing with what git printed.
pub(crate) fn run_or_fail(
    git: &Git,
    args: Vec<String>,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<(), Error> {
    if run(git, args, cancel, report)? {
        Ok(())
    } else {
        Err(Error::Failed(report.last_output()))
    }
}

/// [`run`], with `env` set for git.
pub(crate) fn run_with(
    git: &Git,
    args: Vec<String>,
    env: &[(&str, &str)],
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<bool, Error> {
    let mut child = cancel.start(git.operation_command(&args).envs(env.iter().copied()))?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let errors = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });
    let mut output = Vec::new();
    let read_result = stdout.read_to_end(&mut output);
    let error = errors.join().unwrap_or_default();
    let status = child.wait();
    let cancelled = cancel.finish();
    let status = status.map_err(GitError::Spawn)?;
    output.extend(error);
    report.steps.push(Step {
        args,
        output: String::from_utf8_lossy(&output).trim().to_owned(),
        success: status.success(),
    });
    if cancelled {
        return Err(Error::Cancelled);
    }
    read_result.map_err(GitError::Spawn)?;
    Ok(status.success())
}

/// A file git asks an editor for while it runs, which parterre writes itself.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Edited {
    /// An interactive rebase's.
    TodoList,
    CommitMessage,
}

/// [`run`], with an editor that copies `text` over the file git asks it to edit.
pub(crate) fn run_with_edited(
    git: &Git,
    args: Vec<String>,
    edited: Edited,
    text: &str,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<bool, Error> {
    let (variable, file_name, what) = match edited {
        Edited::TodoList => (
            "GIT_SEQUENCE_EDITOR",
            "parterre-rebase-todo",
            "the todo list",
        ),
        Edited::CommitMessage => ("GIT_EDITOR", "parterre-revert-message", "the message"),
    };
    let dir = git.run(&["rev-parse", "--absolute-git-dir"])?;
    let path = Path::new(dir.trim()).join(file_name);
    std::fs::write(&path, text)
        .map_err(|e| Error::Invalid(format!("Couldn't write {what}: {e}")))?;
    // Git runs it with sh, Git for Windows' too, which takes forward slashes.
    let quoted = path
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', r"'\''");
    let editor = format!("cp '{quoted}'");
    let ran = run_with(git, args, &[(variable, &editor)], cancel, report);
    let _ = std::fs::remove_file(&path);
    ran
}

fn moved(branch: &str) -> Error {
    Error::Invalid(format!(
        "Branch {branch} moved, or is no longer checked out here. Reload and try again."
    ))
}

/// Fails unless the open worktree still has `branch` checked out at `head`.
pub(crate) fn refuse_if_moved(catalog: &Catalog, branch: &str, head: Oid) -> Result<(), Error> {
    if catalog.current.as_deref() == Some(branch) && catalog.head == Some(head) {
        Ok(())
    } else {
        Err(moved(branch))
    }
}

pub(crate) fn refuse_if_stuck(catalog: &Catalog) -> Result<(), Error> {
    match catalog.stuck() {
        Some(stuck) => Err(Error::Invalid(format!("{}.", stuck.reason()))),
        None => Ok(()),
    }
}

/// [`refuse_if_moved`], where a repository without a working tree counts as moved, then
/// [`refuse_if_stuck`].
pub(crate) fn refuse_if_moved_or_stuck(
    catalog: &Catalog,
    branch: &str,
    head: Oid,
) -> Result<(), Error> {
    if !catalog.has_working_tree {
        return Err(moved(branch));
    }
    refuse_if_moved(catalog, branch, head)?;
    refuse_if_stuck(catalog)
}

pub(crate) fn verify_commit(git: &Git, oid: Oid) -> Result<(), Error> {
    if git.resolve_commit(&oid.to_hex())?.is_none() {
        return Err(Error::Invalid(
            "The commit is no longer in the repository. Reload and try again.".into(),
        ));
    }
    Ok(())
}

/// Uncommitted changes to tracked files.
pub(crate) fn has_tracked_changes(git: &Git) -> Result<bool, GitError> {
    Ok(!git
        .run(&["status", "--porcelain", "--untracked-files=no"])?
        .trim()
        .is_empty())
}

pub(crate) fn stash_count(git: &Git) -> usize {
    git.run(&["stash", "list"])
        .map(|out| out.lines().count())
        .unwrap_or(0)
}

/// What the menus name an operation's target by: a branch, or a short hash for a commit named
/// by its full hash.
pub(crate) fn short_target(target: &str, commit: Oid) -> String {
    if target == commit.to_hex() {
        commit.short(DEFAULT_ABBREV_LEN)
    } else {
        target.to_owned()
    }
}

/// "Rebase stopped on conflicts in 2 files", or "Rebase stopped".
pub(crate) fn stopped(operation: &str, conflicted: usize) -> String {
    match conflicted {
        0 => format!("{operation} stopped"),
        n => format!("{operation} stopped on conflicts in {}", plural(n, "file")),
    }
}

/// [`stopped`], except that with nothing conflicted a hook refused to commit.
pub(crate) fn stopped_or_not_committed(operation: &str, conflicted: usize) -> String {
    match conflicted {
        0 => format!("{operation} not committed"),
        n => stopped(operation, n),
    }
}

/// Tells the user to finish or abort the operation that stopped.
pub(crate) fn in_progress(title: String) -> Attention {
    Attention {
        title,
        message: FINISH_OR_ABORT.to_owned(),
    }
}

/// Stashed changes that couldn't be put back: git applied what it could, with conflict
/// markers, and kept the stash entry too.
pub(crate) fn kept_stash(title: String, conflicted: usize) -> Attention {
    let message = match conflicted {
        0 => "Git couldn't put your changes back, and kept them in a stash entry. \
              Recover them with git stash pop."
            .to_owned(),
        n => format!(
            "Putting your changes back conflicted in {}. Resolve {} with git; your \
             changes are also kept in a stash entry until you drop it.",
            plural(n, "file"),
            if n == 1 { "it" } else { "them" }
        ),
    };
    Attention { title, message }
}
