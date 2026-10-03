//! Merging a branch or commit into the open worktree's branch, by one of the merge methods:
//! a fast-forward (`git merge --ff-only`) or a merge commit (`git merge --no-ff`) with the
//! message the user agreed to, pre-filled as git words it. A merge that stops, on conflicts or
//! because a hook refused to commit it, leaves the worktree with an operation in progress,
//! finished with git for now.

use std::path::{Path, PathBuf};

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
    /// PROTOTYPE (#188): rebases the open worktree's branch onto the other, then fast-forwards
    /// the other up to it. Only merging the open worktree's branch into another.
    RebaseFastForward,
    /// PROTOTYPE (#188): the same rebase, then `--no-ff` into the other.
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
            Method::FastForward => "--ff-only",
            Method::MergeCommit => "--no-ff",
            Method::RebaseFastForward => "rebase, then --ff-only",
            Method::SemiLinear => "rebase, then --no-ff",
        }
    }

    /// It rebases first.
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
                "Replays {target}'s commits on top of {branch}, then joins them with a merge \
                 commit."
            ),
        }
    }
}

/// PROTOTYPE (#188): merging the open worktree's branch into another, as a pull request does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    /// The open worktree's branch, merged in.
    pub source: String,
    /// The worktree the branch merged into is checked out in, if any; the merge runs there.
    pub worktree: Option<PathBuf>,
    /// That worktree has uncommitted changes to tracked files.
    pub worktree_dirty: bool,
}

impl Outgoing {
    /// The steps after the rebase run in the open worktree: switching to the branch, merging
    /// and switching back. Only a merge commit into a branch checked out nowhere.
    fn switches(&self, method: Method) -> bool {
        self.worktree.is_none() && method.commits()
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
    /// PROTOTYPE (#188): merging the open worktree's branch into `branch`.
    pub outgoing: Option<Outgoing>,
}

/// The command a merge into the open worktree's branch runs.
pub fn command(merge: &Merge) -> Vec<String> {
    let mut args = vec!["merge".to_owned(), merge.method.flag().to_owned()];
    stash_flag(merge, &mut args);
    merge_args(merge, &mut args);
    args
}

fn stash_flag(merge: &Merge, args: &mut Vec<String>) {
    match merge.autostash {
        Some(true) => args.push("--autostash".into()),
        Some(false) => args.push("--no-autostash".into()),
        None => {}
    }
}

/// PROTOTYPE (#188): the commands a merge runs, in order. Merging the open worktree's branch
/// into another: the rebase, if any, here; then the merge in the other's worktree, or, with
/// none, a fast-forward of the ref, or switching here to merge and back.
pub fn commands(merge: &Merge) -> Vec<Vec<String>> {
    let Some(out) = &merge.outgoing else {
        return vec![command(merge)];
    };
    let words = |w: &[&str]| w.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
    let (source, into) = (out.source.as_str(), merge.branch.as_str());
    let mut steps = Vec::new();
    if merge.method.rebases() {
        let mut rebase = words(&["rebase"]);
        stash_flag(merge, &mut rebase);
        rebase.push(into.to_owned());
        steps.push(rebase);
    }
    let flag = if merge.method.commits() {
        "--no-ff"
    } else {
        "--ff-only"
    };
    let merging = |prefix: Vec<String>| {
        let mut args = prefix;
        args.extend(words(&["merge", flag]));
        if !merge.method.rebases() && out.worktree.is_none() {
            stash_flag(merge, &mut args);
        }
        merge_args(merge, &mut args);
        args.pop();
        args.push(source.to_owned());
        args
    };
    match &out.worktree {
        Some(w) => steps.push(merging(vec!["-C".into(), w.to_string_lossy().into_owned()])),
        None if merge.method.commits() => {
            steps.push(words(&["switch", into]));
            steps.push(merging(Vec::new()));
            steps.push(words(&["switch", source]));
        }
        None => steps.push(words(&["fetch", ".", &format!("{source}:{into}")])),
    }
    steps
}

fn merge_args(merge: &Merge, args: &mut Vec<String>) {
    if merge.method.commits() {
        if merge.no_log {
            args.push("--no-log".into());
        }
        args.push("-m".into());
        args.push(merge.message.clone());
    }
    args.push(merge.target.clone());
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
    /// branch merged into.
    pub replays: usize,
    /// PROTOTYPE (#188): merges a rebase would flatten.
    pub merges: usize,
    /// PROTOTYPE (#188): merging the open worktree's branch into `branch`.
    pub outgoing: Option<Outgoing>,
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
            replays: 0,
            merges: 0,
            outgoing: None,
        })
    }

    /// PROTOTYPE (#188): the merge of the open worktree's branch into local branch `into`.
    pub fn load_outgoing(path: &Path, into: &str) -> Result<Preview, Error> {
        let catalog = Catalog::load(path)?;
        if let Some(stuck) = catalog.stuck() {
            return Err(Error::Invalid(format!("{}.", stuck.reason())));
        }
        let (Some(source), Some(theirs)) = (catalog.current.clone(), catalog.head) else {
            return Err(Error::Invalid(
                "There's no branch checked out to merge.".into(),
            ));
        };
        let Some(head) = catalog
            .locals
            .iter()
            .find(|b| b.name == into)
            .map(|b| b.tip)
        else {
            return Err(Error::Invalid(format!("{into} is not a local branch.")));
        };
        let git = Git::new(&catalog.root);
        let (h, t) = (head.to_hex(), theirs.to_hex());
        let fast_forward = git
            .query(&["merge-base", "--is-ancestor", &h, &t])?
            .is_some();
        let dirty_in = |dir: &Path| -> Result<bool, Error> {
            Ok(!Git::new(dir)
                .run(&["status", "--porcelain", "--untracked-files=no"])?
                .trim()
                .is_empty())
        };
        let dirty = dirty_in(&catalog.root)?;
        let worktree = catalog
            .worktrees
            .iter()
            .find(|w| w.branch.as_deref() == Some(into))
            .map(|w| w.path.clone());
        let worktree_dirty = match &worktree {
            Some(w) => dirty_in(w)?,
            None => false,
        };
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
        let line = format!("{t}\t\tbranch '{source}' of .\n");
        let message = git.run_with_input(&["fmt-merge-msg", "--into-name", into], line)?;
        let count = |args: &[&str]| -> Result<usize, Error> {
            Ok(git.run(args)?.trim().parse().unwrap_or(0))
        };
        let replays = count(&[
            "rev-list",
            "--count",
            "--cherry-pick",
            "--right-only",
            "--no-merges",
            &format!("{h}...{t}"),
        ])?;
        let merges = count(&["rev-list", "--count", "--merges", &format!("{h}..{t}")])?;
        Ok(Preview {
            branch: into.to_owned(),
            head,
            theirs,
            fast_forward,
            dirty,
            // The rebase's, which is what stashes here.
            auto_stash: truthy(&config("rebase.autoStash")),
            no_ff: config("merge.ff").as_deref() == Some("false"),
            log: truthy(&config("merge.log")),
            message: strip_comments(&message),
            replays,
            merges,
            outgoing: Some(Outgoing {
                source,
                worktree,
                worktree_dirty,
            }),
        })
    }

    /// The methods offered: the rebase methods only merging the open worktree's branch into
    /// another.
    pub fn methods(&self) -> &'static [Method] {
        if self.outgoing.is_some() {
            &Method::ALL
        } else {
            &Method::ALL[..2]
        }
    }

    /// Whether *Stash changes* applies to `method`: it changes the open worktree, and can put
    /// the changes back there.
    pub fn stashable(&self, method: Method) -> bool {
        if !self.dirty {
            return false;
        }
        match &self.outgoing {
            None => true,
            Some(out) => method.rebases() && !out.switches(method),
        }
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
        if let Some(out) = &self.outgoing {
            let here = method.rebases() || out.switches(method);
            if self.dirty && here && !(stash && self.stashable(method)) {
                return Some("Commit or stash your changes first.".into());
            }
            if out.worktree_dirty && method.commits() {
                return Some(format!(
                    "{}'s worktree has uncommitted changes.",
                    self.branch
                ));
            }
            return (method.commits() && message.trim().is_empty())
                .then(|| "Enter a message for the merge commit.".into());
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
        let stashing = match &self.outgoing {
            None => self.dirty,
            Some(_) => self.stashable(method),
        };
        let autostash = match (stashing, stash, self.auto_stash) {
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
            outgoing: self.outgoing.clone(),
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
    if merge.outgoing.is_some() {
        return execute_outgoing(catalog, merge, cancel, report);
    }
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

/// PROTOTYPE (#188): merges the open worktree's branch into another, one step after the other,
/// if neither moved. A stop says where, and what's left to do.
fn execute_outgoing(
    catalog: &Catalog,
    merge: &Merge,
    cancel: &Cancel,
    report: &mut Report,
) -> Result<(), Error> {
    let out = merge.outgoing.as_ref().unwrap();
    let (source, into) = (&out.source, &merge.branch);
    let tip = catalog
        .locals
        .iter()
        .find(|b| b.name == *into)
        .map(|b| b.tip);
    if catalog.current.as_deref() != Some(source.as_str())
        || catalog.head != Some(merge.theirs)
        || tip != Some(merge.head)
    {
        return Err(Error::Invalid(format!(
            "{source} or {into} moved, or {source} is no longer checked out here. Reload and \
             try again."
        )));
    }
    if let Some(stuck) = catalog.stuck() {
        return Err(Error::Invalid(format!("{}.", stuck.reason())));
    }
    let git = Git::new(&catalog.root);
    for (i, args) in commands(merge).into_iter().enumerate() {
        let elsewhere = (args[0] == "-C").then(|| PathBuf::from(&args[1]));
        let rebasing = args[0] == "rebase";
        let switching_back = i > 0 && args[0] == "switch" && args[1] == *source;
        if run(&git, args, cancel, report)? {
            continue;
        }
        let dir = elsewhere.clone().unwrap_or_else(|| catalog.root.clone());
        let after = Catalog::load(&dir)?;
        let Some(Stuck::InProgress(_)) = after.stuck() else {
            let output = report.steps.last().unwrap().output.clone();
            return Err(Error::Failed(if merge.method.rebases() && !rebasing {
                format!("{source} was rebased onto {into}, but {into} didn't move. {output}")
            } else if switching_back {
                format!("Merged {source} into {into}, but couldn't switch back. {output}")
            } else {
                output
            }));
        };
        let n = after.conflicted.len();
        let what = if rebasing { "Rebase" } else { "Merge" };
        let mut title = if n == 0 {
            format!("{what} not committed")
        } else {
            format!("{what} stopped on conflicts in {}", plural(n, "file"))
        };
        let message = if rebasing {
            format!(
                "Finish or abort it with git; then merge {source} into {into} again, now \
                 without conflicts."
            )
        } else if let Some(w) = &elsewhere {
            title.push_str(&format!(
                " in {}",
                w.file_name().unwrap_or(w.as_os_str()).to_string_lossy()
            ));
            format!("{into} is checked out there. Go to that worktree to finish or abort it.")
        } else {
            format!(
                "This worktree is on {into} now. Finish or abort the merge with git, then \
                 switch back to {source}."
            )
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
