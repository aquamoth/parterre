//! Local branch operations and their safety checks. No graph visibility or GUI state enters
//! the loss calculation. Git's porcelain remains the final authority on checkout and deletion.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::git::{Git, GitError};
use crate::{Oid, Repo};

#[derive(Clone, Debug)]
pub struct LocalBranch {
    pub name: String,
    pub tip: Oid,
    /// Logical remote/branch name, including an upstream that has not been fetched.
    pub upstream: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RemoteBranch {
    pub name: String,
    pub tip: Oid,
}

/// A fresh catalogue for menus and creation forms. Refresh after repository changes; execution
/// always reads Git again, rather than trusting the catalogue the user first saw.
#[derive(Clone, Debug)]
pub struct Catalog {
    pub locals: Vec<LocalBranch>,
    pub remotes: Vec<RemoteBranch>,
    pub remote_names: Vec<String>,
    pub occupied: HashMap<String, PathBuf>,
    pub current: Option<String>,
    pub head: Option<Oid>,
    pub has_working_tree: bool,
    auto_setup_rebase: bool,
    roots: Vec<(String, Oid)>,
    worktrees: Vec<(PathBuf, Oid)>,
    root: PathBuf,
}

impl Catalog {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let git = Git::new(path);
        let root = git.repo_root()?;
        let has_working_tree = git.run(&["rev-parse", "--is-inside-work-tree"])?.trim() == "true";
        let current = git
            .query(&["symbolic-ref", "--quiet", "HEAD"])?
            .map(|s| s.trim_start_matches("refs/heads/").to_owned());
        let head = git
            .query(&["rev-parse", "--verify", "HEAD^{commit}"])?
            .map(|s| parse_oid(&s))
            .transpose()?;
        let auto_setup_rebase = matches!(
            git.query(&["config", "--get", "branch.autoSetupRebase"])?
                .as_deref(),
            Some("always" | "remote")
        );
        let mut remote_names: Vec<String> =
            git.run(&["remote"])?.lines().map(str::to_owned).collect();
        // A remote may itself contain '/', so match the longest configured prefix.
        remote_names.sort_by_key(|r| std::cmp::Reverse(r.len()));
        let mut locals = Vec::new();
        let mut remotes = Vec::new();
        let mut roots = Vec::new();
        let refs = git.run(&["for-each-ref", "--format=%(refname)%00%(objectname)%00%(objecttype)%00%(*objectname)%00%(*objecttype)%00%(upstream:remotename)%00%(upstream:remoteref)", "refs/heads", "refs/remotes", "refs/tags"])?;
        for line in refs.lines() {
            let f: Vec<_> = line.split('\0').collect();
            if f.len() != 7 {
                return Err(Error::Invalid("Unexpected Git reference listing.".into()));
            }
            let tip = if f[2] == "commit" {
                Some(parse_oid(f[1])?)
            } else if f[4] == "commit" {
                Some(parse_oid(f[3])?)
            } else if f[2] == "tag" {
                git.query(&["rev-parse", "--verify", &format!("{}^{{commit}}", f[0])])?
                    .map(|s| parse_oid(&s))
                    .transpose()?
            } else {
                None
            };
            let Some(tip) = tip else { continue };
            roots.push((f[0].to_owned(), tip));
            if let Some(name) = f[0].strip_prefix("refs/heads/") {
                let upstream = (!f[5].is_empty() && !f[6].is_empty()).then(|| {
                    format!(
                        "{}/{}",
                        f[5],
                        f[6].strip_prefix("refs/heads/").unwrap_or(f[6])
                    )
                });
                locals.push(LocalBranch {
                    name: name.to_owned(),
                    tip,
                    upstream,
                });
            } else if let Some(name) = f[0].strip_prefix("refs/remotes/")
                && !name.ends_with("/HEAD")
            {
                remotes.push(RemoteBranch {
                    name: name.to_owned(),
                    tip,
                });
            }
        }
        locals.sort_by(|a, b| a.name.cmp(&b.name));
        remotes.sort_by(|a, b| a.name.cmp(&b.name));
        let mut occupied = HashMap::new();
        let mut worktrees = Vec::new();
        // Unlike the viewer's compatibility fallback, safety checks propagate listing errors.
        let listing = match git.run(&["worktree", "list", "--porcelain", "-z"]) {
            Ok(listing) => listing,
            Err(GitError::Failed { .. }) => {
                // Git 2.34 lacks -z. Never turn a failed listing into an empty catalogue;
                // retry its older form, rejecting unrecognised fields (e.g. a split path).
                let plain = git.run(&["worktree", "list", "--porcelain"])?;
                if root.to_string_lossy().contains(['\n', '\r'])
                    || plain.lines().any(|line| {
                        !matches!(
                            line.split(' ').next(),
                            Some(
                                "" | "worktree"
                                    | "HEAD"
                                    | "branch"
                                    | "bare"
                                    | "detached"
                                    | "locked"
                                    | "prunable"
                            )
                        )
                    })
                {
                    return Err(Error::Invalid("Cannot safely read worktree paths with this Git version. Update Git to 2.36 or newer.".into()));
                }
                plain.replace('\n', "\0")
            }
            Err(error) => return Err(error.into()),
        };
        let main_place = listing
            .split('\0')
            .find_map(|s| s.strip_prefix("worktree "))
            .map(PathBuf::from)
            .unwrap_or_else(|| root.clone());
        let mut place = None;
        for field in listing.split('\0') {
            if let Some(p) = field.strip_prefix("worktree ") {
                place = Some(PathBuf::from(p));
            } else if let Some(name) = field.strip_prefix("branch refs/heads/") {
                if let Some(p) = &place {
                    occupied.insert(name.to_owned(), p.clone());
                }
            } else if let Some(oid) = field.strip_prefix("HEAD ")
                && let Some(p) = &place
                && !oid.bytes().all(|b| b == b'0')
            {
                worktrees.push((p.clone(), parse_oid(oid)?));
            }
        }
        let common = PathBuf::from(
            git.run(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?
                .trim(),
        );
        reservations(&common, &main_place, &mut occupied)?;
        let linked = common.join("worktrees");
        match std::fs::read_dir(&linked) {
            Ok(entries) => {
                for entry in entries {
                    let dir = entry.map_err(|e| io_error(&linked, e))?.path();
                    let backlink = dir.join("gitdir");
                    let text =
                        std::fs::read_to_string(&backlink).map_err(|e| io_error(&backlink, e))?;
                    let path = dir.join(text.trim());
                    if let Some(p) = path.parent() {
                        reservations(&dir, p, &mut occupied)?;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(&linked, e)),
        }
        Ok(Self {
            locals,
            remotes,
            remote_names,
            occupied,
            current,
            head,
            has_working_tree,
            auto_setup_rebase,
            roots,
            worktrees,
            root,
        })
    }

    pub fn trackers(&self, upstream: &str) -> Vec<&str> {
        self.locals
            .iter()
            .filter(|b| b.upstream.as_deref() == Some(upstream))
            .map(|b| b.name.as_str())
            .collect()
    }

    pub fn tracking_parts<'a>(&'a self, typed: &'a str) -> Option<(&'a str, &'a str)> {
        self.remote_names.iter().find_map(|r| {
            typed
                .strip_prefix(&format!("{r}/"))
                .filter(|b| !b.is_empty())
                .map(|b| (r.as_str(), b))
        })
    }

    pub fn suggested_name(&self, upstream: &str) -> String {
        let Some((_, base)) = self.tracking_parts(upstream) else {
            return String::new();
        };
        if !valid_name(base) {
            return String::new();
        }
        let base = if self
            .locals
            .iter()
            .any(|b| base.starts_with(&format!("{}/", b.name)))
        {
            base.replace('/', "-")
        } else {
            base.to_owned()
        };
        if self.name_error(&base).is_none() {
            return base;
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|n| self.name_error(n).is_none())
            .unwrap()
    }

    pub fn name_error(&self, name: &str) -> Option<&'static str> {
        if !valid_name(name) {
            return Some("Enter a valid Git branch name.");
        }
        if self.locals.iter().any(|b| b.name == name) {
            Some("A local branch with this name already exists.")
        } else if self.locals.iter().any(|b| {
            b.name.starts_with(&format!("{name}/")) || name.starts_with(&format!("{}/", b.name))
        }) {
            Some("This name conflicts with an existing branch path.")
        } else {
            None
        }
    }

    pub fn track_error(&self, track: &str) -> Option<&'static str> {
        if track.trim().is_empty() {
            return None;
        }
        match self.tracking_parts(track.trim()) {
            Some((_, name)) if valid_name(&format!("upstream/{name}")) => None,
            Some(_) => Some("Enter a valid remote branch name."),
            None => {
                Some("Use a configured remote followed by a branch name, such as origin/topic.")
            }
        }
    }
}

fn reservations(
    dir: &Path,
    worktree: &Path,
    occupied: &mut HashMap<String, PathBuf>,
) -> Result<(), Error> {
    for name in [
        "rebase-merge/head-name",
        "rebase-apply/head-name",
        "BISECT_START",
    ] {
        let path = dir.join(name);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let name = text.trim().trim_start_matches("refs/heads/");
                if !name.is_empty() && Oid::from_hex(name).is_none() {
                    occupied.insert(name.to_owned(), worktree.to_owned());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(&path, e)),
        }
    }
    Ok(())
}

fn valid_name(name: &str) -> bool {
    // Git's ref-format rules, applied without spawning a process on every UI keystroke.
    !name.is_empty()
        && name != "HEAD"
        && name != "@"
        && !name.starts_with('-')
        && !name.contains("..")
        && !name.contains("@{")
        && !name.ends_with('.')
        && !name
            .chars()
            .any(|c| c <= '\u{20}' || c == '\u{7f}' || "~^:?*[\\".contains(c))
        && name
            .split('/')
            .all(|p| !p.is_empty() && !p.starts_with('.') && !p.ends_with(".lock"))
}

fn parse_oid(s: &str) -> Result<Oid, Error> {
    Oid::from_hex(s).ok_or_else(|| Error::Invalid("Unexpected Git commit id.".into()))
}

fn io_error(path: &Path, source: std::io::Error) -> Error {
    Error::Git(GitError::Read {
        path: path.to_owned(),
        source,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Create {
    pub start: Oid,
    pub name: String,
    /// Empty/None means no upstream; otherwise a configured remote followed by a branch.
    pub track: Option<String>,
    pub switch: bool,
}

/// Creation-field state: the tracking name follows local edits until the user chooses
/// or types a particular remote branch. Selecting a remote starts that automatic mode again.
#[derive(Clone, Debug)]
pub struct CreateDraft {
    name: String,
    remote: Option<String>,
    track_name: String,
    name_edited: bool,
    track_edited: bool,
    suggestion_source: Option<String>,
}

impl CreateDraft {
    pub fn new(catalog: &Catalog, start: Oid, prefer: Option<&str>) -> Self {
        let remote = prefer
            .and_then(|s| catalog.tracking_parts(s))
            .map(|(remote, _)| remote.to_owned())
            .or_else(|| catalog.remote_names.iter().min().cloned());
        let upstream = prefer.or_else(|| {
            catalog
                .remotes
                .iter()
                .find(|r| {
                    r.tip == start
                        && catalog.trackers(&r.name).is_empty()
                        && catalog
                            .tracking_parts(&r.name)
                            .is_some_and(|(r, _)| Some(r) == remote.as_deref())
                })
                .map(|r| r.name.as_str())
        });
        let parts = upstream.and_then(|s| catalog.tracking_parts(s));
        Self {
            name: upstream
                .map(|s| catalog.suggested_name(s))
                .unwrap_or_default(),
            remote,
            track_name: parts
                .map(|(_, branch)| branch.to_owned())
                .unwrap_or_default(),
            name_edited: false,
            // A remote Switch is already an explicit choice of that upstream branch.
            track_edited: prefer.is_some(),
            suggestion_source: upstream.map(str::to_owned),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn remote(&self) -> Option<&str> {
        self.remote.as_deref()
    }
    pub fn track_name(&self) -> &str {
        &self.track_name
    }

    pub fn upstream(&self) -> Option<String> {
        self.remote
            .as_ref()
            .map(|remote| format!("{remote}/{}", self.track_name.trim()))
    }

    pub fn set_name(&mut self, name: String) {
        self.name = name;
        self.name_edited = true;
        self.follow_name();
    }

    pub fn set_remote(&mut self, remote: Option<String>) {
        self.remote = remote;
        self.track_edited = false;
        self.suggestion_source = None;
        self.follow_name();
    }

    /// A selection counts even when it matches the current automatically generated name.
    pub fn set_track_name(&mut self, catalog: &Catalog, branch: String) {
        self.track_name = branch;
        self.track_edited = true;
        self.suggestion_source = self.upstream();
        let suggested = self.suggested_name(catalog);
        if !self.name_edited && !suggested.is_empty() {
            self.name = suggested;
        }
    }

    pub fn can_restore_track_name(&self) -> bool {
        self.remote.is_some() && (self.track_edited || self.track_name != self.name)
    }

    /// Resume following local-name edits without changing the local name or selected remote.
    pub fn restore_track_name(&mut self) {
        self.track_edited = false;
        self.follow_name();
    }

    pub fn suggested_name(&self, catalog: &Catalog) -> String {
        self.suggestion_source
            .as_deref()
            .map(|s| catalog.suggested_name(s))
            .unwrap_or_default()
    }

    pub fn restore_suggested_name(&mut self, catalog: &Catalog) {
        let suggested = self.suggested_name(catalog);
        if !suggested.is_empty() {
            self.name = suggested;
            self.name_edited = false;
            self.follow_name();
        }
    }

    pub fn remote_branches(&self, catalog: &Catalog) -> Vec<String> {
        catalog
            .remotes
            .iter()
            .filter_map(|r| {
                let (remote, branch) = catalog.tracking_parts(&r.name)?;
                (Some(remote) == self.remote()).then(|| branch.to_owned())
            })
            .collect()
    }

    fn follow_name(&mut self) {
        if self.remote.is_none() {
            self.track_name.clear();
        } else if !self.track_edited {
            self.track_name.clone_from(&self.name);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Create(Create),
    Switch(String),
    Delete { name: String, tip: Oid },
}

impl Action {
    pub fn label(&self) -> String {
        match self {
            Self::Create(c) => {
                if c.switch {
                    format!("Create and switch to {}", c.name)
                } else {
                    format!("Create branch {}", c.name)
                }
            }
            Self::Switch(b) => format!("Switch to {b}"),
            Self::Delete { name, .. } => format!("Delete branch {name}"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Failed(String),
    #[error("Operation cancelled.")]
    Cancelled,
}

/// Cancels the whole Git process group on Unix, or process tree on Windows, including hooks.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<Mutex<(bool, Option<u32>)>>);

impl Cancel {
    pub fn cancel(&self) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.0 {
            return;
        }
        state.0 = true;
        if let Some(pid) = state.1 {
            #[cfg(unix)]
            {
                signal_group(pid, "-TERM");
                let pending = self.0.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    let state = pending
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    // Keep the group registered until its output pipes close, even if Git
                    // has already exited. Hooks can ignore TERM and keep those pipes open.
                    if state.1 == Some(pid) {
                        signal_group(pid, "-KILL");
                    }
                });
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .creation_flags(0x0800_0000)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
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

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug)]
pub struct Step {
    pub args: Vec<String>,
    pub output: String,
    pub success: bool,
}

/// The approval is tied to the source HEAD, deletion tip and exact endangered commit ids.
#[derive(Clone, Debug)]
pub struct Warning {
    pub action: Action,
    pub commits: Vec<Oid>,
    pub repo: Arc<Repo>,
    pub commands: Vec<Vec<String>>,
    head: Option<Oid>,
}

#[derive(Debug)]
pub enum Outcome {
    Done(Report),
    Warning(Warning),
    Failed { error: Error, report: Report },
}

#[derive(Clone, Debug)]
pub struct Branches {
    path: PathBuf,
}

impl Branches {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Commands shown by the form. Execution freezes its start by full OID and validates again.
    pub fn commands(catalog: &Catalog, action: &Action) -> Result<Vec<Vec<String>>, Error> {
        let words = |s: &[&str]| s.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        match action {
            Action::Create(c) => {
                if let Some(error) = catalog.name_error(&c.name) {
                    return Err(Error::Invalid(error.into()));
                }
                let start = c.start.to_hex();
                let create = if c.switch {
                    words(&["switch", "--create", &c.name, "--no-track", &start])
                } else {
                    words(&["branch", "--no-track", "--", &c.name, &start])
                };
                let mut commands = vec![create];
                if let Some(track) = c.track.as_deref().filter(|s| !s.trim().is_empty()) {
                    let Some((remote, branch)) = catalog.tracking_parts(track.trim()) else {
                        return Err(Error::Invalid("Use a configured remote followed by a branch name, such as origin/topic.".into()));
                    };
                    if catalog.track_error(track).is_some() {
                        return Err(Error::Invalid("Enter a valid remote branch name.".into()));
                    }
                    if catalog.remotes.iter().any(|r| r.name == track.trim()) {
                        commands.push(words(&[
                            "branch",
                            &format!("--set-upstream-to=refs/remotes/{}", track.trim()),
                            "--",
                            &c.name,
                        ]));
                        return Ok(commands);
                    }
                    // Configuration works before a remote ref exists, and does not change the
                    // selected start commit even when the upstream points somewhere else.
                    commands.push(words(&[
                        "config",
                        "--local",
                        "--replace-all",
                        &format!("branch.{}.remote", c.name),
                        remote,
                    ]));
                    commands.push(words(&[
                        "config",
                        "--local",
                        "--replace-all",
                        &format!("branch.{}.merge", c.name),
                        &format!("refs/heads/{branch}"),
                    ]));
                    if catalog.auto_setup_rebase {
                        commands.push(words(&[
                            "config",
                            "--local",
                            "--replace-all",
                            &format!("branch.{}.rebase", c.name),
                            "true",
                        ]));
                    }
                }
                Ok(commands)
            }
            Action::Switch(name) => Ok(vec![words(&["switch", "--no-guess", "--", name])]),
            Action::Delete { name, .. } => Ok(vec![words(&["branch", "-D", "--", name])]),
        }
    }

    pub fn execute(&self, action: Action, approval: Option<&Warning>, cancel: &Cancel) -> Outcome {
        let mut report = Report::default();
        let requested = action.clone();
        match self.execute_inner(action, approval, cancel, &mut report) {
            Ok(Some(warning)) => Outcome::Warning(warning),
            Ok(None) => Outcome::Done(report),
            Err(error) => {
                let error = if let Action::Create(c) = requested
                    && !report.steps.is_empty()
                    && Git::new(&self.path)
                        .query(&["rev-parse", "--verify", &format!("refs/heads/{}", c.name)])
                        .ok()
                        .flatten()
                        .is_some_and(|tip| tip == c.start.to_hex())
                {
                    let checked_out = Git::new(&self.path)
                        .query(&["symbolic-ref", "--quiet", "HEAD"])
                        .ok()
                        .flatten()
                        .is_some_and(|head| head == format!("refs/heads/{}", c.name));
                    let detail = if report.steps.first().is_some_and(|s| s.success) {
                        format!("Configuring its upstream did not finish: {error}")
                    } else {
                        error.to_string()
                    };
                    Error::Failed(format!(
                        "Branch {} now exists{}. {detail}",
                        c.name,
                        if checked_out {
                            " and is checked out"
                        } else {
                            ""
                        }
                    ))
                } else {
                    error
                };
                Outcome::Failed { error, report }
            }
        }
    }

    fn execute_inner(
        &self,
        action: Action,
        approval: Option<&Warning>,
        cancel: &Cancel,
        report: &mut Report,
    ) -> Result<Option<Warning>, Error> {
        let git = Git::new(&self.path);
        let mut catalog = Catalog::load(&self.path)?;
        let mut commands = Self::commands(&catalog, &action)?;
        let switching = matches!(
            &action,
            Action::Switch(_) | Action::Create(Create { switch: true, .. })
        );
        if switching && !catalog.has_working_tree {
            return Err(Error::Invalid(
                "This repository has no working tree to switch.".into(),
            ));
        }
        match &action {
            Action::Switch(name) => {
                if !catalog.locals.iter().any(|b| b.name == *name) {
                    return Err(Error::Invalid(
                        "The local branch no longer exists. Reload and try again.".into(),
                    ));
                }
                if catalog.current.as_ref() == Some(name) {
                    return Ok(None);
                }
                check_occupied(&catalog, name)?;
            }
            Action::Delete { name, tip } => {
                check_occupied(&catalog, name)?;
                if !catalog
                    .locals
                    .iter()
                    .any(|b| b.name == *name && b.tip == *tip)
                {
                    return Err(Error::Invalid(
                        "The branch changed. Reload and review it before deleting.".into(),
                    ));
                }
                let safe = vec!["branch".into(), "-d".into(), "--".into(), name.clone()];
                if run(&git, safe, cancel, report)? {
                    return Ok(None);
                }
                // A refusal on a merged branch is unrelated to commit loss (e.g. a lock or
                // permissions). Check Git's merge predicate instead of parsing its stderr.
                let refreshed = Catalog::load(&self.path)?;
                check_occupied(&refreshed, name)?;
                if !refreshed
                    .locals
                    .iter()
                    .any(|b| b.name == *name && b.tip == *tip)
                {
                    return Err(Error::Invalid(
                        "The branch changed. Reload and review it before deleting.".into(),
                    ));
                }
                let upstream = git.query(&[
                    "rev-parse",
                    "--verify",
                    &format!("refs/heads/{name}@{{upstream}}^{{commit}}"),
                ])?;
                let merged_into = upstream
                    .map(|s| parse_oid(&s))
                    .transpose()?
                    .or(refreshed.head);
                if let Some(into) = merged_into
                    && git
                        .query(&["merge-base", "--is-ancestor", &tip.to_hex(), &into.to_hex()])?
                        .is_some()
                {
                    return Err(Error::Failed(report.steps.last().unwrap().output.clone()));
                }
                if merged_into.is_none() {
                    return Err(Error::Failed(report.steps.last().unwrap().output.clone()));
                }
                catalog = refreshed;
            }
            Action::Create(c) => {
                // Git is the final ref-name authority as well as the namespace collision guard.
                git.run(&["check-ref-format", &format!("refs/heads/{}", c.name)])?;
            }
        }
        let (start, excluded_ref, departing) = match &action {
            Action::Delete { name, tip } => (Some(*tip), Some(format!("refs/heads/{name}")), false),
            _ if switching && catalog.current.is_none() => (catalog.head, None, true),
            _ => (None, None, false),
        };
        if let Some(start) = start {
            let future_root = match &action {
                Action::Create(c) => Some(c.start),
                _ => None,
            };
            let commits = lost_commits(
                &git,
                &catalog,
                start,
                excluded_ref.as_deref(),
                departing,
                future_root,
            )?;
            if !commits.is_empty()
                && !approval.is_some_and(|w| {
                    w.action == action && w.head == catalog.head && w.commits == commits
                })
            {
                let repo = Arc::new(git.load()?);
                if commits.iter().any(|oid| repo.lookup(oid).is_none()) {
                    return Err(Error::Invalid(
                        "The repository changed while checking lost commits. Reload and try again."
                            .into(),
                    ));
                }
                return Ok(Some(Warning {
                    action,
                    commits,
                    head: catalog.head,
                    repo,
                    commands: commands.clone(),
                }));
            }
        }
        for args in commands.drain(..) {
            if !run(&git, args, cancel, report)? {
                return Err(Error::Failed(report.steps.last().unwrap().output.clone()));
            }
        }
        Ok(None)
    }
}

fn check_occupied(catalog: &Catalog, name: &str) -> Result<(), Error> {
    if catalog.current.as_deref() == Some(name) {
        return Err(Error::Invalid(format!(
            "Branch {name} is the current branch."
        )));
    }
    if let Some(place) = catalog.occupied.get(name) {
        return Err(Error::Invalid(format!(
            "Branch {name} is in use by {}.",
            place.display()
        )));
    }
    Ok(())
}

fn lost_commits(
    git: &Git,
    catalog: &Catalog,
    start: Oid,
    excluded_ref: Option<&str>,
    departing: bool,
    future_root: Option<Oid>,
) -> Result<Vec<Oid>, Error> {
    let mut protected: HashSet<Oid> = catalog
        .roots
        .iter()
        .filter(|(name, _)| Some(name.as_str()) != excluded_ref)
        .map(|(_, oid)| *oid)
        .collect();
    protected.extend(
        catalog
            .worktrees
            .iter()
            .filter(|(p, _)| !departing || *p != catalog.root)
            .map(|(_, oid)| *oid),
    );
    let mut input = format!("{}\n", start.to_hex());
    if let Some(oid) = future_root {
        protected.insert(oid);
    }
    for oid in protected {
        input.push_str(&format!("^{}\n", oid.to_hex()));
    }
    let result = git.run_with_input(&["rev-list", "--stdin"], input)?;
    let mut commits = result
        .lines()
        .map(parse_oid)
        .collect::<Result<Vec<_>, _>>()?;
    commits.sort_by_key(|o| o.to_hex());
    Ok(commits)
}

fn run(git: &Git, args: Vec<String>, cancel: &Cancel, report: &mut Report) -> Result<bool, Error> {
    let mut state = cancel
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if state.0 {
        return Err(Error::Cancelled);
    }
    let mut child = git
        .operation_command(&args)
        .spawn()
        .map_err(GitError::Spawn)?;
    state.1 = Some(child.id());
    drop(state);
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stderr.read_to_end(&mut b);
        b
    });
    let mut output = Vec::new();
    let read_result = stdout.read_to_end(&mut output);
    let error = errors.join().unwrap_or_default();
    let status = child.wait();
    let mut state = cancel
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.1 = None;
    let status = status.map_err(GitError::Spawn)?;
    output.extend(error);
    report.steps.push(Step {
        args,
        output: String::from_utf8_lossy(&output).trim().to_owned(),
        success: status.success(),
    });
    if state.0 {
        return Err(Error::Cancelled);
    }
    read_result.map_err(GitError::Spawn)?;
    Ok(status.success())
}

/// Display only: execution always passes an argument vector directly to Git.
pub fn command_text(args: &[String]) -> String {
    let quote = |s: &str| {
        if !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/_-.:=".contains(&b))
        {
            s.to_owned()
        } else {
            format!("'{}'", s.replace('\'', "'\\''"))
        }
    };
    format!(
        "git {}",
        args.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" ")
    )
}
