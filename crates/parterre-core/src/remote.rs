//! Keeping local branches and their remotes in step (#181): fetching every remote, pulling the
//! open worktree's branch, pushing a local branch to its own name on a remote, with a
//! lease-guarded force push, and setting a branch's upstream.
//!
//! The network commands run with `--progress`, their output streamed to a [`Live`] for the
//! dialog that shows it. Parterre asks git before each step and never parses what it prints.

use std::io::Read;
use std::sync::{Arc, Mutex, PoisonError};

use parterre_util::CancelTree;

use crate::branches::{Attention, Catalog, Error, LocalBranch, Report, Step, Stuck, Warning};
use crate::git::{Git, GitError};
use crate::{Oid, Repo};

/// Said after a network command fails: parterre has no prompt of its own yet.
pub const NO_PROMPT: &str = "Parterre can't ask for passwords or passphrases: git needs a \
                             credential helper or ssh-agent.";

/// How a pull reconciles a branch that has diverged from its upstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reconcile {
    Merge,
    Rebase,
}

impl Reconcile {
    pub const ALL: [Reconcile; 2] = [Reconcile::Merge, Reconcile::Rebase];

    pub fn name(self) -> &'static str {
        match self {
            Reconcile::Merge => "Merge",
            Reconcile::Rebase => "Rebase",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Reconcile::Merge => "--no-rebase",
            Reconcile::Rebase => "--rebase",
        }
    }
}

/// `git pull` on the open worktree's branch, at the commit it was offered at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pull {
    pub branch: String,
    pub head: Oid,
    /// Chosen by the user, when the branch had diverged and git's config didn't say. Fetched
    /// already then.
    pub how: Option<Reconcile>,
}

/// A pull that needs the user to say how: the branch and its upstream have diverged, and
/// neither `pull.rebase`, `pull.ff` nor the branch's `rebase` is set. Nothing has changed but
/// the fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diverged {
    pub pull: Pull,
    /// The upstream's short name, `origin/main`.
    pub upstream: String,
    pub ahead: usize,
    pub behind: usize,
}

impl Diverged {
    /// The pull `how` runs.
    pub fn pull(&self, how: Reconcile) -> Pull {
        Pull {
            how: Some(how),
            ..self.pull.clone()
        }
    }

    pub fn command(&self, how: Reconcile) -> Vec<String> {
        pull_command(Some(how))
    }
}

/// Pushes local branch `branch`, at `tip`, to its own name on `remote`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Push {
    pub branch: String,
    pub tip: Oid,
    pub remote: String,
}

/// `git branch --set-upstream-to`, or `--unset-upstream` for `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetUpstream {
    pub branch: String,
    /// A remote-tracking branch, `origin/main`.
    pub upstream: Option<String>,
}

/// What pushing a branch to a remote would do, by the last fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushState {
    /// The remote has no branch of that name: the push creates it.
    New,
    /// The remote's branch is behind: a fast-forward.
    Ahead,
    /// Nothing to send: the same commit, or the remote's branch has it already.
    UpToDate,
    /// The remote's branch has commits the branch lacks: only a force push sends it.
    Force,
}

fn words(s: &[&str]) -> Vec<String> {
    s.iter().map(|s| (*s).to_owned()).collect()
}

pub(crate) fn fetch_command() -> Vec<String> {
    words(&["fetch", "--progress", "--all", "--prune"])
}

fn fetch_remote_command(remote: &str) -> Vec<String> {
    words(&["fetch", "--progress", "--prune", remote])
}

pub(crate) fn pull_command(how: Option<Reconcile>) -> Vec<String> {
    let mut args = words(&["pull", "--progress"]);
    args.extend(how.map(|h| h.flag().to_owned()));
    args
}

/// `git push`, with `-u` only when the branch has no upstream at all, so an upstream of
/// another name (or one not pushed yet) is left as it is.
pub(crate) fn push_command(catalog: &Catalog, push: &Push, force: bool) -> Vec<String> {
    let mut args = words(&["push", "--progress"]);
    let has_upstream = catalog
        .locals
        .iter()
        .any(|b| b.name == push.branch && b.upstream.is_some());
    if !has_upstream {
        args.push("-u".into());
    }
    if force {
        args.push(format!("--force-with-lease=refs/heads/{}", push.branch));
        args.push("--force-if-includes".into());
    }
    args.extend([push.remote.clone(), push.branch.clone()]);
    args
}

/// `git branch --set-upstream-to=…`, or `--unset-upstream`.
pub fn set_upstream_command(set: &SetUpstream) -> Vec<String> {
    match &set.upstream {
        Some(upstream) => vec![
            "branch".into(),
            format!("--set-upstream-to=refs/remotes/{upstream}"),
            "--".into(),
            set.branch.clone(),
        ],
        None => words(&["branch", "--unset-upstream", "--", &set.branch]),
    }
}

/// The remotes, by name, with what pushing local branch `branch` to each would do, by the
/// commits in `repo`. Empty if `repo` has no such branch.
pub fn push_targets(repo: &Repo, catalog: &Catalog, branch: &str) -> Vec<(String, PushState)> {
    let tip = |full: &str| {
        repo.refs
            .iter()
            .find(|r| r.full_name == full)
            .map(|r| r.target)
    };
    let Some(local) = tip(&format!("refs/heads/{branch}")) else {
        return Vec::new();
    };
    let mut remotes = catalog.remote_names.clone();
    remotes.sort();
    remotes
        .into_iter()
        .map(|remote| {
            let state = match tip(&format!("refs/remotes/{remote}/{branch}")) {
                None => PushState::New,
                Some(theirs) if repo.reaches(theirs, local) => PushState::UpToDate,
                Some(theirs) if repo.reaches(local, theirs) => PushState::Ahead,
                Some(_) => PushState::Force,
            };
            (remote, state)
        })
        .collect()
}

/// The open worktree's branch, when it is at `commit` and has an upstream: what *Pull* pulls.
pub fn pull_offered(catalog: &Catalog, commit: Oid) -> Option<&LocalBranch> {
    if !catalog.has_working_tree || catalog.head != Some(commit) {
        return None;
    }
    let current = catalog.current.as_deref()?;
    catalog
        .locals
        .iter()
        .find(|b| b.name == current && b.upstream.is_some())
}

/// `git fetch --all --prune`.
pub(crate) fn fetch(
    catalog: &Catalog,
    cancel: &CancelTree,
    report: &mut Report,
    live: Option<&Live>,
) -> Result<(), Error> {
    if catalog.remote_names.is_empty() {
        return Err(Error::Invalid("This repository has no remote.".into()));
    }
    let git = Git::new(&catalog.root);
    if !run_live(&git, fetch_command(), cancel, report, live)? {
        return Err(network_failure(report));
    }
    Ok(())
}

/// Fetches the upstream's remote, then pulls, unless the branch has diverged and git's config
/// doesn't say how to reconcile it: then the user is asked, with nothing changed but the fetch.
/// A stop on conflicts comes back as [`Report::attention`].
pub(crate) fn pull(
    catalog: &Catalog,
    pull: &Pull,
    cancel: &CancelTree,
    report: &mut Report,
    live: Option<&Live>,
) -> Result<Option<Diverged>, Error> {
    let branch = pull.branch.as_str();
    if !catalog.has_working_tree
        || catalog.current.as_deref() != Some(branch)
        || catalog.head != Some(pull.head)
    {
        return Err(Error::Invalid(format!(
            "Branch {branch} moved, or is no longer checked out here. Reload and try again."
        )));
    }
    if let Some(stuck) = catalog.stuck() {
        return Err(Error::Invalid(format!("{}.", stuck.reason())));
    }
    let Some(upstream) = catalog
        .locals
        .iter()
        .find(|b| b.name == branch)
        .and_then(|b| b.upstream.clone())
    else {
        return Err(Error::Invalid(format!("Branch {branch} has no upstream.")));
    };
    let git = Git::new(&catalog.root);
    if pull.how.is_none() {
        let remote = git.query(&["config", "--get", &format!("branch.{branch}.remote")])?;
        if let Some(remote) = remote.filter(|r| catalog.remote_names.contains(r))
            && !run_live(&git, fetch_remote_command(&remote), cancel, report, live)?
        {
            return Err(network_failure(report));
        }
        let range = format!("refs/heads/{branch}...{branch}@{{upstream}}");
        let counts = git.query(&["rev-list", "--left-right", "--count", &range, "--"])?;
        if let Some((ahead, behind)) = counts.as_deref().and_then(parse_counts) {
            if behind == 0 {
                return Ok(None);
            }
            if ahead > 0 && !reconcile_configured(&git, branch)? {
                return Ok(Some(Diverged {
                    pull: pull.clone(),
                    upstream,
                    ahead,
                    behind,
                }));
            }
        }
    }
    let ok = run_live(&git, pull_command(pull.how), cancel, report, live)?;
    let after = Catalog::load(&catalog.root)?;
    if let Some(stuck) = after.stuck() {
        let n = after.conflicted.len();
        let title = match (stuck, n) {
            (_, 1..) => format!("Pull stopped on conflicts in {}", plural(n, "file")),
            (Stuck::InProgress(_), 0) => "Pull stopped".to_owned(),
            (Stuck::Conflicts, 0) => unreachable!("conflicts without conflicted files"),
        };
        report.attention = Some(Attention {
            title,
            message: "Finish or abort it with git, or go to another worktree.".into(),
        });
        return Ok(None);
    }
    if !ok {
        return Err(Error::Failed(last_output(report)));
    }
    Ok(None)
}

/// Pushes the branch to its own name on the remote. One that would replace the remote's
/// commits asks first ([`Warning`]: a confirmation when the branch has a copy of each, else a
/// warning with the commits lost), then forces with a lease. A push the remote rejects is
/// followed by a fetch, to say whether the remote has commits the branch lacks.
pub(crate) fn push(
    catalog: &Catalog,
    action: &crate::branches::Action,
    push: &Push,
    approval: Option<&Warning>,
    cancel: &CancelTree,
    report: &mut Report,
    live: Option<&Live>,
) -> Result<Option<Warning>, Error> {
    let branch = push.branch.as_str();
    if !catalog
        .locals
        .iter()
        .any(|b| b.name == branch && b.tip == push.tip)
    {
        return Err(Error::Invalid(format!(
            "Branch {branch} moved or no longer exists. Reload and try again."
        )));
    }
    if !catalog.remote_names.contains(&push.remote) {
        return Err(Error::Invalid(format!(
            "There is no remote {}. Reload and try again.",
            push.remote
        )));
    }
    let git = Git::new(&catalog.root);
    let tracking = format!("refs/remotes/{}/{branch}", push.remote);
    let theirs = remote_tip(&git, &tracking)?;
    let force = match theirs {
        Some(theirs) => !is_ancestor(&git, theirs, push.tip)?,
        None => false,
    };
    if force {
        let (lost, replaced) = replaced_or_lost(&git, branch, &tracking)?;
        let approved =
            approval.is_some_and(|w| w.action == *action && w.head == theirs && w.commits == lost);
        if !approved {
            let repo = Arc::new(git.load()?);
            return Ok(Some(Warning {
                action: action.clone(),
                commits: lost,
                replaced,
                deletions: Vec::new(),
                repo,
                commands: vec![push_command(catalog, push, true)],
                head: theirs,
            }));
        }
    }
    if run_live(
        &git,
        push_command(catalog, push, force),
        cancel,
        report,
        live,
    )? {
        return Ok(None);
    }
    let pushed = last_output(report);
    if !run_live(
        &git,
        fetch_remote_command(&push.remote),
        cancel,
        report,
        live,
    )? {
        return Err(Error::Failed(format!("{pushed}\n\n{NO_PROMPT}")));
    }
    match remote_tip(&git, &tracking)? {
        Some(theirs) if !is_ancestor(&git, theirs, push.tip)? => Err(Error::Failed(format!(
            "{}/{branch} has commits you don't have.",
            push.remote
        ))),
        _ => Err(Error::Failed(pushed)),
    }
}

/// The remote's commits a force push would replace, split into those with no copy on the
/// branch (lost) and those with one (replaced), as the graph colours them.
fn replaced_or_lost(
    git: &Git,
    branch: &str,
    tracking: &str,
) -> Result<(Vec<Oid>, Vec<Oid>), Error> {
    let repo = git.load()?;
    let full = format!("refs/heads/{branch}");
    let upstreams = crate::upstream::load(git, &repo, &[(full, tracking.to_owned())]);
    let (mut lost, mut replaced) = (Vec::new(), Vec::new());
    for u in &upstreams {
        for &(c, side) in &u.commits {
            let oid = repo.commit(c).oid;
            match side {
                crate::upstream::Side::Ahead => {}
                crate::upstream::Side::Replaced => replaced.push(oid),
                crate::upstream::Side::Behind | crate::upstream::Side::Lost => lost.push(oid),
            }
        }
    }
    lost.sort_by_key(|o| o.to_hex());
    replaced.sort_by_key(|o| o.to_hex());
    Ok((lost, replaced))
}

pub(crate) fn set_upstream(
    catalog: &Catalog,
    set: &SetUpstream,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<(), Error> {
    if !catalog.locals.iter().any(|b| b.name == set.branch) {
        return Err(Error::Invalid(format!(
            "Branch {} no longer exists. Reload and try again.",
            set.branch
        )));
    }
    if let Some(upstream) = &set.upstream
        && !catalog.remotes.iter().any(|r| r.name == *upstream)
    {
        return Err(Error::Invalid(format!(
            "There is no remote branch {upstream}. Fetch, or reload and try again."
        )));
    }
    let git = Git::new(&catalog.root);
    if !run_live(&git, set_upstream_command(set), cancel, report, None)? {
        return Err(Error::Failed(last_output(report)));
    }
    Ok(())
}

fn remote_tip(git: &Git, full: &str) -> Result<Option<Oid>, Error> {
    Ok(git
        .query(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{full}^{{commit}}"),
        ])?
        .and_then(|s| Oid::from_hex(&s)))
}

fn is_ancestor(git: &Git, ancestor: Oid, of: Oid) -> Result<bool, Error> {
    let (a, b) = (ancestor.to_hex(), of.to_hex());
    Ok(git
        .query(&["merge-base", "--is-ancestor", &a, &b])?
        .is_some())
}

/// Whether git's config says how `git pull` reconciles diverged branches.
fn reconcile_configured(git: &Git, branch: &str) -> Result<bool, Error> {
    for key in [
        "pull.rebase".to_owned(),
        "pull.ff".to_owned(),
        format!("branch.{branch}.rebase"),
    ] {
        if git.query(&["config", "--get", &key])?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `rev-list --left-right --count`'s `ahead<TAB>behind`.
fn parse_counts(out: &str) -> Option<(usize, usize)> {
    let mut parts = out.split_whitespace().map(str::parse::<usize>);
    Some((parts.next()?.ok()?, parts.next()?.ok()?))
}

fn last_output(report: &Report) -> String {
    report
        .steps
        .last()
        .map(|s| s.output.clone())
        .unwrap_or_default()
}

/// A network command's failure, in git's words, and what parterre can't do about it.
fn network_failure(report: &Report) -> Error {
    Error::Failed(format!("{}\n\n{NO_PROMPT}", last_output(report)))
}

fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// What git printed so far, each command with its output, as a terminal shows it: a carriage
/// return starts its line again, as git's progress meters write it. Shared with the window
/// showing it while the commands run.
#[derive(Clone, Debug, Default)]
pub struct Live(Arc<Mutex<Vec<Shown>>>);

#[derive(Debug, Default)]
struct Shown {
    command: String,
    text: String,
    /// A carriage return was last: the next character starts the line again.
    restart: bool,
    /// The start of a character split between two reads.
    partial: Vec<u8>,
}

impl Shown {
    fn push(&mut self, bytes: &[u8]) {
        self.partial.extend_from_slice(bytes);
        let bytes = std::mem::take(&mut self.partial);
        let (text, rest) = match std::str::from_utf8(&bytes) {
            Ok(text) => (text.to_owned(), &[][..]),
            Err(e) if e.error_len().is_none() => {
                let (valid, rest) = bytes.split_at(e.valid_up_to());
                (String::from_utf8_lossy(valid).into_owned(), rest)
            }
            Err(_) => (String::from_utf8_lossy(&bytes).into_owned(), &[][..]),
        };
        self.partial = rest.to_vec();
        for ch in text.chars() {
            match ch {
                '\r' => self.restart = true,
                '\n' => {
                    self.restart = false;
                    self.text.push('\n');
                }
                ch => {
                    if std::mem::take(&mut self.restart) {
                        let start = self.text.rfind('\n').map_or(0, |i| i + 1);
                        self.text.truncate(start);
                    }
                    self.text.push(ch);
                }
            }
        }
    }
}

impl Live {
    /// Each command so far, as `git …`, with its output.
    pub fn steps(&self) -> Vec<(String, String)> {
        self.lock()
            .iter()
            .map(|s| (s.command.clone(), s.text.clone()))
            .collect()
    }

    /// A command starts.
    pub fn start(&self, args: &[String]) {
        self.lock().push(Shown {
            command: crate::branches::command_text(args),
            ..Shown::default()
        });
    }

    /// Output of the last command started.
    pub fn push(&self, bytes: &[u8]) {
        if let Some(shown) = self.lock().last_mut() {
            shown.push(bytes);
        }
    }

    /// The last command's output, as shown.
    fn last_text(&self) -> String {
        self.lock()
            .last()
            .map(|s| s.text.clone())
            .unwrap_or_default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Shown>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Runs a git command as [`crate::branches::run`] does, streaming its output to `live` as it
/// comes. ssh is kept from asking on a terminal parterre's user can't see: it fails instead,
/// unless the user has an askpass program of their own.
fn run_live(
    git: &Git,
    args: Vec<String>,
    cancel: &CancelTree,
    report: &mut Report,
    live: Option<&Live>,
) -> Result<bool, Error> {
    let own = Live::default();
    let live = live.unwrap_or(&own);
    live.start(&args);
    let mut command = git.operation_command(&args);
    if std::env::var_os("SSH_ASKPASS").is_none() {
        command
            .env("SSH_ASKPASS", "parterre-has-no-askpass")
            .env("SSH_ASKPASS_REQUIRE", "force");
    }
    let mut child = cancel.start(&mut command).map_err(|e| match e {
        parterre_util::Start::Cancelled => Error::Cancelled,
        parterre_util::Start::Spawn(e) => GitError::Spawn(e).into(),
    })?;
    let pipe = |mut from: Box<dyn Read + Send>, live: Live| {
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match from.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => live.push(&buf[..n]),
                }
            }
        })
    };
    let stdout = pipe(Box::new(child.stdout.take().unwrap()), live.clone());
    let stderr = pipe(Box::new(child.stderr.take().unwrap()), live.clone());
    let _ = stdout.join();
    let _ = stderr.join();
    let status = child.wait();
    let cancelled = cancel.finish();
    let status = status.map_err(GitError::Spawn)?;
    report.steps.push(Step {
        args,
        output: live.last_text().trim().to_owned(),
        success: status.success(),
    });
    if cancelled {
        return Err(Error::Cancelled);
    }
    Ok(status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_split_between_reads_comes_out_whole() {
        let live = Live::default();
        live.start(&["fetch".to_owned()]);
        let bytes = "Über\n".as_bytes();
        live.push(&bytes[..1]);
        live.push(&bytes[1..]);
        assert_eq!(live.steps()[0].1, "Über\n");
    }

    #[test]
    fn counts_are_read_as_git_prints_them() {
        assert_eq!(parse_counts("2\t3"), Some((2, 3)));
        assert_eq!(parse_counts(""), None);
    }
}
