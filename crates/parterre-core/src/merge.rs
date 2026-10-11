use std::path::{Path, PathBuf};

use crate::branches::{Attention, Catalog, Error, Report, Stuck, run};
use crate::git::Git;
use crate::log::is_ancestor;
use crate::{Oid, Repo};
use parterre_util::CancelTree;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    FastForward,
    MergeCommit,
    RebaseFastForward,
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

    /// For display only: the rebase methods take two commands.
    pub fn flag(self) -> &'static str {
        match self {
            Method::FastForward => "--ff-only",
            Method::MergeCommit => "--no-ff",
            Method::RebaseFastForward => "rebase, then merge --ff-only",
            Method::SemiLinear => "rebase, then merge --no-ff",
        }
    }

    fn merge_flag(self) -> &'static str {
        if self.makes_commit() {
            "--no-ff"
        } else {
            "--ff-only"
        }
    }

    pub fn rebases(self) -> bool {
        matches!(self, Method::RebaseFastForward | Method::SemiLinear)
    }

    pub fn makes_commit(self) -> bool {
        matches!(self, Method::MergeCommit | Method::SemiLinear)
    }

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

/// Merging the open worktree's branch into another, as a pull request does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    pub source: String,
    /// Where the branch merged into is checked out, if anywhere.
    pub worktree: Option<PathBuf>,
}

impl Outgoing {
    /// A merge commit into a branch checked out nowhere switches to it here and back; a
    /// fast-forward of one only moves its ref.
    fn switches(&self, method: Method) -> bool {
        self.worktree.is_none() && method.makes_commit()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merge {
    /// The branch merged into.
    pub branch: String,
    pub head: Oid,
    pub theirs: Oid,
    /// What the command names `theirs` by: a branch, or the full hash.
    pub target: String,
    pub method: Method,
    pub message: String,
    /// `--autostash` or `--no-autostash`, where it differs from what git's config does.
    pub autostash: Option<bool>,
    pub no_log: bool,
    pub outgoing: Option<Outgoing>,
}

pub fn command(merge: &Merge) -> Vec<String> {
    let mut args = vec!["merge".to_owned(), merge.method.merge_flag().to_owned()];
    args.extend(autostash_flag(merge).map(str::to_owned));
    args.extend(message_and_target(merge));
    args
}

pub fn commands(merge: &Merge) -> Vec<Vec<String>> {
    match &merge.outgoing {
        None => vec![command(merge)],
        Some(out) => outgoing_steps(merge, out)
            .into_iter()
            .map(|(_, args)| args)
            .collect(),
    }
}

/// A step of merging the open worktree's branch into another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Rebase,
    SwitchToBranch,
    Merge,
    SwitchBack,
}

fn outgoing_steps(merge: &Merge, out: &Outgoing) -> Vec<(Stage, Vec<String>)> {
    let words = |w: &[&str]| w.iter().map(|w| (*w).to_owned()).collect::<Vec<String>>();
    let into = merge.branch.as_str();
    let mut steps = Vec::new();
    if merge.method.rebases() {
        let mut rebase = words(&["rebase"]);
        rebase.extend(autostash_flag(merge).map(str::to_owned));
        rebase.push(into.to_owned());
        steps.push((Stage::Rebase, rebase));
    }
    let mut merging = words(&["merge", merge.method.merge_flag()]);
    merging.extend(message_and_target(merge));
    match &out.worktree {
        Some(w) => {
            let mut there = vec!["-C".to_owned(), w.to_string_lossy().into_owned()];
            there.append(&mut merging);
            steps.push((Stage::Merge, there));
        }
        None if out.switches(merge.method) => {
            steps.push((Stage::SwitchToBranch, words(&["switch", into])));
            steps.push((Stage::Merge, merging));
            steps.push((Stage::SwitchBack, words(&["switch", &out.source])));
        }
        // Git moves it only if that's a fast-forward.
        None => steps.push((
            Stage::Merge,
            words(&["fetch", ".", &format!("{}:{into}", out.source)]),
        )),
    }
    steps
}

fn autostash_flag(merge: &Merge) -> Option<&'static str> {
    merge
        .autostash
        .map(|on| if on { "--autostash" } else { "--no-autostash" })
}

fn message_and_target(merge: &Merge) -> Vec<String> {
    let mut args = Vec::new();
    if merge.method.makes_commit() {
        if merge.no_log {
            args.push("--no-log".to_owned());
        }
        args.extend(["-m".to_owned(), merge.message.clone()]);
    }
    args.push(merge.target.clone());
    args
}

pub fn offered<'a>(repo: &Repo, catalog: &'a Catalog, theirs: Oid) -> Option<&'a str> {
    if !catalog.has_working_tree || catalog.stuck().is_some() {
        return None;
    }
    let branch = catalog.current.as_deref()?;
    let head = repo.lookup(&catalog.head?)?;
    let theirs = repo.lookup(&theirs)?;
    (!is_ancestor(repo, theirs, head)).then_some(branch)
}

/// Offered while the open worktree is stuck too, to be greyed out.
pub fn offered_into<'a>(repo: &Repo, catalog: &'a Catalog, tip: Oid) -> Option<&'a str> {
    if !catalog.has_working_tree {
        return None;
    }
    let source = catalog.current.as_deref()?;
    let head = repo.lookup(&catalog.head?)?;
    let tip = repo.lookup(&tip)?;
    (!is_ancestor(repo, head, tip)).then_some(source)
}

#[derive(Clone, Debug)]
pub struct Preview {
    /// The branch merged into.
    pub branch: String,
    pub head: Oid,
    pub theirs: Oid,
    pub can_fast_forward: bool,
    pub dirty: bool,
    /// `merge.autoStash`, or, merging into another branch, where only the rebase stashes,
    /// `rebase.autoStash`.
    pub auto_stash: bool,
    pub no_ff: bool,
    pub message_has_log: bool,
    pub message: String,
    pub replays: usize,
    pub flattened_merges: usize,
    pub outgoing: Option<Outgoing>,
    refusal: Option<String>,
    /// A fast-forward is left to git.
    merge_commit_refusal: Option<String>,
}

impl Preview {
    pub fn load(path: &Path, theirs: Oid, target: &str) -> Result<Preview, Error> {
        let catalog = Catalog::load(path)?;
        refuse_if_stuck(&catalog)?;
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
        let line = format!("{theirs}\t\t{what}\n");
        let message = git.run_with_input(&["fmt-merge-msg"], line)?;
        Ok(Preview {
            branch,
            head,
            theirs,
            can_fast_forward: is_ancestor_git(&git, head, theirs)?,
            dirty: dirty(&catalog.root)?,
            auto_stash: config.merge_auto_stash,
            no_ff: config.no_ff,
            message_has_log: config.log,
            message: strip_comments(&message),
            replays: 0,
            flattened_merges: 0,
            outgoing: None,
            refusal: None,
            merge_commit_refusal: None,
        })
    }

    pub fn load_into(path: &Path, into: &str) -> Result<Preview, Error> {
        let catalog = Catalog::load(path)?;
        refuse_if_stuck(&catalog)?;
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
        let refusal = match there {
            Some(w) if w.missing => Some(format!("{into}'s worktree {} is missing", w.name())),
            Some(w) => w
                .in_progress
                .map(|what| format!("{into}'s worktree {} has {what} in progress", w.name())),
            // Being rebased or bisected there, detached.
            None => catalog
                .in_use_elsewhere(into)
                .map(|p| format!("{into} is in use in worktree {}", folder(p))),
        };
        let usable = there.filter(|w| !w.missing);
        let merge_commit_refusal = match usable {
            Some(w) if dirty(&w.path)? => Some(format!(
                "{into}'s worktree {} has uncommitted changes",
                w.name()
            )),
            _ => None,
        };
        let line = format!("{theirs}\t\tbranch '{source}' of .\n");
        let message = match usable {
            // Where `into` is HEAD, git words it as it will.
            Some(w) => Git::new(&w.path).run_with_input(&["fmt-merge-msg"], line)?,
            // Here, git would list `merge.log`'s commits as missing from this branch, which
            // has them all: the merge adds them itself, once it has switched to `into`.
            None => {
                let args = ["fmt-merge-msg", "--no-log", "--into-name", into];
                match git.run_with_input(&args, line.clone()) {
                    Ok(message) => message,
                    // Git before 2.35 names only the checked-out branch.
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
        Ok(Preview {
            branch: into.to_owned(),
            head,
            theirs,
            can_fast_forward: is_ancestor_git(&git, head, theirs)?,
            dirty: dirty(&catalog.root)?,
            auto_stash: config.rebase_auto_stash,
            no_ff: config.no_ff,
            message_has_log: config.log && usable.is_some(),
            message: strip_comments(&message),
            replays: count(&[
                "rev-list",
                "--count",
                "--cherry-pick",
                "--right-only",
                "--no-merges",
                &format!("{head}...{theirs}"),
            ])?,
            flattened_merges: count(&[
                "rev-list",
                "--count",
                "--merges",
                &format!("{head}..{theirs}"),
            ])?,
            outgoing: Some(Outgoing {
                source,
                worktree: usable.map(|w| w.path.clone()),
            }),
            refusal,
            merge_commit_refusal,
        })
    }

    pub fn methods(&self) -> &'static [Method] {
        if self.outgoing.is_some() {
            &Method::ALL
        } else {
            &Method::ALL[..2]
        }
    }

    pub fn stashable(&self, method: Method) -> bool {
        self.dirty
            && match &self.outgoing {
                None => true,
                Some(out) => method.rebases() && !out.switches(method),
            }
    }

    pub fn default_method(&self) -> Method {
        if self.can_fast_forward && !self.no_ff {
            Method::FastForward
        } else {
            Method::MergeCommit
        }
    }

    pub fn unavailable(&self, method: Method, target: &str) -> Option<String> {
        if method == Method::FastForward && !self.can_fast_forward {
            return Some(format!("{} has commits {target} doesn't", self.branch));
        }
        if method.rebases() {
            if self.replays == 0 {
                return Some(format!("{} already has {target}'s changes", self.branch));
            }
            if self.can_fast_forward && self.flattened_merges == 0 {
                return Some(format!("{target} is already on top of {}", self.branch));
            }
        }
        None
    }

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
        if let Some(why) = &self.refusal {
            return Some(format!("{why}."));
        }
        let changes_here = match &self.outgoing {
            // A fast-forward carries the changes along, or git refuses without losing any.
            None => method == Method::MergeCommit,
            Some(out) => method.rebases() || out.switches(method),
        };
        // `git merge --abort` can't always put them back, and a switch can't stash them.
        if self.dirty && changes_here && !(stash && self.stashable(method)) {
            return Some("Commit or stash your changes first.".into());
        }
        if method.makes_commit()
            && let Some(why) = &self.merge_commit_refusal
        {
            return Some(format!("{why}."));
        }
        (method.makes_commit() && message.trim().is_empty())
            .then(|| "Enter a message for the merge commit.".into())
    }

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
            no_log: self.message_has_log,
            outgoing: self.outgoing.clone(),
        }
    }
}

struct Config {
    merge_auto_stash: bool,
    rebase_auto_stash: bool,
    no_ff: bool,
    log: bool,
}

impl Config {
    fn read(git: &Git) -> Config {
        // One `config --list` rather than a `--get` per key (#309).
        let config = git.config().unwrap_or_default();
        let truthy = |key: &str| config.bool(key).unwrap_or(false);
        Config {
            merge_auto_stash: truthy("merge.autoStash"),
            rebase_auto_stash: truthy("rebase.autoStash"),
            no_ff: config
                .get("merge.ff")
                .is_some_and(|v| v.eq_ignore_ascii_case("false")),
            log: truthy("merge.log"),
        }
    }
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

pub(crate) fn execute(
    catalog: &Catalog,
    merge: &Merge,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<(), Error> {
    if let Some(out) = &merge.outgoing {
        return execute_into(catalog, merge, out, cancel, report);
    }
    refuse_if_moved(catalog, &merge.branch, merge.head)?;
    refuse_if_stuck(catalog)?;
    let git = Git::new(&catalog.root);
    let before = stash_count(&git);
    let ok = run(&git, command(merge), cancel, report)?;
    let after = Catalog::load(&catalog.root)?;
    if let Some(Stuck::InProgress(_)) = after.stuck() {
        // Git keeps autostashed changes aside until the merge is committed or aborted.
        let aside = git
            .query(&["rev-parse", "--verify", "--quiet", "MERGE_AUTOSTASH"])
            .ok()
            .flatten()
            .is_some();
        let mut message = FINISH_OR_ABORT.to_owned();
        if aside {
            message.push_str(" Your changes are set aside until then.");
        }
        report.attention = Some(Attention {
            title: stopped("Merge", after.conflicted.len()),
            message,
        });
        return Ok(());
    }
    if !ok {
        return Err(Error::Failed(last_output(report)));
    }
    if stash_count(&git) > before {
        report.attention = Some(kept_stash(merged(merge), after.conflicted.len()));
    }
    Ok(())
}

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
    refuse_if_stuck(catalog)?;
    let git = Git::new(&catalog.root);
    let before = stash_count(&git);
    for (stage, args) in outgoing_steps(merge, out) {
        if run(&git, args, cancel, report)? {
            continue;
        }
        let elsewhere = out.worktree.as_deref().filter(|_| stage == Stage::Merge);
        let after = Catalog::load(elsewhere.unwrap_or(&catalog.root))?;
        let Some(Stuck::InProgress(_)) = after.stuck() else {
            let output = last_output(report);
            return Err(Error::Failed(match stage {
                Stage::SwitchBack => format!(
                    "Merged {source} into {into}, but couldn't switch back to {source}. {output}"
                ),
                _ if merge.method.rebases() && stage != Stage::Rebase => {
                    format!("{source} was rebased onto {into}, but {into} didn't move. {output}")
                }
                _ => output,
            }));
        };
        let n = after.conflicted.len();
        let (title, message) = match (stage, elsewhere) {
            (Stage::Rebase, _) => (
                if n == 0 {
                    "Rebase stopped".to_owned()
                } else {
                    stopped("Rebase", n)
                },
                format!(
                    "{FINISH_OR_ABORT} Once rebased, {source} merges into {into} without \
                     conflicts."
                ),
            ),
            (_, Some(w)) => (
                format!("{} in {}", stopped("Merge", n), folder(w)),
                format!(
                    "{into} is checked out there. Finish or abort the merge in that worktree \
                     with git."
                ),
            ),
            _ => (
                stopped("Merge", n),
                format!(
                    "This worktree is on {into} now. Finish or abort the merge with git, then \
                     switch back to {source}."
                ),
            ),
        };
        report.attention = Some(Attention { title, message });
        return Ok(());
    }
    if stash_count(&git) > before {
        let after = Catalog::load(&catalog.root)?;
        report.attention = Some(kept_stash(merged(merge), after.conflicted.len()));
    }
    Ok(())
}

fn merged(merge: &Merge) -> String {
    format!("Merged {} into {}", short_target(merge), merge.branch)
}

fn folder(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub fn short_target(merge: &Merge) -> String {
    if merge.target == merge.theirs.to_hex() {
        merge.theirs.short(7)
    } else {
        merge.target.clone()
    }
}

pub(crate) const FINISH_OR_ABORT: &str = "Finish or abort it with git, or go to another worktree.";

pub(crate) fn refuse_if_stuck(catalog: &Catalog) -> Result<(), Error> {
    match catalog.stuck() {
        Some(stuck) => Err(Error::Invalid(format!("{}.", stuck.reason()))),
        None => Ok(()),
    }
}

pub(crate) fn refuse_if_moved(catalog: &Catalog, branch: &str, head: Oid) -> Result<(), Error> {
    if catalog.current.as_deref() == Some(branch) && catalog.head == Some(head) {
        return Ok(());
    }
    Err(Error::Invalid(format!(
        "Branch {branch} moved, or is no longer checked out here. Reload and try again."
    )))
}

pub(crate) fn is_ancestor_git(git: &Git, ancestor: Oid, of: Oid) -> Result<bool, Error> {
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
pub(crate) fn dirty(dir: &Path) -> Result<bool, Error> {
    Ok(!Git::new(dir)
        .run(&["status", "--porcelain", "--untracked-files=no"])?
        .trim()
        .is_empty())
}

pub(crate) fn stash_count(git: &Git) -> usize {
    git.run(&["stash", "list"])
        .map(|out| out.lines().count())
        .unwrap_or(0)
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

/// With nothing conflicted, a hook refused to commit.
pub(crate) fn stopped(what: &str, conflicted: usize) -> String {
    if conflicted == 0 {
        format!("{what} not committed")
    } else {
        format!(
            "{what} stopped on conflicts in {}",
            plural(conflicted, "file")
        )
    }
}

pub(crate) fn last_output(report: &Report) -> String {
    report
        .steps
        .last()
        .map(|s| s.output.clone())
        .unwrap_or_default()
}

pub(crate) fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}
