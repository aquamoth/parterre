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
    /// PROTOTYPE (#188): replays the other's commits on top of the branch, then fast-forwards.
    RebaseFastForward,
    /// PROTOTYPE (#188): replays the other's commits on top of the branch, then `--no-ff`.
    SemiLinear,
}

impl Method {
    pub const ALL: [Method; 4] = [
        Method::FastForward,
        Method::MergeCommit,
        Method::RebaseFastForward,
        Method::SemiLinear,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Method::FastForward => "Fast-forward",
            Method::MergeCommit => "Merge commit",
            Method::RebaseFastForward => "Rebase and fast-forward",
            Method::SemiLinear => "Semi-linear merge",
        }
    }

    pub fn flag(self) -> &'static str {
        match self {
            Method::FastForward | Method::RebaseFastForward => "--ff-only",
            Method::MergeCommit | Method::SemiLinear => "--no-ff",
        }
    }

    /// PROTOTYPE (#188): it rebases first.
    pub fn rebases(self) -> bool {
        matches!(self, Method::RebaseFastForward | Method::SemiLinear)
    }

    /// It makes a merge commit, with a message.
    pub fn commits(self) -> bool {
        matches!(self, Method::MergeCommit | Method::SemiLinear)
    }

    /// What it does to `branch`, merging `target`.
    pub fn help(self, branch: &str, target: &str) -> String {
        match self {
            Method::FastForward => format!("Moves {branch} up to {target}."),
            Method::MergeCommit => format!("Joins {target} into {branch} with a merge commit."),
            Method::RebaseFastForward => format!(
                "Replays {target}'s commits on top of {branch}, then moves {branch} up to them."
            ),
            Method::SemiLinear => format!(
                "Replays {target}'s commits on top of {branch}, then joins them with a merge commit."
            ),
        }
    }
}

/// PROTOTYPE (#188), the question being prototyped: what a rebase method rebases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rebased {
    /// The branch itself, as GitLab does: `git rebase main x`, which checks `x` out here, then
    /// `git switch main` and the merge. `x` ends up merged, and moved.
    Itself,
    /// A copy on a detached HEAD, as GitHub's "Rebase and merge" does: `git rebase main <x's
    /// hash>`, then `git switch main` and the merge of the copy. `x` stays where it was,
    /// looking unmerged.
    Copy,
    /// A copy made on `main` itself: `git cherry-pick` of the commits the rebase would replay.
    /// Rebase and fast-forward only: a merge commit needs the copies on a side line.
    Picked,
}

impl Rebased {
    pub const ALL: [Rebased; 3] = [Rebased::Itself, Rebased::Copy, Rebased::Picked];

    pub fn name(self, target: &str) -> String {
        match self {
            Rebased::Itself => format!("{target} itself"),
            Rebased::Copy => "A copy".into(),
            Rebased::Picked => "A copy, picked onto it".into(),
        }
    }

    pub fn help(self, branch: &str, target: &str) -> String {
        match self {
            Rebased::Itself => format!(
                "Checks out {target} here, rebases it, then switches back to {branch}. \
                 {target} moves, and ends up merged."
            ),
            Rebased::Copy => format!(
                "Rebases a detached copy of {target}, then switches back to {branch}. \
                 {target} stays where it was, and looks unmerged."
            ),
            Rebased::Picked => format!(
                "Cherry-picks {target}'s commits onto {branch}. {branch} stays checked out; \
                 {target} stays where it was, and looks unmerged."
            ),
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
    /// PROTOTYPE (#188): what a rebase method rebases.
    pub rebased: Rebased,
    /// The merge commit's message; unused by a fast-forward.
    pub message: String,
    /// `--autostash` (`Some(true)`) or `--no-autostash` (`Some(false)`), where it differs from
    /// what `merge.autoStash` does.
    pub autostash: Option<bool>,
    /// `merge.log` is set: `--no-log`, since the message already has what it would add.
    pub no_log: bool,
}

/// The commands a merge runs, in order. PROTOTYPE (#188): a rebase method's three.
pub fn commands(merge: &Merge) -> Vec<Vec<String>> {
    let words = |w: &[&str]| w.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
    if !merge.method.rebases() {
        return vec![command(merge)];
    }
    let branch = merge.branch.as_str();
    match merge.rebased {
        Rebased::Itself => vec![
            words(&["rebase", branch, &merge.target]),
            words(&["switch", branch]),
            command(merge),
        ],
        Rebased::Copy => vec![
            words(&["rebase", branch, &merge.theirs.to_hex()]),
            words(&["switch", branch]),
            // The copy: where HEAD was before the switch.
            merge_command(merge, "HEAD@{1}"),
        ],
        Rebased::Picked => vec![words(&[
            "cherry-pick",
            "--cherry-pick",
            "--right-only",
            "--no-merges",
            &format!("{branch}...{}", merge.target),
        ])],
    }
}

/// The merge command.
pub fn command(merge: &Merge) -> Vec<String> {
    merge_command(merge, &merge.target)
}

/// The merge command, of `target`.
fn merge_command(merge: &Merge, target: &str) -> Vec<String> {
    let mut args = vec!["merge".to_owned(), merge.method.flag().to_owned()];
    match merge.autostash {
        Some(true) => args.push("--autostash".into()),
        Some(false) => args.push("--no-autostash".into()),
        None => {}
    }
    if merge.method.commits() {
        if merge.no_log {
            args.push("--no-log".into());
        }
        args.push("-m".into());
        args.push(merge.message.clone());
    }
    args.push(target.to_owned());
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
    /// PROTOTYPE (#188): the commits a rebase would replay: not merges, nor already in the
    /// branch.
    pub replays: usize,
    /// PROTOTYPE (#188): merges in `branch..theirs`, which a rebase flattens.
    pub merges: usize,
    /// PROTOTYPE (#188): what the target is, for which ways of rebasing it can take.
    pub kind: Kind,
}

/// PROTOTYPE (#188): what's merged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A local branch, and the worktree that has it checked out, if another does.
    Local {
        elsewhere: Option<std::path::PathBuf>,
    },
    Remote,
    Commit,
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
        let count = |args: &[&str]| -> Result<usize, Error> {
            Ok(git.run(args)?.trim().parse().unwrap_or(0))
        };
        let range = format!("{}...{}", head.to_hex(), theirs.to_hex());
        let replays = count(&[
            "rev-list",
            "--count",
            "--cherry-pick",
            "--right-only",
            "--no-merges",
            &range,
        ])?;
        let merges = count(&[
            "rev-list",
            "--count",
            "--merges",
            &format!("{}..{}", head.to_hex(), theirs.to_hex()),
        ])?;
        let kind = if catalog.locals.iter().any(|b| b.name == target) {
            Kind::Local {
                elsewhere: catalog.in_use_elsewhere(target).map(Path::to_path_buf),
            }
        } else if catalog.remotes.iter().any(|b| b.name == target) {
            Kind::Remote
        } else {
            Kind::Commit
        };
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
            replays,
            merges,
            kind,
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
        if method == Method::FastForward && !self.fast_forward {
            return Some(format!("{} has commits {target} doesn't", self.branch));
        }
        if method.rebases() {
            // The rebase would replay nothing.
            if self.replays == 0 {
                return Some(format!("{} already has {target}'s changes", self.branch));
            }
            if self.fast_forward && self.merges == 0 {
                return Some(format!("{target} is already on top of {}", self.branch));
            }
        }
        None
    }

    /// PROTOTYPE (#188): why `target` can't be rebased that way by `method`.
    pub fn unrebasable(&self, method: Method, rebased: Rebased, target: &str) -> Option<String> {
        match (rebased, &self.kind) {
            (Rebased::Itself, Kind::Local { elsewhere: Some(p) }) => Some(format!(
                "{target} is checked out in {}",
                p.file_name().unwrap_or(p.as_os_str()).to_string_lossy()
            )),
            (Rebased::Itself, Kind::Remote) => {
                Some(format!("{target} is a remote-tracking branch"))
            }
            (Rebased::Itself, Kind::Commit) => Some(format!("{target} is a commit, not a branch")),
            (Rebased::Picked, _) if method == Method::SemiLinear => {
                Some("A merge commit needs the copies on a side line".into())
            }
            _ => None,
        }
    }

    /// PROTOTYPE (#188): the way to rebase picked to begin with, for `method`.
    pub fn default_rebased(&self, method: Method) -> Rebased {
        Rebased::ALL
            .into_iter()
            .find(|r| self.unrebasable(method, *r, "").is_none())
            .unwrap_or(Rebased::Copy)
    }

    /// Why the merge can't start as chosen.
    pub fn blocked(
        &self,
        method: Method,
        rebased: Rebased,
        stash: bool,
        message: &str,
        target: &str,
    ) -> Option<String> {
        if let Some(why) = self.unavailable(method, target) {
            return Some(format!("{why}."));
        }
        if method.rebases() {
            if let Some(why) = self.unrebasable(method, rebased, target) {
                return Some(format!("{why}."));
            }
            // `--autostash` would put them back on what was rebased, not here.
            if self.dirty {
                return Some("Commit or stash your changes first.".into());
            }
        }
        if method == Method::FastForward {
            // It carries the changes along, or git refuses without losing any.
            return None;
        }
        if self.dirty && !stash && !method.rebases() {
            // `git merge --abort` can't always put them back.
            return Some("Commit or stash your changes first.".into());
        }
        (method.commits() && message.trim().is_empty())
            .then(|| "Enter a message for the merge commit.".into())
    }

    /// The merge, with the target named by `target`, by `method`, with the changes stashed or
    /// not.
    pub fn merge(
        &self,
        target: String,
        method: Method,
        rebased: Rebased,
        stash: bool,
        message: &str,
    ) -> Merge {
        let autostash = match (self.dirty && !method.rebases(), stash, self.auto_stash) {
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
            rebased,
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
    if merge.method.rebases() {
        return execute_rebasing(&git, merge, cancel, report);
    }
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

/// PROTOTYPE (#188): a rebase method's commands, one after the other, stopping at the first
/// that doesn't finish. A stop says what's left to do.
fn execute_rebasing(
    git: &Git,
    merge: &Merge,
    cancel: &Cancel,
    report: &mut Report,
) -> Result<(), Error> {
    let name = short_target(merge);
    let branch = &merge.branch;
    for (i, args) in commands(merge).into_iter().enumerate() {
        let picking = args[0] == "cherry-pick";
        let rebasing = args[0] == "rebase";
        if run(git, args, cancel, report)? {
            continue;
        }
        let after = Catalog::load(git.dir())?;
        let n = after.conflicted.len();
        let Some(Stuck::InProgress(_)) = after.stuck() else {
            let output = report.steps.last().unwrap().output.clone();
            return Err(Error::Failed(if i == 0 {
                output
            } else {
                format!("Stopped after the rebase, with {branch} not merged yet. {output}")
            }));
        };
        let what = if picking {
            "Cherry-pick"
        } else if rebasing {
            "Rebase"
        } else {
            "Merge"
        };
        let title = if n == 0 {
            format!("{what} stopped")
        } else {
            format!("{what} stopped on conflicts in {}", plural(n, "file"))
        };
        let message = match (merge.rebased, rebasing) {
            (Rebased::Itself, true) => format!(
                "{name} is checked out here, part rebased. Finish or abort it with git; \
                 then switch to {branch} and merge {name}."
            ),
            (Rebased::Copy, true) => format!(
                "The copy of {name} is on a detached HEAD here, part rebased. Finish or abort \
                 it with git; then switch to {branch} and merge the copy before it's lost."
            ),
            (Rebased::Picked, _) => format!(
                "{branch} has the commits picked so far. Finish or abort it with git, or go to \
                 another worktree."
            ),
            _ => "Finish or abort it with git, or go to another worktree.".into(),
        };
        report.attention = Some(Attention { title, message });
        return Ok(());
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
