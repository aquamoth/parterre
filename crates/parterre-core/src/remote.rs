//! Keeping local branches and their remotes in step (#181). The network commands run with
//! `--progress`, their output streamed to a [`Live`]. Parterre asks git before each step rather
//! than read what the porcelain prints.

use std::io::Read;
use std::sync::{Arc, Mutex, PoisonError};

use parterre_util::CancelTree;

use crate::branches::{
    Action, Attention, Catalog, Deletion, Error, LocalBranch, Report, Step, Warning, all_commits,
    lost_commits, same_losses,
};
use crate::git::{Git, GitError};
use crate::operation::{git_args, in_progress, refuse_if_moved_or_stuck, run_or_fail, stopped};
use crate::upstream::Side;
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
    /// Chosen by the user, when the branch had diverged and git's config didn't say.
    pub how: Option<Reconcile>,
}

/// A pull that needs the user to say how: the branch and its upstream have diverged, and
/// neither `pull.rebase`, `pull.ff` nor the branch's `rebase` is set. Nothing has changed but
/// the fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diverged {
    pub pull: Pull,
    /// `origin/main`.
    pub upstream: String,
    pub ahead: usize,
    pub behind: usize,
}

impl Diverged {
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

/// Pushes local branch `branch`, at `tip`, to branch `to` on `remote`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Push {
    pub branch: String,
    pub tip: Oid,
    pub remote: String,
    pub to: String,
}

impl Push {
    /// Pushing `branch` to `remote` as `git push` does: to its upstream on the upstream's
    /// remote, to its own name on any other.
    pub fn new(branch: &LocalBranch, remote: &str) -> Push {
        let to = branch
            .upstream_remote
            .as_ref()
            .filter(|(r, _)| r == remote)
            .map_or(&branch.name, |(_, b)| b);
        Push {
            branch: branch.name.clone(),
            tip: branch.tip,
            remote: remote.to_owned(),
            to: to.to_owned(),
        }
    }

    /// Where it goes: `origin`, or `origin/main` for a branch of another name.
    pub fn target(&self) -> String {
        if self.to == self.branch {
            self.remote.clone()
        } else {
            self.remote_branch()
        }
    }

    /// The branch it goes to, as its remote-tracking branch is called: `origin/main`.
    pub fn remote_branch(&self) -> String {
        format!("{}/{}", self.remote, self.to)
    }

    /// `refs/remotes/origin/main`.
    fn tracking_ref(&self) -> String {
        format!("refs/remotes/{}", self.remote_branch())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetUpstream {
    pub branch: String,
    /// A remote-tracking branch, `origin/main`.
    pub upstream: String,
}

/// A branch on a remote, at the tip its remote-tracking branch had when offered: `origin/topic`
/// is branch `topic` of remote `origin`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteBranchTip {
    pub remote: String,
    pub branch: String,
    pub tip: Oid,
}

impl RemoteBranchTip {
    /// `origin/topic`.
    pub fn name(&self) -> String {
        format!("{}/{}", self.remote, self.branch)
    }
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

pub(crate) fn fetch_command() -> Vec<String> {
    git_args(&["fetch", "--progress", "--all", "--prune"])
}

fn fetch_remote_command(remote: &str) -> Vec<String> {
    git_args(&["fetch", "--progress", "--prune", remote])
}

pub(crate) fn pull_command(how: Option<Reconcile>) -> Vec<String> {
    let mut args = git_args(&["pull", "--progress"]);
    args.extend(how.map(|h| h.flag().to_owned()));
    args
}

/// `git push`, with `-u` only when the branch has no upstream at all, so an upstream not pushed
/// yet is left as it is. `force` is the remote's tip it replaces: a lease on the branch's own
/// name takes `--force-if-includes`, but git checks that against the reflog of the local branch
/// named like the remote's, so a push to another name leases that tip instead.
pub(crate) fn push_command(catalog: &Catalog, push: &Push, force: Option<Oid>) -> Vec<String> {
    let mut args = git_args(&["push", "--progress"]);
    let has_upstream = catalog
        .locals
        .iter()
        .any(|b| b.name == push.branch && b.upstream.is_some());
    if !has_upstream {
        args.push("-u".into());
    }
    match force {
        Some(_) if push.to == push.branch => {
            args.push(format!("--force-with-lease=refs/heads/{}", push.to));
            args.push("--force-if-includes".into());
        }
        Some(theirs) => {
            args.push(format!(
                "--force-with-lease=refs/heads/{}:{theirs}",
                push.to
            ));
        }
        None => {}
    }
    args.push(push.remote.clone());
    if push.to == push.branch {
        args.push(push.branch.clone());
    } else {
        args.push(format!("{}:{}", push.branch, push.to));
    }
    args
}

pub fn set_upstream_command(set: &SetUpstream) -> Vec<String> {
    set_upstream_args(&set.branch, &set.upstream)
}

/// `git branch --set-upstream-to` for local branch `branch`, to remote-tracking branch
/// `upstream`.
pub(crate) fn set_upstream_args(branch: &str, upstream: &str) -> Vec<String> {
    git_args(&[
        "branch",
        &format!("--set-upstream-to=refs/remotes/{upstream}"),
        "--",
        branch,
    ])
}

/// Deletes the branch on its remote, only if it is still where the last fetch saw it.
pub(crate) fn delete_remote_command(b: &RemoteBranchTip) -> Vec<String> {
    git_args(&[
        "push",
        "--progress",
        &format!("--force-with-lease=refs/heads/{}:{}", b.branch, b.tip),
        &b.remote,
        "--delete",
        &b.branch,
    ])
}

/// Pushing local branch `branch` to each remote, by name, with what each would do by the
/// commits in `repo`. Empty if there is no such branch.
pub fn push_targets(repo: &Repo, catalog: &Catalog, branch: &str) -> Vec<(Push, PushState)> {
    let tip = |full: &str| {
        repo.refs
            .iter()
            .find(|r| r.full_name == full)
            .map(|r| r.target)
    };
    let (Some(local), Some(branch)) = (
        tip(&format!("refs/heads/{branch}")),
        catalog.locals.iter().find(|b| b.name == branch),
    ) else {
        return Vec::new();
    };
    let mut remotes = catalog.remote_names.clone();
    remotes.sort();
    remotes
        .into_iter()
        .map(|remote| {
            let push = Push::new(branch, &remote);
            let state = match tip(&push.tracking_ref()) {
                None => PushState::New,
                Some(theirs) if repo.reaches(theirs, local) => PushState::UpToDate,
                Some(theirs) if repo.reaches(local, theirs) => PushState::Ahead,
                Some(_) => PushState::Force,
            };
            (push, state)
        })
        .collect()
}

/// The open worktree's branch, when it has an upstream and `commit` is where it or that
/// upstream is: what *Pull* pulls (#356).
pub fn pull_offered(catalog: &Catalog, commit: Oid) -> Option<&LocalBranch> {
    if !catalog.has_working_tree {
        return None;
    }
    let current = catalog.current.as_deref()?;
    let branch = catalog.locals.iter().find(|b| b.name == current)?;
    let upstream = branch.upstream.as_deref()?;
    let at_upstream = || {
        catalog
            .remotes
            .iter()
            .any(|r| r.name == upstream && r.tip == commit)
    };
    (catalog.head == Some(commit) || at_upstream()).then_some(branch)
}

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

/// Fetches the upstream's remote, then pulls, unless the branch has diverged and neither git's
/// config nor the user says how to reconcile it: then the user is asked, with nothing changed
/// but the fetch. A stop on conflicts comes back as [`Report::attention`].
pub(crate) fn pull(
    catalog: &Catalog,
    pull: &Pull,
    cancel: &CancelTree,
    report: &mut Report,
    live: Option<&Live>,
) -> Result<Option<Diverged>, Error> {
    let branch = pull.branch.as_str();
    refuse_if_moved_or_stuck(catalog, branch, pull.head)?;
    let Some(upstream) = catalog
        .locals
        .iter()
        .find(|b| b.name == branch)
        .and_then(|b| b.upstream.clone())
    else {
        return Err(Error::Invalid(format!("Branch {branch} has no upstream.")));
    };
    let git = Git::new(&catalog.root);
    // Fetched here, so that a failure to reach the remote says what parterre can't do.
    let remote = git.query(&["config", "--get", &format!("branch.{branch}.remote")])?;
    if let Some(remote) = remote.filter(|r| catalog.remote_names.contains(r))
        && !run_live(&git, fetch_remote_command(&remote), cancel, report, live)?
    {
        return Err(network_failure(report));
    }
    if pull.how.is_none() {
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
    let pulled = run_live(&git, pull_command(pull.how), cancel, report, live)?;
    let after = Catalog::load(&catalog.root)?;
    if after.stuck().is_some() {
        report.attention = Some(in_progress(stopped("Pull", after.conflicted.len())));
        return Ok(None);
    }
    if !pulled {
        return Err(Error::Failed(report.last_output()));
    }
    Ok(None)
}

/// Pushes the branch to the remote. One that would replace the remote's commits asks first
/// ([`Warning`]: a confirmation when the branch has a copy of each, else a warning with the
/// commits lost), then forces with a lease. A push the remote rejects is followed by a fetch,
/// to say whether the remote has commits the branch lacks.
pub(crate) fn push(
    catalog: &Catalog,
    action: &Action,
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
    let tracking = push.tracking_ref();
    let theirs_name = push.remote_branch();
    let theirs = git.resolve_commit(&tracking)?;
    if let Some(theirs) = theirs
        && git.is_ancestor(push.tip, theirs)?
    {
        return Err(Error::Invalid(format!(
            "{theirs_name} has every commit of {branch}: there's nothing to push."
        )));
    }
    let force = match theirs {
        Some(theirs) if !git.is_ancestor(theirs, push.tip)? => Some(theirs),
        _ => None,
    };
    if force.is_some() {
        let (lost, replaced) = lost_and_replaced(&git, branch, &tracking)?;
        let approved =
            approval.is_some_and(|w| w.action == *action && w.head == theirs && w.commits == lost);
        if !approved {
            return Ok(Some(Warning {
                action: action.clone(),
                commits: lost,
                replaced,
                deletions: Vec::new(),
                repo: Some(Arc::new(git.load()?)),
                changed: false,
                commands: vec![push_command(catalog, push, force)],
                head: theirs,
            }));
        }
    }
    // What `--force-if-includes` would refuse, for a push to another name.
    if let Some(theirs) = force
        && push.to != push.branch
        && !ever_had(&git, branch, theirs)?
    {
        return Err(Error::Failed(format!(
            "{theirs_name} has commits {branch} never had, so parterre won't force push over \
             them. Take them into {branch} first."
        )));
    }
    let command = push_command(catalog, push, force);
    if run_live(&git, command, cancel, report, live)? {
        return Ok(None);
    }
    let pushed = report.last_output();
    let fetch = fetch_remote_command(&push.remote);
    if !run_live(&git, fetch, cancel, report, live)? {
        return Err(failed_without_prompt(&pushed));
    }
    match git.resolve_commit(&tracking)? {
        // The lease held, so it was `--force-if-includes`: the remote's commits were fetched,
        // but never on the branch.
        now if force.is_some() && push.to == push.branch && now == theirs => {
            Err(Error::Failed(format!(
                "{theirs_name} has commits {branch} never had, so git won't force push over \
                 them. Take them into {branch} first.\n\n{pushed}"
            )))
        }
        Some(now) if !git.is_ancestor(now, push.tip)? => Err(Error::Failed(format!(
            "{theirs_name} has commits you don't have."
        ))),
        _ => Err(Error::Failed(pushed)),
    }
}

/// Deletes branches on their remotes, one by one. It always asks first ([`Warning`]): a
/// confirmation when the commits are reached from elsewhere, else a warning listing those only
/// they reach. A branch the remote moved since the last fetch is kept, and a fetch suggested;
/// one already gone there is as asked, and comes back as [`Report::attention`].
pub(crate) fn delete_remote_branches(
    catalog: &Catalog,
    action: &Action,
    branches: &[RemoteBranchTip],
    approval: Option<&Warning>,
    cancel: &CancelTree,
    report: &mut Report,
    live: Option<&Live>,
) -> Result<Option<Warning>, Error> {
    check_remote_branches(catalog, branches)?;
    let git = Git::new(&catalog.root);
    let deletions = remote_branch_losses(&git, catalog, branches)?;
    let commits = all_commits(&deletions);
    let approved = approval.is_some_and(|w| {
        w.action == *action && w.commits == commits && same_losses(&w.deletions, &deletions)
    });
    if !approved {
        return Ok(Some(Warning {
            action: action.clone(),
            commits,
            replaced: Vec::new(),
            deletions,
            repo: Some(Arc::new(git.load()?)),
            changed: false,
            commands: branches.iter().map(delete_remote_command).collect(),
            head: None,
        }));
    }
    let (mut deleted, mut gone) = (Vec::new(), Vec::new());
    for b in branches {
        if run_live(&git, delete_remote_command(b), cancel, report, live)? {
            deleted.push(b.name());
            continue;
        }
        let pushed = report.last_output();
        let (name, remote) = (b.name(), &b.remote);
        match ls_remote(&git, b, cancel)? {
            OnRemote::Unknown => return Err(failed_without_prompt(&pushed)),
            // Someone deleted it first: as asked.
            OnRemote::Gone => gone.push(name),
            OnRemote::At(tip) if tip == b.tip => {
                let went = went_so_far(&deleted, &gone);
                return Err(Error::Failed(format!("{pushed}{went}")));
            }
            OnRemote::At(_) => {
                report.suggest_fetch = true;
                let went = went_so_far(&deleted, &gone);
                return Err(Error::Failed(format!(
                    "{name} moved on {remote} since the last fetch. Fetch, and look again.{went}"
                )));
            }
        }
    }
    if !gone.is_empty() {
        // The graph shows them until a fetch.
        report.suggest_fetch = true;
        let (title, it) = match gone.as_slice() {
            [one] => (format!("{one} was already gone"), "it"),
            many => (format!("{} were already gone", many.join(", ")), "them"),
        };
        report.attention = Some(Attention {
            title,
            message: format!(
                "Someone deleted {it} on the remote first. Fetch to update the graph."
            ),
        });
    }
    Ok(None)
}

/// Each remote branch is listed once, and is where it was offered.
fn check_remote_branches(catalog: &Catalog, branches: &[RemoteBranchTip]) -> Result<(), Error> {
    if branches.is_empty() {
        return Err(Error::Invalid("Choose a remote branch to delete.".into()));
    }
    for (i, b) in branches.iter().enumerate() {
        let name = b.name();
        if branches[..i].contains(b) {
            return Err(Error::Invalid(format!(
                "Remote branch {name} is listed twice."
            )));
        }
        if !catalog.remote_names.contains(&b.remote)
            || !catalog
                .remotes
                .iter()
                .any(|r| r.name == name && r.tip == b.tip)
        {
            return Err(Error::Invalid(format!(
                "Remote branch {name} moved or is gone since. Reload and try again."
            )));
        }
    }
    Ok(())
}

/// What deleting the remote branches would lose: the commits only each reaches once they're
/// all gone.
fn remote_branch_losses(
    git: &Git,
    catalog: &Catalog,
    branches: &[RemoteBranchTip],
) -> Result<Vec<Deletion>, Error> {
    // A remote's `HEAD` naming one of them goes with it.
    let excluded: Vec<String> = branches
        .iter()
        .flat_map(|b| {
            let name = b.name();
            let head = catalog
                .remote_defaults
                .contains(&name)
                .then(|| format!("refs/remotes/{}/HEAD", b.remote));
            std::iter::once(format!("refs/remotes/{name}")).chain(head)
        })
        .collect();
    branches
        .iter()
        .map(|b| {
            let commits = lost_commits(git, catalog, b.tip, &excluded, &[], None)?;
            Ok(Deletion::of_branch(b.name(), b.tip, commits))
        })
        .collect()
}

/// What a remote deletion that stopped partway did, as sentences after its error.
fn went_so_far(deleted: &[String], gone: &[String]) -> String {
    let said = |names: &[String], one: &str, many: &str| match names {
        [] => String::new(),
        [name] => format!(" {name} {one}."),
        names => format!(" {} {many}.", names.join(", ")),
    };
    said(deleted, "was deleted", "were deleted")
        + &said(gone, "was already gone", "were already gone")
}

/// Where a branch is on its remote now.
enum OnRemote {
    /// git couldn't ask.
    Unknown,
    Gone,
    At(Oid),
}

fn ls_remote(git: &Git, b: &RemoteBranchTip, cancel: &CancelTree) -> Result<OnRemote, Error> {
    let args = git_args(&["ls-remote", &b.remote, &format!("refs/heads/{}", b.branch)]);
    let child = cancel.start(&mut network_command(git, &args))?;
    let out = child.wait_with_output();
    if cancel.finish() {
        return Err(Error::Cancelled);
    }
    let out = out.map_err(GitError::Spawn)?;
    if !out.status.success() {
        return Ok(OnRemote::Unknown);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(
        match text.split_whitespace().next().and_then(Oid::from_hex) {
            Some(tip) => OnRemote::At(tip),
            None => OnRemote::Gone,
        },
    )
}

/// The remote's commits a force push would replace, split into those with no copy on the
/// branch (lost) and those with one (replaced), as the graph colours them.
fn lost_and_replaced(
    git: &Git,
    branch: &str,
    tracking: &str,
) -> Result<(Vec<Oid>, Vec<Oid>), Error> {
    let repo = git.load()?;
    let full = format!("refs/heads/{branch}");
    let upstreams = crate::upstream::load(git, &repo, &[(full, tracking.to_owned())]);
    let (mut lost, mut replaced) = (Vec::new(), Vec::new());
    for &(c, side) in upstreams.iter().flat_map(|u| &u.commits) {
        let oid = repo.commit(c).oid;
        match side {
            Side::Ahead => {}
            Side::Replaced => replaced.push(oid),
            Side::Behind | Side::Lost => lost.push(oid),
        }
    }
    lost.sort_unstable();
    replaced.sort_unstable();
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
    if !catalog.remotes.iter().any(|r| r.name == set.upstream) {
        return Err(Error::Invalid(format!(
            "There is no remote-tracking branch {}. Fetch, or reload and try again.",
            set.upstream
        )));
    }
    let git = Git::new(&catalog.root);
    run_or_fail(&git, set_upstream_command(set), cancel, report)
}

/// Whether local branch `branch` has `commit`, or had it at any point its reflog recalls.
fn ever_had(git: &Git, branch: &str, commit: Oid) -> Result<bool, Error> {
    let reflog = git.run(&[
        "rev-list",
        "--walk-reflogs",
        &format!("refs/heads/{branch}"),
    ])?;
    let mut input = format!("{commit}\n");
    input.extend(reflog.lines().map(|oid| format!("^{oid}\n")));
    Ok(git
        .run_with_input(&["rev-list", "-n", "1", "--stdin"], input)?
        .trim()
        .is_empty())
}

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

/// A network command's failure, in git's words, and what parterre can't do about it.
pub(crate) fn network_failure(report: &Report) -> Error {
    failed_without_prompt(&report.last_output())
}

fn failed_without_prompt(output: &str) -> Error {
    Error::Failed(format!("{output}\n\n{NO_PROMPT}"))
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

/// An operation's git command that may reach a remote. ssh is kept from asking on a terminal
/// parterre's user can't see: it fails instead, unless the user has an askpass program of
/// their own.
fn network_command(git: &Git, args: &[String]) -> std::process::Command {
    let mut command = git.operation_command(args);
    if std::env::var_os("SSH_ASKPASS").is_none() {
        command
            .env("SSH_ASKPASS", "parterre-has-no-askpass")
            .env("SSH_ASKPASS_REQUIRE", "force");
    }
    command
}

/// Runs a git command as [`crate::operation::run`] does, streaming its output to `live` as it
/// comes.
pub(crate) fn run_live(
    git: &Git,
    args: Vec<String>,
    cancel: &CancelTree,
    report: &mut Report,
    live: Option<&Live>,
) -> Result<bool, Error> {
    let own = Live::default();
    let live = live.unwrap_or(&own);
    live.start(&args);
    let mut child = cancel.start(&mut network_command(git, &args))?;
    let stdout = stream(child.stdout.take().expect("stdout is piped"), live.clone());
    let stderr = stream(child.stderr.take().expect("stderr is piped"), live.clone());
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

/// Copies what `from` gives to `live` as it comes, on a thread of its own.
fn stream(mut from: impl Read + Send + 'static, live: Live) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n @ 1..) = from.read(&mut buf) {
            live.push(&buf[..n]);
        }
    })
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
