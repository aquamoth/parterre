//! Merging, by one of the merge methods: a branch or commit into the open worktree's branch,
//! or, as a pull request does, the open worktree's branch into another local branch. A
//! fast-forward (`--ff-only`) or a merge commit (`--no-ff`) with the message the user agreed
//! to, pre-filled as git words it; into another branch, also with the open worktree's branch
//! rebased first. A merge that stops, on conflicts or because a hook refused to commit it,
//! leaves a worktree with an operation in progress, finished with git for now.

use std::path::{Path, PathBuf};

use crate::branches::{Attention, Catalog, Error, Report, Stuck, run};
use crate::git::Git;
use crate::log::is_ancestor;
use crate::{Oid, Repo};
use parterre_util::CancelTree;

/// How the branch takes in the other's commits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Moves the branch up to the other: `--ff-only`.
    FastForward,
    /// Joins the other in with a merge commit, even where a fast-forward would do: `--no-ff`.
    MergeCommit,
    /// Rebases the other onto the branch, then fast-forwards the branch up to it. Only merging
    /// the open worktree's branch into another, which is the one rebased.
    RebaseFastForward,
    /// The same rebase, then a merge commit: `--no-ff`.
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

    /// How git is asked for it.
    pub fn flag(self) -> &'static str {
        match self {
            Method::FastForward => "--ff-only",
            Method::MergeCommit => "--no-ff",
            Method::RebaseFastForward => "rebase, then merge --ff-only",
            Method::SemiLinear => "rebase, then merge --no-ff",
        }
    }

    /// The merge's own flag, after any rebase.
    fn merge_flag(self) -> &'static str {
        if self.commits() {
            "--no-ff"
        } else {
            "--ff-only"
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

/// Merging the open worktree's branch into another local branch, as a pull request does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    /// The open worktree's branch, which is merged in, and rebased first by the rebase
    /// methods.
    pub source: String,
    /// The worktree the branch merged into is checked out in, where the merge runs; `None`
    /// when it's checked out nowhere.
    pub worktree: Option<PathBuf>,
}

impl Outgoing {
    /// The merge runs in the open worktree, switching to the branch merged into and back: a
    /// merge commit into a branch checked out nowhere. A fast-forward of one only moves its
    /// ref.
    fn switches(&self, method: Method) -> bool {
        self.worktree.is_none() && method.commits()
    }
}

/// A merge the user agreed to. It runs only while the branches are still where they were.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merge {
    /// The branch merged into.
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
    /// what `merge.autoStash` does, or `rebase.autoStash` for the rebase methods.
    pub autostash: Option<bool>,
    /// `merge.log` is set: `--no-log`, since the message already has what it would add.
    pub no_log: bool,
    /// Merging the open worktree's branch, `target`, into `branch`.
    pub outgoing: Option<Outgoing>,
}

/// The command a merge into the open worktree's branch runs.
pub fn command(merge: &Merge) -> Vec<String> {
    let mut args = vec!["merge".to_owned(), merge.method.flag().to_owned()];
    autostash(merge, &mut args);
    merge_args(merge, &mut args);
    args
}

/// The commands a merge runs, in order. Into the open worktree's branch, one. Into another:
/// the rebase, if any, here; then the merge in the other's worktree, or, when it's checked out
/// nowhere, a fast-forward of its ref, or switching here to merge and back.
pub fn commands(merge: &Merge) -> Vec<Vec<String>> {
    let Some(out) = &merge.outgoing else {
        return vec![command(merge)];
    };
    let words = |w: &[&str]| w.iter().map(|w| (*w).to_owned()).collect::<Vec<String>>();
    let into = merge.branch.as_str();
    let mut steps = Vec::new();
    if merge.method.rebases() {
        let mut rebase = words(&["rebase"]);
        autostash(merge, &mut rebase);
        rebase.push(into.to_owned());
        steps.push(rebase);
    }
    let mut merging = words(&["merge", merge.method.merge_flag()]);
    merge_args(merge, &mut merging);
    match &out.worktree {
        Some(w) => {
            let mut there = vec!["-C".to_owned(), w.to_string_lossy().into_owned()];
            there.append(&mut merging);
            steps.push(there);
        }
        None if out.switches(merge.method) => {
            steps.push(words(&["switch", into]));
            steps.push(merging);
            steps.push(words(&["switch", &out.source]));
        }
        // Git moves it only if that's a fast-forward.
        None => steps.push(words(&["fetch", ".", &format!("{}:{into}", out.source)])),
    }
    steps
}

fn autostash(merge: &Merge, args: &mut Vec<String>) {
    match merge.autostash {
        Some(true) => args.push("--autostash".into()),
        Some(false) => args.push("--no-autostash".into()),
        None => {}
    }
}

/// The message, if any, and what's merged.
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

/// The open worktree's branch, when it can be merged into a local branch at `tip`: it has
/// commits that branch lacks. Offered while the open worktree is stuck too, to be greyed out.
pub fn offered_into<'a>(repo: &Repo, catalog: &'a Catalog, tip: Oid) -> Option<&'a str> {
    if !catalog.has_working_tree {
        return None;
    }
    let source = catalog.current.as_deref()?;
    let head = repo.lookup(&catalog.head?)?;
    let tip = repo.lookup(&tip)?;
    (!is_ancestor(repo, head, tip)).then_some(source)
}

/// What a merge would do, for its dialog.
#[derive(Clone, Debug)]
pub struct Preview {
    /// The branch merged into.
    pub branch: String,
    pub head: Oid,
    pub theirs: Oid,
    /// The branch has no commits of its own since `theirs`: it can fast-forward.
    pub fast_forward: bool,
    /// Uncommitted changes to tracked files in the open worktree.
    pub dirty: bool,
    /// `merge.autoStash` is set, or `rebase.autoStash` merging into another branch, where
    /// only the rebase stashes.
    pub auto_stash: bool,
    /// `merge.ff` is `false`: the user wants a merge commit even where a fast-forward would do.
    pub no_ff: bool,
    /// `merge.log` is set, and the message has its list of commits: the merge mustn't add
    /// it again.
    pub log: bool,
    /// The merge commit's message as git would write it.
    pub message: String,
    /// The commits a rebase would replay: neither merges nor already in the branch.
    pub replays: usize,
    /// Merges a rebase would flatten.
    pub merges: usize,
    /// Merging the open worktree's branch into `branch`.
    pub outgoing: Option<Outgoing>,
    /// Why the branch merged into can't take a merge commit now: its worktree has uncommitted
    /// changes. A fast-forward is left to git.
    pub target_dirty: Option<String>,
    /// Why the branch merged into can't take a merge at all now: its worktree is gone, or has
    /// an operation in progress.
    pub target_busy: Option<String>,
}

impl Preview {
    /// The merge of `theirs` into the open worktree's branch, named by `target`: a branch's
    /// name, or the full hash.
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
        let config = Config::read(&git);
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
            fast_forward: is_ancestor_git(&git, head, theirs)?,
            dirty: dirty(&catalog.root)?,
            auto_stash: config.merge_auto_stash,
            no_ff: config.no_ff,
            log: config.log,
            message: strip_comments(&message),
            replays: 0,
            merges: 0,
            outgoing: None,
            target_dirty: None,
            target_busy: None,
        })
    }

    /// The merge of the open worktree's branch into local branch `into`.
    pub fn load_into(path: &Path, into: &str) -> Result<Preview, Error> {
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
            .find(|b| b.name == into && b.name != source)
            .map(|b| b.tip)
        else {
            return Err(Error::Invalid(format!(
                "Branch {into} no longer exists. Reload and try again."
            )));
        };
        let git = Git::new(&catalog.root);
        if is_ancestor_git(&git, theirs, head)? {
            return Err(Error::Invalid(format!(
                "Branch {into} already has {source}'s commits."
            )));
        }
        let config = Config::read(&git);
        let there = catalog
            .worktrees
            .iter()
            .find(|w| w.branch.as_deref() == Some(into));
        let mut target_busy = match there {
            Some(w) if w.missing => Some(format!("{into}'s worktree {} is missing", w.name())),
            Some(w) => w
                .in_progress
                .map(|what| format!("{into}'s worktree {} has {what} in progress", w.name())),
            None => None,
        };
        if there.is_none()
            && let Some(p) = catalog.in_use_elsewhere(into)
        {
            // Being rebased or bisected there, detached.
            let name = p.file_name().unwrap_or(p.as_os_str()).to_string_lossy();
            target_busy = Some(format!("{into} is in use in worktree {name}"));
        }
        let usable = there.filter(|w| !w.missing);
        let target_dirty = match usable {
            Some(w) if dirty(&w.path)? => Some(format!(
                "{into}'s worktree {} has uncommitted changes",
                w.name()
            )),
            _ => None,
        };
        let line = format!("{}\t\tbranch '{source}' of .\n", theirs.to_hex());
        let message = match usable {
            // Where `into` is HEAD, git words it as it will.
            Some(w) => Git::new(&w.path).run_with_input(&["fmt-merge-msg"], line)?,
            // Here, git would list `merge.log`'s commits as missing from this branch, which
            // has them all: the merge adds them itself, once it has switched to `into`.
            None => {
                let args = ["fmt-merge-msg", "--no-log", "--into-name", into];
                match git.run_with_input(&args, line.clone()) {
                    Ok(message) => message,
                    // Git before 2.38 names only the checked-out branch.
                    Err(_) => retarget(
                        &git.run_with_input(&["fmt-merge-msg", "--no-log"], line)?,
                        &source,
                        into,
                    ),
                }
            }
        };
        let count = |args: &[&str]| -> Result<usize, Error> {
            Ok(git.run(args)?.trim().parse().unwrap_or(0))
        };
        let (h, t) = (head.to_hex(), theirs.to_hex());
        Ok(Preview {
            branch: into.to_owned(),
            head,
            theirs,
            fast_forward: is_ancestor_git(&git, head, theirs)?,
            dirty: dirty(&catalog.root)?,
            auto_stash: config.rebase_auto_stash,
            no_ff: config.no_ff,
            // Only where the message has `merge.log`'s commits already.
            log: config.log && usable.is_some(),
            message: strip_comments(&message),
            replays: count(&[
                "rev-list",
                "--count",
                "--cherry-pick",
                "--right-only",
                "--no-merges",
                &format!("{h}...{t}"),
            ])?,
            merges: count(&["rev-list", "--count", "--merges", &format!("{h}..{t}")])?,
            outgoing: Some(Outgoing {
                source,
                worktree: usable.map(|w| w.path.clone()),
            }),
            target_dirty,
            target_busy,
        })
    }

    /// The methods offered: the rebase methods only merging the open worktree's branch into
    /// another, the branch they rebase.
    pub fn methods(&self) -> &'static [Method] {
        if self.outgoing.is_some() {
            &Method::ALL
        } else {
            &Method::ALL[..2]
        }
    }

    /// *Stash changes* is offered for `method`: the open worktree has uncommitted changes, and
    /// it changes that worktree in a way that can put them back.
    pub fn stashable(&self, method: Method) -> bool {
        self.dirty
            && match &self.outgoing {
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
        if let Some(why) = &self.target_busy {
            return Some(format!("{why}."));
        }
        let changes_here = match &self.outgoing {
            // It carries the changes along, or git refuses without losing any.
            None => method == Method::MergeCommit,
            Some(out) => method.rebases() || out.switches(method),
        };
        // `git merge --abort` can't always put them back, and a switch can't stash them.
        if self.dirty && changes_here && !(stash && self.stashable(method)) {
            return Some("Commit or stash your changes first.".into());
        }
        if method.commits()
            && let Some(why) = &self.target_dirty
        {
            return Some(format!("{why}."));
        }
        (method.commits() && message.trim().is_empty())
            .then(|| "Enter a message for the merge commit.".into())
    }

    /// The merge, with the target named by `target`, by `method`, with the changes stashed or
    /// not.
    pub fn merge(&self, target: String, method: Method, stash: bool, message: &str) -> Merge {
        let autostash = match (self.stashable(method), stash, self.auto_stash) {
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

/// The merge settings in git's config.
struct Config {
    merge_auto_stash: bool,
    rebase_auto_stash: bool,
    no_ff: bool,
    log: bool,
}

impl Config {
    fn read(git: &Git) -> Config {
        let get = |key: &str| {
            git.query(&["config", "--get", key])
                .ok()
                .flatten()
                .map(|v| v.trim().to_ascii_lowercase())
        };
        let truthy = |key: &str| {
            get(key)
                .as_deref()
                .is_some_and(|v| !matches!(v, "false" | "no" | "off" | "0" | ""))
        };
        Config {
            merge_auto_stash: truthy("merge.autoStash"),
            rebase_auto_stash: truthy("rebase.autoStash"),
            no_ff: get("merge.ff").as_deref() == Some("false"),
            log: truthy("merge.log"),
        }
    }
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

/// Uncommitted changes to tracked files in the worktree at `dir`.
fn dirty(dir: &Path) -> Result<bool, Error> {
    Ok(!Git::new(dir)
        .run(&["status", "--porcelain", "--untracked-files=no"])?
        .trim()
        .is_empty())
}

/// `fmt-merge-msg`'s message, worded in the worktree of `from`, as if merging into `into`.
fn retarget(message: &str, from: &str, into: &str) -> String {
    let (first, rest) = message.split_once('\n').unwrap_or((message, ""));
    let first = first
        .strip_suffix(&format!(" into {from}"))
        .unwrap_or(first);
    format!("{first} into {into}\n{rest}")
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

fn stash_count(git: &Git) -> usize {
    git.run(&["stash", "list"])
        .map(|out| out.lines().count())
        .unwrap_or(0)
}

/// The notice when the autostash couldn't be put back: git applied what it could, with
/// conflict markers, and kept the stash entry too.
fn kept_stash(merge: &Merge, conflicted: usize) -> Attention {
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
    Attention {
        title: format!("Merged {} into {}", short_target(merge), merge.branch),
        message,
    }
}

/// Runs the merge if the branches are still where they were. A stop, on conflicts or because
/// a hook refused to commit, or changes the autostash couldn't put back, come back as
/// [`Report::attention`].
pub(crate) fn execute(
    catalog: &Catalog,
    merge: &Merge,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<(), Error> {
    if let Some(out) = &merge.outgoing {
        return execute_into(catalog, merge, out, cancel, report);
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
    let before = stash_count(&git);
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
            title: stopped("Merge", n),
            message,
        });
        return Ok(());
    }
    if !ok {
        return Err(Error::Failed(report.steps.last().unwrap().output.clone()));
    }
    if stash_count(&git) > before {
        report.attention = Some(kept_stash(merge, after.conflicted.len()));
    }
    Ok(())
}

/// Merges the open worktree's branch into another, step by step, if neither branch moved. A
/// stop says where, and what's left to do.
fn execute_into(
    catalog: &Catalog,
    merge: &Merge,
    out: &Outgoing,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<(), Error> {
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
            "Branch {source} or {into} moved, or {source} is no longer checked out here. \
             Reload and try again."
        )));
    }
    let there = catalog
        .worktrees
        .iter()
        .find(|w| w.branch.as_deref() == Some(into.as_str()))
        .map(|w| w.path.as_path());
    if there != out.worktree.as_deref() {
        return Err(Error::Invalid(format!(
            "Branch {into} was checked out or released elsewhere. Reload and try again."
        )));
    }
    if let Some(stuck) = catalog.stuck() {
        return Err(Error::Invalid(format!("{}.", stuck.reason())));
    }
    let git = Git::new(&catalog.root);
    let before = stash_count(&git);
    for args in commands(merge) {
        let elsewhere = (args[0] == "-C").then(|| PathBuf::from(&args[1]));
        let rebasing = args[0] == "rebase";
        let switching_back = args[0] == "switch" && args[1] == *source;
        if run(&git, args, cancel, report)? {
            continue;
        }
        let dir = elsewhere.as_deref().unwrap_or(&catalog.root);
        let after = Catalog::load(dir)?;
        let Some(Stuck::InProgress(_)) = after.stuck() else {
            let output = report.steps.last().unwrap().output.clone();
            return Err(Error::Failed(if switching_back {
                format!(
                    "Merged {source} into {into}, but couldn't switch back to {source}. {output}"
                )
            } else if merge.method.rebases() && !rebasing {
                format!("{source} was rebased onto {into}, but {into} didn't move. {output}")
            } else {
                output
            }));
        };
        let n = after.conflicted.len();
        let (title, message) = if rebasing {
            (
                if n == 0 {
                    "Rebase stopped".to_owned()
                } else {
                    stopped("Rebase", n)
                },
                format!(
                    "Finish or abort it with git, or go to another worktree. Once rebased, \
                     {source} merges into {into} without conflicts."
                ),
            )
        } else if let Some(w) = &elsewhere {
            (
                format!("{} in {}", stopped("Merge", n), folder(w)),
                format!(
                    "{into} is checked out there. Finish or abort the merge in that worktree \
                     with git."
                ),
            )
        } else {
            (
                stopped("Merge", n),
                format!(
                    "This worktree is on {into} now. Finish or abort the merge with git, then \
                     switch back to {source}."
                ),
            )
        };
        report.attention = Some(Attention { title, message });
        return Ok(());
    }
    if stash_count(&git) > before {
        let after = Catalog::load(&catalog.root)?;
        report.attention = Some(kept_stash(merge, after.conflicted.len()));
    }
    Ok(())
}

/// "Merge stopped on conflicts in 2 files", or "not committed" when none conflicts: a hook
/// refused it.
fn stopped(what: &str, conflicted: usize) -> String {
    if conflicted == 0 {
        format!("{what} not committed")
    } else {
        format!(
            "{what} stopped on conflicts in {}",
            plural(conflicted, "file")
        )
    }
}

fn folder(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
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
