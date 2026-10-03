//! Merging a branch or commit into the open worktree's branch, by one of the merge methods:
//! a fast-forward (`git merge --ff-only`) or a merge commit (`git merge --no-ff`) with the
//! message the user agreed to, pre-filled as git words it. A merge that stops, on conflicts or
//! because a hook refused to commit it, leaves the worktree with an operation in progress,
//! finished with git for now.

use std::path::Path;

use crate::branches::{Attention, Cancel, Catalog, Error, Report, Stuck, run};
use crate::git::Git;
use crate::log::is_ancestor;
use crate::{Oid, Repo};

/// How the branch takes in the other's commits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Moves the branch up to the other: `--ff-only`.
    FastForward,
    /// Joins the other in with a merge commit, even where a fast-forward would do: `--no-ff`.
    MergeCommit,
}

impl Method {
    pub const ALL: [Method; 2] = [Method::FastForward, Method::MergeCommit];

    pub fn name(self) -> &'static str {
        match self {
            Method::FastForward => "Fast-forward",
            Method::MergeCommit => "Merge commit",
        }
    }

    pub fn flag(self) -> &'static str {
        match self {
            Method::FastForward => "--ff-only",
            Method::MergeCommit => "--no-ff",
        }
    }

    /// What it does to `branch`, merging `target`.
    pub fn help(self, branch: &str, target: &str) -> String {
        match self {
            Method::FastForward => format!("Moves {branch} up to {target}."),
            Method::MergeCommit => format!("Joins {target} into {branch} with a merge commit."),
        }
    }
}

/// A merge the user agreed to. It runs only while the branch is still where it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merge {
    pub branch: String,
    /// Where the branch was.
    pub head: Oid,
    /// The commit merged in.
    pub theirs: Oid,
    /// What the command names it by: a branch's name, or the full hash.
    pub target: String,
    pub method: Method,
    /// The merge commit's message; unused by a fast-forward.
    pub message: String,
    /// `--autostash` (`Some(true)`) or `--no-autostash` (`Some(false)`), where it differs from
    /// what `merge.autoStash` does.
    pub autostash: Option<bool>,
    /// `merge.log` is set: `--no-log`, since the message already has what it would add.
    pub no_log: bool,
}

/// The command a merge runs.
pub fn command(merge: &Merge) -> Vec<String> {
    let mut args = vec!["merge".to_owned(), merge.method.flag().to_owned()];
    match merge.autostash {
        Some(true) => args.push("--autostash".into()),
        Some(false) => args.push("--no-autostash".into()),
        None => {}
    }
    if merge.method == Method::MergeCommit {
        if merge.no_log {
            args.push("--no-log".into());
        }
        args.push("-m".into());
        args.push(merge.message.clone());
    }
    args.push(merge.target.clone());
    args
}

/// The branch a merge of `theirs` would go into, when parterre offers one: the open worktree's
/// branch, while no operation is in progress there, when `theirs` has commits it lacks.
pub fn offered<'a>(repo: &Repo, catalog: &'a Catalog, theirs: Oid) -> Option<&'a str> {
    if !catalog.has_working_tree || catalog.stuck().is_some() {
        return None;
    }
    let branch = catalog.current.as_deref()?;
    let head = repo.lookup(&catalog.head?)?;
    let theirs = repo.lookup(&theirs)?;
    (!is_ancestor(repo, theirs, head)).then_some(branch)
}

/// What a merge of a commit would do, for its dialog.
#[derive(Clone, Debug)]
pub struct Preview {
    pub branch: String,
    pub head: Oid,
    pub theirs: Oid,
    /// The branch has no commits of its own since `theirs`: it can fast-forward.
    pub fast_forward: bool,
    /// Uncommitted changes to tracked files.
    pub dirty: bool,
    /// `merge.autoStash` is set.
    pub auto_stash: bool,
    /// `merge.ff` is `false`: the user wants a merge commit even where a fast-forward would do.
    pub no_ff: bool,
    /// `merge.log` is set.
    pub log: bool,
    /// The merge commit's message as git would write it.
    pub message: String,
}

impl Preview {
    /// The merge of `theirs`, named by `target`: a branch's name, or the full hash.
    pub fn load(path: &Path, theirs: Oid, target: &str) -> Result<Preview, Error> {
        let catalog = Catalog::load(path)?;
        if let Some(stuck) = catalog.stuck() {
            return Err(Error::Invalid(format!("{}.", stuck.reason())));
        }
        let (Some(branch), Some(head)) = (catalog.current.clone(), catalog.head) else {
            return Err(Error::Invalid(
                "There's no branch checked out to merge into.".into(),
            ));
        };
        let git = Git::new(&catalog.root);
        let fast_forward = git
            .query(&[
                "merge-base",
                "--is-ancestor",
                &head.to_hex(),
                &theirs.to_hex(),
            ])?
            .is_some();
        let dirty = !git
            .run(&["status", "--porcelain", "--untracked-files=no"])?
            .trim()
            .is_empty();
        let config = |key: &str| {
            git.query(&["config", "--get", key])
                .ok()
                .flatten()
                .map(|v| v.trim().to_ascii_lowercase())
        };
        let truthy = |v: &Option<String>| {
            v.as_deref()
                .is_some_and(|v| !matches!(v, "false" | "no" | "off" | "0" | ""))
        };
        // How `git merge` names what it merges, for `fmt-merge-msg`: branches "of" the
        // repository itself, so `merge.log` heads their commits with the bare name.
        let what = if catalog.locals.iter().any(|b| b.name == target) {
            format!("branch '{target}' of .")
        } else if catalog.remotes.iter().any(|b| b.name == target) {
            format!("remote-tracking branch '{target}' of .")
        } else {
            let short = git.run(&["rev-parse", "--short", &theirs.to_hex()])?;
            format!("commit '{}'", short.trim())
        };
        let line = format!("{}\t\t{what}\n", theirs.to_hex());
        let message = git.run_with_input(&["fmt-merge-msg"], line)?;
        Ok(Preview {
            branch,
            head,
            theirs,
            fast_forward,
            dirty,
            auto_stash: truthy(&config("merge.autoStash")),
            no_ff: config("merge.ff").as_deref() == Some("false"),
            log: truthy(&config("merge.log")),
            message: strip_comments(&message),
        })
    }

    /// The method picked to begin with: a fast-forward where one would do, unless `merge.ff`
    /// says otherwise.
    pub fn default_method(&self) -> Method {
        if self.fast_forward && !self.no_ff {
            Method::FastForward
        } else {
            Method::MergeCommit
        }
    }

    /// Why `method` makes no sense for this merge, if it doesn't.
    pub fn unavailable(&self, method: Method, target: &str) -> Option<String> {
        (method == Method::FastForward && !self.fast_forward)
            .then(|| format!("{} has commits {target} doesn't", self.branch))
    }

    /// Why the merge can't start as chosen.
    pub fn blocked(
        &self,
        method: Method,
        stash: bool,
        message: &str,
        target: &str,
    ) -> Option<String> {
        if let Some(why) = self.unavailable(method, target) {
            return Some(format!("{why}."));
        }
        if method == Method::FastForward {
            // It carries the changes along, or git refuses without losing any.
            return None;
        }
        if self.dirty && !stash {
            // `git merge --abort` can't always put them back.
            return Some("Commit or stash your changes first.".into());
        }
        message
            .trim()
            .is_empty()
            .then(|| "Enter a message for the merge commit.".into())
    }

    /// The merge, with the target named by `target`, by `method`, with the changes stashed or
    /// not.
    pub fn merge(&self, target: String, method: Method, stash: bool, message: &str) -> Merge {
        let autostash = match (self.dirty, stash, self.auto_stash) {
            (true, true, false) => Some(true),
            (true, false, true) => Some(false),
            _ => None,
        };
        Merge {
            branch: self.branch.clone(),
            head: self.head,
            theirs: self.theirs,
            target,
            method,
            message: message.to_owned(),
            autostash,
            no_log: self.log,
        }
    }
}

/// Git's own merge does without the comment lines `fmt-merge-msg` adds for an editor.
fn strip_comments(message: &str) -> String {
    let lines: Vec<&str> = message
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(str::trim_end)
        .collect();
    let mut text = lines.join("\n");
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    text.trim().to_owned()
}

/// Runs the merge if the branch is still where it was. A stop, on conflicts or because a hook
/// refused to commit, or changes the autostash couldn't put back, come back as
/// [`Report::attention`].
pub(crate) fn execute(
    catalog: &Catalog,
    merge: &Merge,
    cancel: &Cancel,
    report: &mut Report,
) -> Result<(), Error> {
    if catalog.current.as_deref() != Some(merge.branch.as_str()) || catalog.head != Some(merge.head)
    {
        return Err(Error::Invalid(format!(
            "Branch {} moved, or is no longer checked out here. Reload and try again.",
            merge.branch
        )));
    }
    if let Some(stuck) = catalog.stuck() {
        return Err(Error::Invalid(format!("{}.", stuck.reason())));
    }
    let git = Git::new(&catalog.root);
    let stashes = || {
        git.run(&["stash", "list"])
            .map(|out| out.lines().count())
            .unwrap_or(0)
    };
    let before = stashes();
    let ok = run(&git, command(merge), cancel, report)?;
    let after = Catalog::load(&catalog.root)?;
    if let Some(Stuck::InProgress(_)) = after.stuck() {
        let n = after.conflicted.len();
        // Git keeps autostashed changes aside until the merge is committed or aborted.
        let aside = git
            .query(&["rev-parse", "--verify", "--quiet", "MERGE_AUTOSTASH"])
            .ok()
            .flatten()
            .is_some();
        let mut message = "Finish or abort it with git, or go to another worktree.".to_owned();
        if aside {
            message.push_str(" Your changes are set aside until then.");
        }
        report.attention = Some(Attention {
            title: if n == 0 {
                "Merge not committed".into()
            } else {
                format!("Merge stopped on conflicts in {}", plural(n, "file"))
            },
            message,
        });
        return Ok(());
    }
    if !ok {
        return Err(Error::Failed(report.steps.last().unwrap().output.clone()));
    }
    if stashes() > before {
        // Git applied what it could, with conflict markers, and kept the stash entry too.
        let message = match after.conflicted.len() {
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
        report.attention = Some(Attention {
            title: format!("Merged {} into {}", short_target(merge), merge.branch),
            message,
        });
    }
    Ok(())
}

/// The target as the menus name it: a branch, or a short hash.
pub fn short_target(merge: &Merge) -> String {
    if merge.target == merge.theirs.to_hex() {
        merge.theirs.to_hex()[..7].to_owned()
    } else {
        merge.target.clone()
    }
}

fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}
