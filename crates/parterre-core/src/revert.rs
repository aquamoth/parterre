//! Reverting a commit on the open worktree's branch: a new commit at its tip that undoes it,
//! with the message git words for it. A merge is reverted against its first parent. Git's
//! `revert` has no `--autostash`, so parterre stashes uncommitted changes itself when asked, and
//! leaves putting them back to the user. A revert that stops on conflicts leaves an operation
//! in progress, finished with git for now.

use std::path::Path;

use crate::branches::{Attention, Cancel, Catalog, Error, Report, Stashed, Stuck, run, run_with};
use crate::git::Git;
use crate::log::is_ancestor;
use crate::{Oid, Repo};

/// A revert the user agreed to. It runs only while the branch is still where it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revert {
    /// The open worktree's branch; `None` when its HEAD is detached.
    pub branch: Option<String>,
    /// Where it was.
    pub head: Oid,
    pub commit: Oid,
    /// The commit is a merge, reverted against its first parent: `-m 1`.
    pub merge: bool,
    /// The message, handed to git through its editor; `None` lets git word it (`--no-edit`).
    pub message: Option<String>,
    /// Stash the uncommitted changes first.
    pub stash: bool,
}

impl Revert {
    /// The branch, or `HEAD` when detached.
    pub fn name(&self) -> &str {
        self.branch.as_deref().unwrap_or("HEAD")
    }
}

/// The revert itself.
pub fn command(revert: &Revert) -> Vec<String> {
    let mut args = vec!["revert".to_owned()];
    args.push(
        if revert.message.is_some() {
            "--edit"
        } else {
            "--no-edit"
        }
        .into(),
    );
    if revert.merge {
        args.extend(["-m".to_owned(), "1".to_owned()]);
    }
    args.push(revert.commit.to_hex());
    args
}

/// The stash, if any, then the revert.
pub fn commands(revert: &Revert) -> Vec<Vec<String>> {
    let mut steps = Vec::new();
    if revert.stash {
        steps.push(stash_command(revert.commit));
    }
    steps.push(command(revert));
    steps
}

fn stash_command(commit: Oid) -> Vec<String> {
    ["stash", "push", "-m", &stash_message(commit)]
        .map(str::to_owned)
        .to_vec()
}

/// The stash entry's message, by which a stuck revert finds it again.
fn stash_message(commit: Oid) -> String {
    format!("autostash for reverting {}", &commit.to_hex()[..7])
}

/// The entry, such as `stash@{0}`, parterre stashed the changes in before reverting `commit`.
pub(crate) fn stash_entry(git: &Git, commit: Oid) -> Option<String> {
    let suffix = format!(": {}", stash_message(commit));
    git.run(&["stash", "list", "--format=%gd%x00%gs"])
        .ok()?
        .lines()
        .filter_map(|l| l.split_once('\0'))
        .find(|(_, subject)| subject.ends_with(&suffix))
        .map(|(name, _)| name.to_owned())
}

/// What the menu names the branch reverted on, when it offers reverting `commit`: the open
/// worktree's branch, or `HEAD` when detached, where HEAD reaches `commit`.
pub fn offered(repo: &Repo, catalog: &Catalog, commit: Oid) -> Option<String> {
    if !catalog.has_working_tree {
        return None;
    }
    let head = repo.lookup(&catalog.head?)?;
    let commit = repo.lookup(&commit)?;
    is_ancestor(repo, commit, head).then(|| catalog.current.clone().unwrap_or("HEAD".into()))
}

/// What a revert would do, for its dialog.
#[derive(Clone, Debug)]
pub struct Preview {
    /// The open worktree's branch; `None` when detached.
    pub branch: Option<String>,
    pub head: Oid,
    pub commit: Oid,
    /// The commit is a merge, reverted against its first parent.
    pub merge: bool,
    /// The message as git would write it.
    pub message: String,
    /// `revert.reference` is set: git leaves the title line for the user to say why.
    pub reference: bool,
    /// Tracked files with uncommitted changes, staged or not.
    pub changed: Vec<String>,
    /// Those with staged changes: git refuses to revert over any.
    pub staged: Vec<String>,
    /// Those the revert changes too: git refuses to revert over them.
    pub overlap: Vec<String>,
}

impl Preview {
    /// The revert of `commit` on the open worktree's branch.
    pub fn load(path: &Path, commit: Oid) -> Result<Preview, Error> {
        let catalog = Catalog::load(path)?;
        if let Some(stuck) = catalog.stuck() {
            return Err(Error::Invalid(format!("{}.", stuck.reason())));
        }
        let Some(head) = catalog.head.filter(|_| catalog.has_working_tree) else {
            return Err(Error::Invalid(
                "There's nothing checked out to revert on.".into(),
            ));
        };
        let git = Git::new(&catalog.root);
        let hex = commit.to_hex();
        if !is_ancestor_git(&git, commit, head)? {
            return Err(Error::Invalid(format!(
                "{} is not on {}. Reload and try again.",
                &hex[..7],
                catalog.current.as_deref().unwrap_or("HEAD")
            )));
        }
        let parents = git.run(&["rev-list", "--parents", "-n", "1", &hex])?;
        let merge = parents.split_whitespace().count() > 2;
        let reference = git
            .query(&["config", "--type=bool", "--get", "revert.reference"])?
            .is_some_and(|v| v.trim() == "true");
        let message = message(&git, commit, merge, reference)?;
        let (changed, staged) = local_changes(&catalog.root)?;
        let touched = touched(&git, commit)?;
        let overlap = changed
            .iter()
            .filter(|f| touched.contains(f))
            .cloned()
            .collect();
        Ok(Preview {
            branch: catalog.current.clone(),
            head,
            commit,
            merge,
            message,
            reference,
            changed,
            staged,
            overlap,
        })
    }

    /// The branch, or `HEAD` when detached.
    pub fn name(&self) -> &str {
        self.branch.as_deref().unwrap_or("HEAD")
    }

    /// *Stash before revert* is offered: there are uncommitted changes to stash.
    pub fn stashable(&self) -> bool {
        !self.changed.is_empty()
    }

    /// The files git would refuse to revert over unstashed: every one with staged changes,
    /// and those the revert changes too.
    pub fn refused(&self, stash: bool) -> Vec<String> {
        if stash && self.stashable() {
            return Vec::new();
        }
        let mut files = self.staged.clone();
        files.extend(self.overlap.iter().cloned());
        files.sort();
        files.dedup();
        files
    }

    /// Why git would refuse the revert unstashed.
    pub fn refusal(&self, stash: bool) -> Option<String> {
        let files = self.refused(stash);
        if files.is_empty() {
            return None;
        }
        let short = &self.commit.to_hex()[..7];
        Some(if self.overlap.is_empty() {
            format!(
                "{} staged changes",
                plural(files.len(), "file has", "files have")
            )
        } else {
            format!(
                "{short} changes {} with local changes",
                plural(files.len(), "file", "files")
            )
        })
    }

    /// The local changes the revert runs on top of, unstashed, when git lets it.
    pub fn caution(&self, stash: bool) -> Option<String> {
        (self.stashable() && !stash && self.refusal(false).is_none()).then(|| {
            format!(
                "{} with local changes {} in the worktree",
                plural(self.changed.len(), "file", "files"),
                if self.changed.len() == 1 {
                    "stays"
                } else {
                    "stay"
                }
            )
        })
    }

    /// Why the revert can't start as asked.
    pub fn blocked(&self, stash: bool, message: &str) -> Option<String> {
        if let Some(why) = self.refusal(stash) {
            return Some(format!("{why}."));
        }
        let title = message.lines().next().unwrap_or("").trim();
        if title.is_empty() {
            return Some(if message.trim().is_empty() {
                "Enter a message for the revert.".into()
            } else {
                "Say why you're reverting on the title line.".into()
            });
        }
        None
    }

    /// The revert, with `message` and the changes stashed or not.
    pub fn revert(&self, stash: bool, message: &str) -> Revert {
        // Git words it as shown, unless the user changed it. Git before 2.43 words a revert's
        // revert otherwise, and with `revert.reference` leaves a comment for the title.
        let own = message.trim_end() == self.message.trim_end()
            && !self.reference
            && !self.message.starts_with("Reapply \"");
        Revert {
            branch: self.branch.clone(),
            head: self.head,
            commit: self.commit,
            merge: self.merge,
            message: (!own).then(|| format!("{}\n", message.trim_end())),
            stash: stash && self.stashable(),
        }
    }
}

/// The message `git revert` writes for `commit`: `Revert "<subject>"`, or `Reapply` for a
/// revert's revert, then the commit reverted, and for a merge the parent it's reverted to.
/// With `revert.reference`, the title is the user's to write and commits are named by
/// `--pretty=reference`.
fn message(git: &Git, commit: Oid, merge: bool, reference: bool) -> Result<String, Error> {
    let hex = commit.to_hex();
    let refer = |rev: &str| -> Result<String, Error> {
        Ok(if reference {
            git.run(&["show", "-s", "--pretty=reference", rev, "--"])?
                .trim()
                .to_owned()
        } else {
            git.run(&["rev-parse", "--verify", &format!("{rev}^{{commit}}")])?
                .trim()
                .to_owned()
        })
    };
    let subject = git.run(&["show", "-s", "--format=%B", &hex, "--"])?;
    let subject = subject.lines().next().unwrap_or("").trim_end();
    let title = if reference {
        String::new()
    } else if let Some(original) = subject.strip_prefix("Revert \"")
        && !original.starts_with("Revert \"")
    {
        // As git 2.43 and later word it; git's own nesting stops at one level.
        format!("Reapply \"{original}")
    } else {
        format!("Revert \"{subject}\"")
    };
    let mut text = format!("{title}\n\nThis reverts commit {}", refer(&hex)?);
    if merge {
        text.push_str(&format!(
            ", reversing\nchanges made to {}",
            refer(&format!("{hex}^1"))?
        ));
    }
    text.push_str(".\n");
    Ok(text)
}

/// The files the revert of `commit` changes: those the commit changed from its first parent.
fn touched(git: &Git, commit: Oid) -> Result<Vec<String>, Error> {
    let out = git.run(&[
        "diff-tree",
        "-r",
        "-z",
        "--no-commit-id",
        "--name-only",
        "--root",
        "-m",
        "--first-parent",
        &commit.to_hex(),
        "--",
    ])?;
    Ok(out
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect())
}

/// The tracked files with uncommitted changes in the worktree at `dir`, and those of them
/// with staged changes. A rename counts by both its names.
fn local_changes(dir: &Path) -> Result<(Vec<String>, Vec<String>), Error> {
    let out = Git::new(dir).run(&["status", "--porcelain=v1", "-z", "--untracked-files=no"])?;
    let (mut changed, mut staged) = (Vec::new(), Vec::new());
    let mut entries = out.split('\0').filter(|e| !e.is_empty());
    while let Some(entry) = entries.next() {
        let (Some(x), Some(path)) = (entry.chars().next(), entry.get(3..)) else {
            continue;
        };
        let mut paths = vec![path.to_owned()];
        if matches!(x, 'R' | 'C')
            && let Some(from) = entries.next()
        {
            paths.push(from.to_owned());
        }
        for path in paths {
            if x != ' ' && !staged.contains(&path) {
                staged.push(path.clone());
            }
            if !changed.contains(&path) {
                changed.push(path);
            }
        }
    }
    changed.sort();
    staged.sort();
    Ok((changed, staged))
}

fn is_ancestor_git(git: &Git, ancestor: Oid, of: Oid) -> Result<bool, Error> {
    Ok(git
        .query(&[
            "merge-base",
            "--is-ancestor",
            &ancestor.to_hex(),
            &of.to_hex(),
        ])?
        .is_some())
}

fn stash_top(git: &Git) -> Option<String> {
    git.query(&["rev-parse", "--verify", "--quiet", "refs/stash"])
        .ok()
        .flatten()
        .map(|s| s.trim().to_owned())
}

/// Runs the revert if the branch is still where it was, stashing first if asked. A stop on
/// conflicts, or a revert with nothing to undo, comes back as [`Report::attention`]; the stash
/// entry made, unless the revert stopped, as [`Report::stash`], for the user to restore.
pub(crate) fn execute(
    catalog: &Catalog,
    revert: &Revert,
    cancel: &Cancel,
    report: &mut Report,
) -> Result<(), Error> {
    let name = revert.name();
    if catalog.current != revert.branch || catalog.head != Some(revert.head) {
        return Err(Error::Invalid(format!(
            "{name} moved, or is no longer checked out here. Reload and try again."
        )));
    }
    if let Some(stuck) = catalog.stuck() {
        return Err(Error::Invalid(format!("{}.", stuck.reason())));
    }
    let git = Git::new(&catalog.root);
    let short = revert.commit.to_hex()[..7].to_owned();
    if !is_ancestor_git(&git, revert.commit, revert.head)? {
        return Err(Error::Invalid(format!("{short} is not on {name}.")));
    }
    let (changed, _) = local_changes(&catalog.root)?;
    if revert.stash && !changed.is_empty() {
        let before = stash_top(&git);
        if !run(&git, stash_command(revert.commit), cancel, report)? {
            return Err(Error::Failed(report.steps.last().unwrap().output.clone()));
        }
        if let Some(top) = stash_top(&git).filter(|top| Some(top) != before.as_ref())
            && let Some(oid) = Oid::from_hex(&top)
        {
            let files = git.run(&["stash", "show", "--name-only", "-z", &top])?;
            let files = files
                .split('\0')
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect();
            let name = stash_entry(&git, revert.commit).unwrap_or_else(|| "stash@{0}".into());
            report.stash = Some(Stashed { oid, name, files });
        }
    } else if !revert.stash {
        // What the dialog saw may have changed: git would refuse, or the user would expect
        // the changes stashed.
        let (_, staged) = local_changes(&catalog.root)?;
        let touched = touched(&git, revert.commit)?;
        if !staged.is_empty() || changed.iter().any(|f| touched.contains(f)) {
            return Err(Error::Invalid(
                "Your local changes changed. Reload and try again.".into(),
            ));
        }
    }
    let ok = match &revert.message {
        None => run(&git, command(revert), cancel, report)?,
        Some(message) => {
            // Git asks its editor for the message: copy ours over it.
            let dir = git.run(&["rev-parse", "--absolute-git-dir"])?;
            let path = Path::new(dir.trim()).join("parterre-revert-message");
            std::fs::write(&path, message)
                .map_err(|e| Error::Invalid(format!("Couldn't write the message: {e}")))?;
            // Git runs it with sh, Git for Windows' too, which takes forward slashes.
            let quoted = path
                .to_string_lossy()
                .replace('\\', "/")
                .replace('\'', r"'\''");
            let editor = format!("cp '{quoted}'");
            let ok = run_with(
                &git,
                command(revert),
                &[("GIT_EDITOR", &editor)],
                cancel,
                report,
            );
            let _ = std::fs::remove_file(&path);
            ok?
        }
    };
    let after = Catalog::load(&catalog.root)?;
    let moved = after.head != Some(revert.head);
    if !ok
        && !moved
        && after.conflicted.is_empty()
        && index_clean(&git)?
        && !untracked_in_the_way(&git, revert.commit)?
    {
        // The commit's changes are undone already. Git 2.34 leaves the revert in progress
        // then; later versions don't.
        if matches!(after.stuck(), Some(Stuck::InProgress(_))) {
            run(
                &git,
                ["revert", "--abort"].map(str::to_owned).to_vec(),
                cancel,
                report,
            )?;
        }
        report.attention = Some(Attention {
            title: format!("Nothing to revert: {short}'s changes are already undone"),
            message: String::new(),
        });
        return Ok(());
    }
    if let Some(Stuck::InProgress(_)) = after.stuck() {
        let n = after.conflicted.len();
        let mut message = "Finish or abort it with git, or go to another worktree.".to_owned();
        if let Some(stashed) = report.stash.take() {
            let entry = stash_entry(&git, revert.commit).unwrap_or_else(|| stashed.oid.to_hex());
            message.push_str(&format!(" Your changes are stashed in {entry}."));
        }
        report.attention = Some(Attention {
            title: if n == 0 {
                "Revert not committed".into()
            } else {
                format!(
                    "Revert stopped on conflicts in {}",
                    plural(n, "file", "files")
                )
            },
            message,
        });
        return Ok(());
    }
    if !ok {
        return Err(Error::Failed(report.steps.last().unwrap().output.clone()));
    }
    report.created = after.head;
    Ok(())
}

/// Untracked files the revert would write over, which git refuses to without a conflict.
fn untracked_in_the_way(git: &Git, commit: Oid) -> Result<bool, Error> {
    let touched = touched(git, commit)?;
    if touched.is_empty() {
        return Ok(false);
    }
    let mut args = vec!["ls-files", "--others", "--exclude-standard", "--"];
    args.extend(touched.iter().map(String::as_str));
    Ok(!git.run(&args)?.trim().is_empty())
}

/// The index matches HEAD.
fn index_clean(git: &Git) -> Result<bool, Error> {
    Ok(git
        .query(&["diff", "--cached", "--quiet", "HEAD", "--"])?
        .is_some())
}

/// Puts back the changes in the stash entry `stash`, wherever it is in the list now, and drops
/// it, as `git stash pop` does. Changes that conflict keep the entry, and come back as
/// [`Report::attention`].
pub(crate) fn restore(
    catalog: &Catalog,
    stash: Oid,
    cancel: &Cancel,
    report: &mut Report,
) -> Result<(), Error> {
    if let Some(stuck) = catalog.stuck() {
        return Err(Error::Invalid(format!("{}.", stuck.reason())));
    }
    let git = Git::new(&catalog.root);
    let hex = stash.to_hex();
    let entry = git
        .run(&["stash", "list", "--format=%gd%x00%H"])?
        .lines()
        .filter_map(|l| l.split_once('\0'))
        .find(|(_, oid)| *oid == hex)
        .map(|(name, _)| name.to_owned())
        .ok_or_else(|| {
            Error::Invalid(format!(
                "The stash entry {} is gone from the stash list.",
                &hex[..7]
            ))
        })?;
    let ok = run(
        &git,
        ["stash", "pop", &entry].map(str::to_owned).to_vec(),
        cancel,
        report,
    )?;
    let after = Catalog::load(&catalog.root)?;
    if !after.conflicted.is_empty() {
        let n = after.conflicted.len();
        report.attention = Some(Attention {
            title: format!(
                "Restoring stashed changes conflicted in {}",
                plural(n, "file", "files")
            ),
            message: format!(
                "Resolve {} with git. Your changes are also kept in {entry} until you drop it.",
                if n == 1 { "it" } else { "them" }
            ),
        });
        return Ok(());
    }
    if !ok {
        return Err(Error::Failed(report.steps.last().unwrap().output.clone()));
    }
    Ok(())
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
