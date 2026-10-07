//! Loading a [`Repo`] snapshot by running the `git` command-line tool.
//!
//! We shell out to `git` rather than linking a git library: it is always present where
//! parterre is useful, honours every repository configuration (worktrees, alternates,
//! packed refs, sha256, ...), and `git log` streams tens of thousands of commits in
//! milliseconds.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use parterre_util::Cancel;

use crate::blame::{Blame, BlameOptions, BlameSpec};
use crate::changed_files::{ChangedFile, parse_diff_tree};
use crate::file_diff::{Content, FileDiffSpec, LoadedDiff, Rev, Version, decode};
use crate::file_history::{self, FileLog};
use crate::oid::Oid;
use crate::repo::{Commit, CommitIx, DEFAULT_ABBREV_LEN, GitRef, Head, RefKind, Repo, Worktree};
use crate::worktree_folder::same_path;

mod program;
mod version;

pub use program::program;
pub use version::{MINIMUM_VERSION, version};

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("could not run git ({0}); is git installed and on PATH?")]
    Spawn(#[source] std::io::Error),
    /// The version `git --version` printed, older than [`MINIMUM_VERSION`].
    #[error(
        "git {0} is too old; parterre needs git {major}.{minor} or newer",
        major = MINIMUM_VERSION.0,
        minor = MINIMUM_VERSION.1
    )]
    TooOld(String),
    #[error("`git {args}` failed: {stderr}")]
    Failed { args: String, stderr: String },
    #[error("{0} is not inside a git repository")]
    NotARepository(PathBuf),
    /// git found a bare repository by itself and won't use it: `safe.bareRepository` is
    /// `explicit`, Git 3.0's default. `open` is the folder that does open (#228).
    #[error(
        "git won't use discovered bare repository {found} (safe.bareRepository is 'explicit'); \
         open {open}"
    )]
    BareRepositoryRefused { found: PathBuf, open: PathBuf },
    #[error("unexpected output from git: {0}")]
    Parse(String),
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// Stopped with [`Cancel::cancel`].
    #[error("cancelled")]
    Cancelled,
}

/// Fails with [`GitError::Cancelled`] once `cancel` is.
fn check(cancel: &Cancel) -> Result<(), GitError> {
    if cancel.is_cancelled() {
        return Err(GitError::Cancelled);
    }
    Ok(())
}

/// A handle for running git commands against one repository.
#[derive(Clone, Debug)]
pub struct Git {
    dir: PathBuf,
    /// Named with `--git-dir`: a bare repository git would not discover (#228).
    pass_git_dir: bool,
}

/// Separator for `for-each-ref` fields (ref names cannot contain control characters).
const FIELD: char = '\x1f';
/// `git log -z` format: NUL-separated fields, so subjects and names may contain anything.
const LOG_FORMAT: &str = "--format=%H%x00%P%x00%T%x00%an%x00%ae%x00%at%x00%ad%x00%ct%x00%s";
const LOG_FIELDS: usize = 9;
const EMPTY_TREE_SHA1: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const EMPTY_TREE_SHA256: &str = "6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321";

/// git with no arguments yet: piped output, the C locale, the user's shell's `PATH` and no
/// console window.
fn git_command() -> Command {
    let mut cmd = Command::new(program());
    crate::shell_path::apply(&mut cmd)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        // parterre is a GUI-subsystem app on Windows; without this every git invocation
        // would flash a console window.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Whether `dir` is a git dir that git would take for a bare repository if it found it by
/// itself, which `safe.bareRepository=explicit` (Git 3.0's default) forbids. That is any but a
/// work tree's `.git`, or a linked worktree's or a submodule's git dir inside one: git lets
/// those through (`is_implicit_bare_repo` in setup.c), and `--git-dir` would make a `.git`
/// folder its own work tree. Judged by the real path, as git does, so `.` inside a `.git` counts.
fn is_implicit_bare(dir: &Path) -> bool {
    let looks_like_git_dir =
        dir.join("HEAD").is_file() && dir.join("objects").is_dir() && dir.join("refs").is_dir();
    if !looks_like_git_dir {
        return false;
    }
    let Ok(real) = std::fs::canonicalize(dir) else {
        return false;
    };
    let names: Vec<_> = real.components().map(|c| c.as_os_str()).collect();
    let inside_dot_git = names.last() == Some(&OsStr::new(".git"))
        || names
            .windows(2)
            .any(|w| w[0] == ".git" && (w[1] == "worktrees" || w[1] == "modules"));
    !inside_dot_git
}

/// Why git could not open `dir`, from what it printed.
fn not_opened(dir: &Path, stderr: &str) -> GitError {
    let stderr = stderr.trim();
    let refused = stderr.lines().find_map(|line| {
        let rest = line.strip_prefix("fatal: cannot use bare repository '")?;
        Some(PathBuf::from(rest.split_once("' (safe.bareRepository")?.0))
    });
    if let Some(found) = refused {
        // Before 2.45 git refuses a work tree's `.git` too; the work tree opens.
        let open = match found.parent() {
            Some(work) if found.file_name() == Some(OsStr::new(".git")) => work.to_owned(),
            _ => found.clone(),
        };
        return GitError::BareRepositoryRefused { found, open };
    }
    if stderr.starts_with("fatal: not a git repository")
        || stderr.starts_with("fatal: cannot change to")
    {
        return GitError::NotARepository(dir.to_owned());
    }
    GitError::Failed {
        args: format!("-C {} rev-parse", dir.display()),
        stderr: stderr.to_owned(),
    }
}

/// Where the repository around a folder keeps things, from one `rev-parse`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Location {
    /// The git dir: a work tree's `.git`, a linked worktree's `.git/worktrees/<name>`, or a
    /// bare repository itself.
    pub git_dir: PathBuf,
    /// The git dir the refs are kept in: the main worktree's, from a linked worktree;
    /// otherwise the git dir itself.
    pub common_dir: PathBuf,
    /// The root of the work tree the folder is in: none in a bare repository or inside a
    /// `.git` folder.
    pub work_tree: Option<PathBuf>,
}

impl Location {
    /// The repository root: the work tree, or the git dir where there is none.
    pub fn root(&self) -> &Path {
        self.work_tree.as_deref().unwrap_or(&self.git_dir)
    }
}

/// What [`Git::location`]'s `rev-parse` printed, as far as it got: with no work tree it stops
/// at `--show-toplevel`, after `--is-inside-work-tree` has said `false`. `None` for anything
/// else cut short, or nothing at all (not a repository).
fn parse_location(stdout: &str, success: bool) -> Option<Location> {
    let mut lines = stdout.lines();
    let inside_work_tree = lines.next()? == "true";
    let git_dir = PathBuf::from(lines.next()?);
    let common_dir = PathBuf::from(lines.next()?);
    let work_tree = match (inside_work_tree, lines.next()) {
        (true, Some(top)) if success => Some(PathBuf::from(top)),
        (false, _) => None,
        _ => return None,
    };
    Some(Location {
        git_dir,
        common_dir,
        work_tree,
    })
}

/// git's settings as `git config --list` prints them, read in one call for whatever a loader
/// or a dialog wants to know, where `git config --get` is a call per key (#309).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// Each setting's key, its section and name in lower case as git prints them, and its
    /// value, in git's order: a later one overrides an earlier one. A key set without a value
    /// (`[core]\n\tbare`) has none.
    entries: Vec<(String, Option<String>)>,
}

impl Config {
    /// `key`'s value as `git config --get` gives it: the last one set, the section and name
    /// matched regardless of case and the subsection as it is; empty for a key set without
    /// a value.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.last(key).map(|value| value.unwrap_or(""))
    }

    /// `key` as `git config --bool` reads it: `true`, `yes`, `on`, `1` or set without a
    /// value; `false`, `no`, `off`, `0` or empty. `None` if unset, or something else.
    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.last(key)?.map(str::to_ascii_lowercase).as_deref() {
            None | Some("true" | "yes" | "on" | "1") => Some(true),
            Some("false" | "no" | "off" | "0" | "") => Some(false),
            _ => None,
        }
    }

    /// The remotes' names, as `git remote` lists them: every `remote.<name>.<setting>` names
    /// one.
    pub fn remotes(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .entries
            .iter()
            .filter_map(|(key, _)| key.strip_prefix("remote.")?.rsplit_once('.'))
            .map(|(name, _)| name.to_owned())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    fn last(&self, key: &str) -> Option<Option<&str>> {
        let key = canonical_key(key);
        self.entries
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, value)| value.as_deref())
    }
}

/// `key` with its section and name in lower case, as git prints keys, and any subsection
/// between them as it is.
fn canonical_key(key: &str) -> String {
    match (key.find('.'), key.rfind('.')) {
        (Some(first), Some(last)) if first < last => format!(
            "{}.{}.{}",
            key[..first].to_ascii_lowercase(),
            &key[first + 1..last],
            key[last + 1..].to_ascii_lowercase()
        ),
        _ => key.to_ascii_lowercase(),
    }
}

/// `git config -z --list`'s output: a NUL after each setting, its key and value separated by
/// the first newline, and no newline for a key set without a value.
fn parse_config(out: &[u8]) -> Config {
    let entries = out
        .split(|&b| b == 0)
        .filter(|entry| !entry.is_empty())
        .map(
            |entry| match String::from_utf8_lossy(entry).split_once('\n') {
                Some((key, value)) => (key.to_owned(), Some(value.to_owned())),
                None => (String::from_utf8_lossy(entry).into_owned(), None),
            },
        )
        .collect();
    Config { entries }
}

/// `cat-file --batch`'s output for `count` objects asked for: `<oid> <type> <size>`, a
/// newline, the contents and a newline, each. `None` if it reads otherwise, or an object is
/// missing.
fn parse_batch(out: &[u8], count: usize) -> Option<Vec<Vec<u8>>> {
    let mut objects = Vec::with_capacity(count);
    let mut rest = out;
    for _ in 0..count {
        let newline = rest.iter().position(|&b| b == b'\n')?;
        let header = std::str::from_utf8(&rest[..newline]).ok()?;
        rest = &rest[newline + 1..];
        let mut fields = header.split(' ');
        let (_oid, kind) = (fields.next()?, fields.next()?);
        if kind == "missing" {
            return None;
        }
        let size: usize = fields.next()?.parse().ok()?;
        objects.push(rest.get(..size)?.to_vec());
        rest = rest.get(size + 1..)?;
    }
    Some(objects)
}

impl Git {
    /// git's settings, in one call.
    pub fn config(&self) -> Result<Config, GitError> {
        Ok(parse_config(&self.run_bytes(&["config", "-z", "--list"])?))
    }

    /// The contents of blobs, in one call.
    pub(crate) fn blobs(&self, oids: &[Oid]) -> Result<Vec<Vec<u8>>, GitError> {
        if oids.is_empty() {
            return Ok(Vec::new());
        }
        let input: String = oids.iter().map(|o| format!("{o}\n")).collect();
        let out = self.run_with_input_bytes(&["cat-file", "--batch"], input)?;
        parse_batch(&out, oids.len())
            .ok_or_else(|| GitError::Parse("unexpected output from cat-file --batch".into()))
    }
}

impl Git {
    pub fn new(dir: impl Into<PathBuf>) -> Git {
        let dir = dir.into();
        let pass_git_dir = is_implicit_bare(&dir);
        Git { dir, pass_git_dir }
    }

    /// The folder git runs in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn command<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = git_command();
        cmd.arg("-C").arg(&self.dir);
        if self.pass_git_dir {
            cmd.arg("--git-dir=.");
        }
        cmd.args(["-c", "core.quotepath=off"])
            .args(["-c", "log.showSignature=false"])
            .args(["-c", "i18n.logOutputEncoding=UTF-8"])
            .args(["-c", "color.ui=false"])
            .args(args)
            // Read-only tool: never take the index lock for opportunistic refreshes.
            .env("GIT_OPTIONAL_LOCKS", "0");
        cmd
    }

    fn output(&self, args: &[&str]) -> Result<Output, GitError> {
        self.command(args).output().map_err(GitError::Spawn)
    }

    /// Repository-changing porcelain: locks enabled and the user's locale inherited.
    /// Kept separate from the viewer's read-only runner.
    pub(crate) fn operation_command(&self, args: &[String]) -> Command {
        let mut cmd = self.command(args);
        cmd.env("GIT_OPTIONAL_LOCKS", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", ":");
        if let Some(locale) = std::env::var_os("LC_ALL") {
            cmd.env("LC_ALL", locale);
        } else {
            cmd.env_remove("LC_ALL");
        }
        cmd
    }

    /// Runs git and returns stdout, failing on a non-zero exit status. Public for the crates
    /// built on this one (the forge client); the app goes through the methods below.
    pub fn run(&self, args: &[&str]) -> Result<String, GitError> {
        let out = self.output(args)?;
        if !out.status.success() {
            return Err(GitError::Failed {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Runs git and returns stdout as bytes, failing on a non-zero exit status.
    pub(crate) fn run_bytes(&self, args: &[&str]) -> Result<Vec<u8>, GitError> {
        let out = self.output(args)?;
        if !out.status.success() {
            return Err(GitError::Failed {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            });
        }
        Ok(out.stdout)
    }

    /// Runs git with `input` on stdin and returns stdout, failing on a non-zero exit status.
    pub(crate) fn run_with_input(
        &self,
        args: &[&str],
        input: impl Into<Vec<u8>>,
    ) -> Result<String, GitError> {
        let out = self.run_with_input_bytes(args, input)?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// Runs git with `input` on stdin and returns stdout as bytes, failing on a non-zero exit
    /// status.
    fn run_with_input_bytes(
        &self,
        args: &[&str],
        input: impl Into<Vec<u8>>,
    ) -> Result<Vec<u8>, GitError> {
        use std::io::Write as _;
        let input = input.into();
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .spawn()
            .map_err(GitError::Spawn)?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        // Write from another thread so a large output cannot deadlock against a full pipe.
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
        let out = child.wait_with_output().map_err(GitError::Spawn)?;
        let _ = writer.join();
        if !out.status.success() {
            return Err(GitError::Failed {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            });
        }
        Ok(out.stdout)
    }

    /// Runs git and returns stdout as bytes, failing on a non-zero exit status, unless `cancel`
    /// kills it first.
    fn run_cancellable(&self, args: &[&str], cancel: &Cancel) -> Result<Vec<u8>, GitError> {
        let pipes = cancel
            .spawn(&mut self.command(args))
            .map_err(GitError::Spawn)?
            .ok_or(GitError::Cancelled)?;
        let mut stdout = pipes.stdout.expect("stdout is piped");
        let mut stderr = pipes.stderr.expect("stderr is piped");
        // Read stderr alongside, so that neither pipe can fill up and stall git.
        let errors = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf);
            buf
        });
        let mut out = Vec::new();
        let read = stdout.read_to_end(&mut out);
        let stderr = errors.join().unwrap_or_default();
        // Gone if `cancel` killed it (and waited for it).
        let Some(mut child) = cancel.release() else {
            return Err(GitError::Cancelled);
        };
        let status = child.wait().map_err(GitError::Spawn)?;
        read.map_err(GitError::Spawn)?;
        if !status.success() {
            return Err(GitError::Failed {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&stderr).trim().to_owned(),
            });
        }
        Ok(out)
    }

    /// Runs git and returns trimmed stdout, or `None` on a non-zero exit status (for queries
    /// such as `symbolic-ref -q` that signal "no" through the exit code). Public as [`Git::run`].
    pub fn query(&self, args: &[&str]) -> Result<Option<String>, GitError> {
        let out = self.output(args)?;
        Ok(out
            .status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned()))
    }

    /// Where the repository around [`Git::dir`] keeps things, or why git couldn't open it.
    pub fn location(&self) -> Result<Location, GitError> {
        // One call for all of it: `--show-toplevel` fails after the others have printed where
        // there is no work tree (a bare repository, or inside a `.git` folder), which is how
        // git says so (#309).
        let out = self.output(&[
            "rev-parse",
            "--path-format=absolute",
            "--is-inside-work-tree",
            "--git-dir",
            "--git-common-dir",
            "--show-toplevel",
        ])?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        match parse_location(&stdout, out.status.success()) {
            Some(location) => Ok(location),
            None if out.status.success() => {
                Err(GitError::Parse(format!("rev-parse printed {stdout:?}")))
            }
            None => Err(not_opened(&self.dir, &String::from_utf8_lossy(&out.stderr))),
        }
    }

    /// Resolves the repository root: the working tree, or the git dir for a bare repository.
    pub fn repo_root(&self) -> Result<PathBuf, GitError> {
        Ok(self.location()?.root().to_owned())
    }

    /// Loads all refs (notes excluded) and every commit reachable from them.
    ///
    /// Refs and HEAD are read first and the log is then walked from exactly those commits, so
    /// a concurrent fetch cannot leave refs pointing at commits that were not loaded.
    pub fn load(&self) -> Result<Repo, GitError> {
        let location = self.location()?;
        let git = Git::new(location.root());
        let listing = git.list(location)?;
        let root = listing.location.root().to_owned();
        let has_working_tree = listing.location.work_tree.is_some();
        let (head_branch, head_oid) = (&listing.head_branch, listing.head);

        let mut starts: Vec<Oid> = listing.refs.iter().map(|r| r.commit).collect();
        starts.extend(head_oid);
        // Other worktrees' detached HEADs, which no ref may reach.
        starts.extend(listing.worktrees.iter().filter_map(|w| w.head));
        starts.sort_unstable();
        starts.dedup();
        let (log, abbrev_len) = std::thread::scope(|s| {
            // Asked alongside the log, so it adds no time to the load.
            let abbrev = s.spawn(|| git.abbrev_len(&starts));
            let log = if starts.is_empty() {
                Ok((Vec::new(), HashMap::new()))
            } else {
                let input: String = starts.iter().map(|o| format!("{o}\n")).collect();
                git.run_with_input(
                    &[
                        "log",
                        "--no-color",
                        "--no-decorate",
                        "--date=format-local:%Y-%m-%d %H:%M",
                        "-z",
                        LOG_FORMAT,
                        "--stdin",
                    ],
                    input,
                )
                .and_then(|log| parse_log(&log))
            };
            (log, abbrev.join().unwrap_or(DEFAULT_ABBREV_LEN))
        });
        let (commits, by_oid) = log?;

        let lookup = |oid: &Oid| by_oid.get(oid).copied();
        let head = match (head_branch, head_oid) {
            (Some(branch), target) => Head::Branch {
                name: branch.clone(),
                target: target.and_then(|o| lookup(&o)),
            },
            (None, Some(oid)) => Head::Detached(
                lookup(&oid).ok_or_else(|| GitError::Parse(format!("HEAD {oid} not in log")))?,
            ),
            (None, None) => {
                return Err(GitError::Parse(
                    "HEAD is neither a branch nor a commit".into(),
                ));
            }
        };

        let configured: Vec<(String, String)> = listing
            .refs
            .iter()
            .filter(|r| r.full_name.starts_with("refs/heads/"))
            .filter_map(|r| Some((r.full_name.clone(), r.upstream.clone()?)))
            .collect();
        let mut refs: Vec<GitRef> = listing
            .refs
            .iter()
            .filter_map(|r| {
                let target = lookup(&r.commit)?;
                let (kind, name) = classify_ref(&r.full_name);
                Some(GitRef {
                    is_head: Some(r.full_name.as_str()) == head_branch.as_deref(),
                    full_name: r.full_name.clone(),
                    name,
                    kind,
                    target,
                    annotated: r.annotated,
                })
            })
            .collect();
        if let Head::Detached(c) = head {
            refs.push(GitRef {
                full_name: "HEAD".into(),
                name: "HEAD".into(),
                kind: RefKind::DetachedHead,
                target: c,
                annotated: false,
                is_head: true,
            });
        }
        let worktrees = listing
            .worktrees
            .iter()
            .filter(|w| !w.bare)
            .map(|w| {
                let path = PathBuf::from(&w.path);
                Worktree {
                    head: w.head.and_then(|o| lookup(&o)),
                    branch: w.branch.clone(),
                    locked: w.locked.is_some(),
                    // A locked worktree is never prunable, even with its folder gone.
                    missing: w.prunable || !path.is_dir(),
                    open: has_working_tree && same_path(&path, &root),
                    path,
                }
            })
            .collect();
        let default_branch = listing
            .symrefs
            .iter()
            .find(|(name, _)| name == "refs/remotes/origin/HEAD")
            .map(|(_, target)| target.clone());
        let mut repo = Repo::new(root, commits, refs, head);
        repo.abbrev_len = abbrev_len;
        repo.has_working_tree = has_working_tree;
        repo.worktrees = worktrees;
        repo.default_branch = default_branch;
        repo.upstreams = crate::upstream::load(&git, &repo, &configured);
        repo.listing = listing;
        Ok(repo)
    }

    /// The refs and worktrees of the repository at `location`, and HEAD: what [`Git::load`]
    /// walks the log from, and what the branch catalogue is built from (#310). Run in its
    /// root.
    pub(crate) fn list(&self, location: Location) -> Result<Listing, GitError> {
        let root = location.root().to_owned();
        let ref_format = format!(
            "--format=%(refname){FIELD}%(objecttype){FIELD}%(objectname){FIELD}%(*objecttype){FIELD}%(*objectname){FIELD}%(symref){FIELD}%(upstream){FIELD}%(upstream:remotename){FIELD}%(upstream:remoteref)"
        );
        let (listing, worktrees) = std::thread::scope(|s| {
            // Listed alongside the refs, and before the walk, which starts from their HEADs
            // too.
            let worktrees = s.spawn(|| self.worktrees(&root));
            (
                self.run(&["for-each-ref", &ref_format]),
                worktrees.join().expect("worktree listing"),
            )
        });
        let (mut raw_refs, symrefs) = parse_refs(&listing?);
        // Tags of tags: let git peel them all the way to a commit.
        let nested: Vec<usize> = (0..raw_refs.len())
            .filter(|&i| raw_refs[i].commit.is_none())
            .collect();
        if !nested.is_empty() {
            let input: String = nested
                .iter()
                .map(|&i| format!("{}^{{commit}}\n", raw_refs[i].full_name))
                .collect();
            let out = self.run_with_input(&["cat-file", "--batch-check"], input)?;
            for (&i, line) in nested.iter().zip(out.lines()) {
                if let Some((oid, "commit")) = line
                    .split_once(' ')
                    .map(|(o, rest)| (o, rest.split(' ').next().unwrap_or("")))
                {
                    raw_refs[i].commit = Oid::from_hex(oid);
                }
            }
        }
        let refs: Vec<ListedRef> = raw_refs
            .into_iter()
            .filter_map(|r| {
                Some(ListedRef {
                    commit: r.commit?,
                    full_name: r.full_name,
                    annotated: r.annotated,
                    upstream: r.upstream,
                    upstream_remote: r.upstream_remote,
                })
            })
            .collect();
        let (worktrees, worktree_error) = match worktrees {
            Ok(worktrees) => (worktrees, None),
            Err(error) => (Vec::new(), Some(error)),
        };

        // HEAD's branch and commit are in the worktree listing, for the open worktree. Without
        // one to match (a bare repository, or inside a `.git` folder) git is asked: the branch,
        // and its commit unless the ref listing has it (#309).
        let open = worktrees
            .iter()
            .filter(|_| location.work_tree.is_some())
            .find(|w| same_path(Path::new(&w.path), &root));
        let (head_branch, head) = match open {
            Some(w) => (w.branch.clone(), w.head),
            None => {
                let branch = self.query(&["symbolic-ref", "-q", "HEAD"])?;
                let listed = branch
                    .as_deref()
                    .and_then(|branch| refs.iter().find(|r| r.full_name == branch))
                    .map(|r| r.commit);
                let oid = match listed {
                    Some(oid) => Some(oid),
                    None => self
                        .query(&["rev-parse", "-q", "--verify", "HEAD^{commit}"])?
                        .and_then(|s| Oid::from_hex(&s)),
                };
                (branch, oid)
            }
        };
        Ok(Listing {
            location,
            refs,
            symrefs,
            worktrees,
            worktree_error,
            head_branch,
            head,
        })
    }

    /// `git worktree list --porcelain -z`'s worktrees, or the older form's where git lacks
    /// `-z` (before 2.36: Ubuntu 22.04 has 2.34). An error where the older form can't be read
    /// safely: a path with a newline in it splits its lines.
    fn worktrees(&self, root: &Path) -> Result<Vec<ListedWorktree>, String> {
        match self.run(&["worktree", "list", "--porcelain", "-z"]) {
            Ok(listing) => Ok(parse_worktrees(&listing)),
            Err(GitError::Failed { .. }) => {
                let plain = self
                    .run(&["worktree", "list", "--porcelain"])
                    .map_err(|e| e.to_string())?;
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
                    return Err("Cannot safely read worktree paths with this Git version. \
                                Update Git to 2.36 or newer."
                        .into());
                }
                Ok(parse_worktrees(&plain.replace('\n', "\0")))
            }
            Err(error) => Err(error.to_string()),
        }
    }

    /// git's abbreviation length for the repository, as `%h` would print it (`core.abbrev`,
    /// `auto` by default). git lengthens an abbreviation that would be ambiguous, so this takes
    /// the shortest of a few samples. [`DEFAULT_ABBREV_LEN`] if there are no commits or git
    /// fails.
    fn abbrev_len(&self, commits: &[Oid]) -> usize {
        let samples: Vec<String> = commits.iter().take(3).map(Oid::to_hex).collect();
        if samples.is_empty() {
            return DEFAULT_ABBREV_LEN;
        }
        let mut args = vec!["log", "--no-walk=unsorted", "--no-color", "--format=%h"];
        args.extend(samples.iter().map(String::as_str));
        match self.query(&args) {
            Ok(Some(out)) => out
                .lines()
                .map(|l| l.trim().len())
                .filter(|&n| n > 0)
                .min()
                .unwrap_or(DEFAULT_ABBREV_LEN),
            _ => DEFAULT_ABBREV_LEN,
        }
    }
}

/// What `git log` shows of a commit beyond what [`Git::load`] reads for every commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitDetails {
    /// The full message: subject and body.
    pub message: String,
    /// The author date to the second, in the author's time zone (`2026-09-27 08:14:19 +0200`).
    pub author_date: String,
    pub committer_name: String,
    pub committer_email: String,
    /// The committer date, like [`CommitDetails::author_date`].
    pub committer_date: String,
    /// The commit's notes from the refs `git log` shows notes from, in its order.
    pub notes: Vec<CommitNote>,
}

/// The note of one notes ref on a commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitNote {
    /// `Notes` for the default notes ref, `Notes (review)` for `refs/notes/review`.
    pub heading: String,
    pub text: String,
}

impl Git {
    /// The message, dates, committer and notes of a commit.
    pub fn details(&self, oid: &Oid) -> Result<CommitDetails, GitError> {
        let hex = oid.to_hex();
        let out = self.run(&[
            "log",
            "-1",
            "--no-color",
            "--format=%ai%x00%cn%x00%ce%x00%ci%x00%B",
            &hex,
        ])?;
        let mut fields = out.splitn(5, '\0');
        let mut field = || fields.next().unwrap_or_default().to_owned();
        let (author_date, committer_name, committer_email, committer_date) =
            (field(), field(), field(), field());
        let message = field().trim_end().to_owned();
        // A format of one's own gets the notes without their headings (`%N`); only git's
        // own formats say which notes ref each comes from, as `git log` does.
        let fuller = self.run(&[
            "log",
            "-1",
            "--no-color",
            "--no-expand-tabs",
            "--format=fuller",
            "--notes",
            &hex,
        ])?;
        Ok(CommitDetails {
            message,
            author_date,
            committer_name,
            committer_email,
            committer_date,
            notes: parse_notes(&fuller),
        })
    }

    /// The changed files of a commit: what it changed compared with its first parent (a root
    /// commit, or the boundary of a shallow clone, against the empty tree). Renames are
    /// detected (`-M`); binary files have no line counts. Sorted by
    /// [`compare_paths`](crate::changed_files::compare_paths).
    pub fn changed_files(&self, commit: &Oid) -> Result<Vec<ChangedFile>, GitError> {
        let out = self.run(&[
            "diff-tree",
            "-r",
            "-M",
            "--root",
            "--diff-merges=first-parent",
            "--no-commit-id",
            "--no-ext-diff",
            "--no-textconv",
            "-z",
            "--raw",
            "--numstat",
            &commit.to_hex(),
        ])?;
        parse_diff_tree(&out).map_err(GitError::Parse)
    }

    /// The files that differ between the trees of two commits, `old` against `new`, as
    /// `git diff-tree <old> <new>` lists them (TortoiseGit's "Compare revisions").
    pub fn changed_between(&self, old: &Oid, new: &Oid) -> Result<Vec<ChangedFile>, GitError> {
        let out = self.run(&[
            "diff-tree",
            "-r",
            "-M",
            "--no-ext-diff",
            "--no-textconv",
            "-z",
            "--raw",
            "--numstat",
            &old.to_hex(),
            &new.to_hex(),
        ])?;
        parse_diff_tree(&out).map_err(GitError::Parse)
    }

    /// The common ancestor git picks for two commits (`git merge-base`; hashes or names such
    /// as `HEAD`), or `None` for unrelated histories.
    pub fn merge_base(&self, a: &str, b: &str) -> Result<Option<Oid>, GitError> {
        let out = self.query(&["merge-base", a, b])?;
        Ok(out.and_then(|hex| Oid::from_hex(&hex)))
    }

    /// The files that differ between `commit` and the working tree, staged or not, as
    /// `git diff <commit>` lists them; `reverse` lists the working tree against the commit
    /// instead. Untracked files are not listed. The index's stat information is refreshed in
    /// memory only (`--no-optional-locks`), so nothing is written.
    pub fn changed_in_working_tree(
        &self,
        commit: &Oid,
        reverse: bool,
    ) -> Result<Vec<ChangedFile>, GitError> {
        let hex = commit.to_hex();
        let mut args = vec![
            "--no-optional-locks",
            "diff",
            "-r",
            "-M",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "-z",
            "--raw",
            "--numstat",
        ];
        if reverse {
            args.push("-R");
        }
        args.extend([hex.as_str(), "--"]);
        let out = self.run(&args)?;
        parse_diff_tree(&out).map_err(GitError::Parse)
    }
}

impl Git {
    /// Loads what a file diff compares: both versions as text, through the textconv filter
    /// the file's `diff` attribute names (as `git show` does); or, for a binary file, their
    /// sizes; or, for a submodule, the commits it pointed at.
    pub fn load_file_diff(&self, spec: &FileDiffSpec) -> Result<LoadedDiff, GitError> {
        let versions = [spec.old.as_ref(), spec.new.as_ref()];
        if spec.is_submodule() {
            let commit = |v: Option<&Version>| -> Result<Option<Oid>, GitError> {
                let Some(v) = v else { return Ok(None) };
                let out = match v.rev {
                    Rev::Commit(_) => self.run(&["rev-parse", &object_name(v)])?,
                    // What the submodule has checked out; nothing if it isn't.
                    Rev::WorkingTree => {
                        let dir = self.dir.join(&v.path);
                        let dir = dir.to_string_lossy();
                        let head = self.query(&["-C", &dir, "rev-parse", "-q", "--verify", "HEAD"]);
                        head.ok().flatten().unwrap_or_default()
                    }
                };
                Ok(Oid::from_hex(out.trim()))
            };
            return Ok(LoadedDiff {
                spec: spec.clone(),
                content: Content::Submodule {
                    old: commit(versions[0])?,
                    new: commit(versions[1])?,
                },
                textconv: None,
            });
        }
        if spec.binary {
            let size = |v: Option<&Version>| -> Result<Option<u64>, GitError> {
                let Some(v) = v else { return Ok(None) };
                if v.rev == Rev::WorkingTree {
                    let path = self.dir.join(&v.path);
                    // A symlink's own size, as git sees it: its target needn't exist.
                    let meta = std::fs::symlink_metadata(&path)
                        .map_err(|source| GitError::Read { path, source })?;
                    return Ok(Some(meta.len()));
                }
                let out = self.run(&["cat-file", "-s", &object_name(v)])?;
                out.trim()
                    .parse()
                    .map(Some)
                    .map_err(|_| GitError::Parse(format!("object size {out:?}")))
            };
            return Ok(LoadedDiff {
                spec: spec.clone(),
                content: Content::Binary {
                    old_size: size(versions[0])?,
                    new_size: size(versions[1])?,
                },
                textconv: None,
            });
        }
        let mut invalid_bytes = 0;
        let mut text = |v: Option<&Version>| -> Result<String, GitError> {
            let Some(v) = v else {
                return Ok(String::new());
            };
            let bytes = match v.rev {
                Rev::Commit(_) => self.run_bytes(&["cat-file", "--textconv", &object_name(v)])?,
                Rev::WorkingTree => self.working_tree_file(&v.path)?,
            };
            let (text, bad) = decode(&bytes);
            invalid_bytes += bad;
            Ok(text)
        };
        let old = text(versions[0])?;
        let new = text(versions[1])?;
        Ok(LoadedDiff {
            spec: spec.clone(),
            content: Content::Text {
                old,
                new,
                invalid_bytes,
            },
            textconv: self.textconv(spec.path())?,
        })
    }

    /// The textconv filter git applies to `path` when diffing, as `driver: command`: the
    /// `diff` attribute (from the working tree, git's default) names a driver that has a
    /// `diff.<driver>.textconv` command.
    fn textconv(&self, path: &str) -> Result<Option<String>, GitError> {
        let out = self.run(&["check-attr", "-z", "diff", "--", path])?;
        // `<path> NUL diff NUL <value> NUL`
        let value = out.split('\0').nth(2).unwrap_or("");
        if matches!(value, "" | "unspecified" | "set" | "unset") {
            return Ok(None);
        }
        let command = self.query(&["config", &format!("diff.{value}.textconv")])?;
        Ok(command
            .filter(|c| !c.is_empty())
            .map(|c| format!("{value}: {c}")))
    }
}

impl Git {
    /// Blames a file: which commit last changed each of its lines (`git blame
    /// --line-porcelain`), through the textconv filter of its `diff` attribute, as a diff
    /// reads it. A root commit is the origin of its lines rather than a boundary; in the
    /// working tree, lines no commit has yet belong to no commit.
    pub fn blame(&self, spec: &BlameSpec, options: BlameOptions) -> Result<Blame, GitError> {
        let mut args = vec!["blame", "--line-porcelain", "--root"];
        if options.ignore_whitespace {
            args.push("-w");
        }
        args.extend(options.moves.args());
        let rev = spec.rev.commit().map(|o| o.to_hex());
        if let Some(rev) = &rev {
            args.push(rev);
        }
        args.extend(["--", &spec.path]);
        let out = self.run_bytes(&args)?;
        Blame::parse(&out).map_err(GitError::Parse)
    }
}

impl Git {
    /// The commits that changed the file a blame is of, up to the blamed revision, for the
    /// history pane: `git log --no-follow --topo-order --parents <rev> -- <path>`, in git's
    /// default history simplification, so a merge is listed only if it changed the file
    /// compared with every parent. Combine it with the blame in
    /// [`FileHistory::new`](crate::file_history::FileHistory::new).
    ///
    /// For the working tree, the log starts at `HEAD` (and `MERGE_HEAD` during a merge) and
    /// runs on the file's path in `HEAD`, which differs after a staged rename; it also says
    /// whether the file differs from `HEAD` at all (`git diff --quiet`), since an edit that
    /// only deletes lines leaves no lines of its own in the blame.
    ///
    /// `cancel` kills git from another thread (the walk can take seconds on a long history).
    pub fn file_log(&self, spec: &BlameSpec, cancel: &Cancel) -> Result<FileLog, GitError> {
        let (revs, path, working_tree_changed) = match spec.rev {
            Rev::Commit(oid) => (vec![oid.to_hex()], spec.path.clone(), false),
            Rev::WorkingTree => {
                let mut revs = vec!["HEAD".to_owned()];
                revs.extend(self.query(&["rev-parse", "-q", "--verify", "MERGE_HEAD^{commit}"])?);
                let changed = self.differs_from_head(&spec.path)?;
                check(cancel)?;
                (revs, self.path_in_head(&spec.path)?, changed)
            }
        };
        check(cancel)?;
        let mut args = vec![
            "--literal-pathspecs",
            "log",
            "--no-follow",
            "--topo-order",
            "--parents",
            "--no-color",
            "--no-decorate",
            file_history::DATE_FORMAT,
            "-z",
            file_history::LOG_FORMAT,
        ];
        args.extend(revs.iter().map(String::as_str));
        args.extend(["--", &path]);
        let out = self.run_cancellable(&args, cancel)?;
        Ok(FileLog {
            commits: file_history::parse_log(&out).map_err(GitError::Parse)?,
            path,
            working_tree_changed,
        })
    }

    /// True if the file in the working tree, staged or not, differs from `HEAD`.
    fn differs_from_head(&self, path: &str) -> Result<bool, GitError> {
        let args = [
            "--no-optional-locks",
            "--literal-pathspecs",
            "diff",
            "--quiet",
            "--no-ext-diff",
            "HEAD",
            "--",
            path,
        ];
        let out = self.output(&args)?;
        match out.status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(GitError::Failed {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            }),
        }
    }

    /// The path in `HEAD` of the working tree's file at `path`, as blame finds it: the same
    /// path if `HEAD` has it, else the file a staged rename came from (`HEAD` against the
    /// index, unlimited, since a diff limited to the path can't see the rename). Otherwise the
    /// file is new and the path stays.
    fn path_in_head(&self, path: &str) -> Result<String, GitError> {
        if self
            .query(&["rev-parse", "-q", "--verify", &format!("HEAD:{path}")])?
            .is_some()
        {
            return Ok(path.to_owned());
        }
        let out = self.run(&[
            "diff-index",
            "--cached",
            "-M",
            "-z",
            "--name-status",
            "HEAD",
        ])?;
        // `<status> NUL <path> NUL`, with the old path first for a rename.
        let mut fields = out.split('\0');
        while let Some(status) = fields.next() {
            if status.starts_with('R') {
                let (old, new) = (fields.next(), fields.next());
                if new == Some(path)
                    && let Some(old) = old
                {
                    return Ok(old.to_owned());
                }
            } else {
                fields.next();
            }
        }
        Ok(path.to_owned())
    }
}

impl Git {
    /// A file of the working tree as `git diff` reads it: through its clean filter and
    /// line-ending conversion, then the textconv filter of its `diff` attribute. git has no
    /// command that prints that, so this diffs the file against the empty tree, which lists
    /// every line as added, and takes the lines back out of the patch.
    fn working_tree_file(&self, path: &str) -> Result<Vec<u8>, GitError> {
        let empty_tree =
            self.run_with_input(&["hash-object", "-t", "tree", "--stdin"], String::new())?;
        let patch = self.run_bytes(&[
            "--no-optional-locks",
            "--literal-pathspecs",
            "diff",
            "--no-ext-diff",
            "--textconv",
            "--no-color",
            "--no-renames",
            "-U0",
            empty_tree.trim(),
            "--",
            path,
        ])?;
        added_lines(&patch).map_err(GitError::Parse)
    }
}

/// The text of a patch that adds one whole file: its `+` lines, with the last newline taken
/// off after `\ No newline at end of file`. Anything but added lines is an error.
fn added_lines(patch: &[u8]) -> Result<Vec<u8>, String> {
    let mut text = Vec::with_capacity(patch.len());
    let mut in_hunk = false;
    let mut lines = patch.split(|&b| b == b'\n').peekable();
    while let Some(line) = lines.next() {
        // The split leaves an empty piece after the final newline.
        if line.is_empty() && lines.peek().is_none() {
            break;
        }
        if !in_hunk {
            in_hunk = line.starts_with(b"@@ ");
            if !in_hunk && line.starts_with(b"Binary files ") {
                return Err("git reads the working tree file as binary".into());
            }
            continue;
        }
        match line.first() {
            Some(b'+') => {
                text.extend_from_slice(&line[1..]);
                text.push(b'\n');
            }
            Some(b'\\') => {
                text.pop();
            }
            _ => {
                let line = String::from_utf8_lossy(line);
                return Err(format!(
                    "unexpected line in the working tree patch: {line:?}"
                ));
            }
        }
    }
    Ok(text)
}

/// `<rev>:<path>`, the name git reads a committed version by.
/// The notes in `git log --format=fuller --notes` output of one commit. The headers end at the
/// first empty line; after them, the message and every note's text are indented by four
/// spaces, and each note starts with a heading at the start of a line.
fn parse_notes(fuller: &str) -> Vec<CommitNote> {
    let mut notes: Vec<CommitNote> = Vec::new();
    for line in fuller.lines().skip_while(|l| !l.is_empty()) {
        if let Some(heading) = line.strip_suffix(':').filter(|h| h.starts_with("Notes")) {
            notes.push(CommitNote {
                heading: heading.to_owned(),
                text: String::new(),
            });
        } else if let Some(note) = notes.last_mut()
            && let Some(text) = line.strip_prefix("    ")
        {
            note.text.push_str(text);
            note.text.push('\n');
        }
    }
    for note in &mut notes {
        note.text.truncate(note.text.trim_end().len());
    }
    notes
}

fn object_name(v: &Version) -> String {
    let rev = v.rev.commit().map(|o| o.to_hex()).unwrap_or_default();
    format!("{rev}:{}", v.path)
}

/// Convenience wrapper: load the repository containing `dir`. Fails with
/// [`GitError::TooOld`] first if git is older than [`MINIMUM_VERSION`].
pub fn load_repo(dir: &Path) -> Result<Repo, GitError> {
    version::check()?;
    Git::new(dir).load()
}

fn parse_log(log: &str) -> Result<(Vec<Commit>, HashMap<Oid, CommitIx>), GitError> {
    struct Raw<'a> {
        oid: Oid,
        parents: &'a str,
        empty_tree: bool,
        author_name: &'a str,
        author_email: &'a str,
        author_time: i64,
        author_date: &'a str,
        commit_time: i64,
        subject: &'a str,
    }

    let mut tokens: Vec<&str> = log.split('\0').collect();
    // `-z` terminates the last record with a NUL too.
    if tokens.last() == Some(&"") && tokens.len() % LOG_FIELDS == 1 {
        tokens.pop();
    }
    if !tokens.len().is_multiple_of(LOG_FIELDS) {
        return Err(GitError::Parse(format!(
            "log output has {} fields, not a multiple of {LOG_FIELDS}",
            tokens.len()
        )));
    }
    let mut raw = Vec::with_capacity(tokens.len() / LOG_FIELDS);
    let (records, _) = tokens.as_chunks::<LOG_FIELDS>();
    for [hash, parents, tree, an, ae, at, ad, ct, subject] in records {
        let hash = hash.trim_start_matches('\n');
        raw.push(Raw {
            oid: Oid::from_hex(hash)
                .ok_or_else(|| GitError::Parse(format!("bad hash {hash:?}")))?,
            parents,
            empty_tree: *tree == EMPTY_TREE_SHA1 || *tree == EMPTY_TREE_SHA256,
            author_name: an,
            author_email: ae,
            author_time: at.parse().unwrap_or(0),
            author_date: ad,
            commit_time: ct.parse().unwrap_or(0),
            subject,
        });
    }

    let by_oid: HashMap<Oid, CommitIx> = raw
        .iter()
        .enumerate()
        .map(|(i, r)| (r.oid, CommitIx(i as u32)))
        .collect();

    let commits = raw
        .into_iter()
        .map(|r| {
            let mut truncated = false;
            let parents = r
                .parents
                .split_ascii_whitespace()
                .filter_map(|p| {
                    let ix = Oid::from_hex(p).and_then(|o| by_oid.get(&o).copied());
                    truncated |= ix.is_none();
                    ix
                })
                .collect();
            Commit {
                oid: r.oid,
                parents,
                truncated,
                empty_tree: r.empty_tree,
                author_name: r.author_name.to_owned(),
                author_email: r.author_email.to_owned(),
                author_time: r.author_time,
                author_date: r.author_date.to_owned(),
                commit_time: r.commit_time,
                subject: r.subject.to_owned(),
            }
        })
        .collect();
    Ok((commits, by_oid))
}

/// What git lists of a repository's refs and worktrees, and its HEAD: read once, by
/// [`Git::load`], and kept in the [`Repo`] for the branch catalogue to be built from, so the
/// catalogue agrees with the graph (#310).
#[derive(Clone, Debug, Default)]
pub(crate) struct Listing {
    pub location: Location,
    /// The refs that point at commits, tags peeled; notes and symbolic refs left out.
    pub refs: Vec<ListedRef>,
    /// The symbolic refs and the refs they point at: `refs/remotes/origin/HEAD` at
    /// `refs/remotes/origin/main`.
    pub symrefs: Vec<(String, String)>,
    /// Every worktree, the main one first, a bare main repository's too.
    pub worktrees: Vec<ListedWorktree>,
    /// Why the worktrees couldn't be read, if they couldn't: there are none then.
    pub worktree_error: Option<String>,
    /// The branch HEAD is on (`refs/heads/main`), if any.
    pub head_branch: Option<String>,
    /// HEAD's commit: none for an unborn branch.
    pub head: Option<Oid>,
}

/// A ref in a [`Listing`].
#[derive(Clone, Debug)]
pub(crate) struct ListedRef {
    pub full_name: String,
    /// The commit it points at, tags peeled.
    pub commit: Oid,
    pub annotated: bool,
    /// A local branch's upstream (`refs/remotes/origin/topic`), whether it exists or not.
    pub upstream: Option<String>,
    /// That upstream's remote, and its name there: `origin` and `refs/heads/topic`.
    pub upstream_remote: Option<(String, String)>,
}

/// A ref as listed by `for-each-ref`, before its commit is looked up.
#[derive(Debug)]
struct RawRef {
    full_name: String,
    /// The commit it points at (tags peeled), or `None` for a tag of a tag, which still needs
    /// peeling.
    commit: Option<Oid>,
    annotated: bool,
    upstream: Option<String>,
    upstream_remote: Option<(String, String)>,
}

/// Parses `for-each-ref` output, and the symbolic refs with what they point at. Symbolic refs
/// (duplicates), notes, and refs to trees or blobs are left out of the refs.
fn parse_refs(out: &str) -> (Vec<RawRef>, Vec<(String, String)>) {
    let mut refs = Vec::new();
    let mut symrefs = Vec::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split(FIELD).collect();
        let [
            full_name,
            obj_type,
            obj,
            peeled_type,
            peeled,
            symref,
            upstream,
            remote,
            remote_ref,
        ] = f[..]
        else {
            continue;
        };
        // e.g. refs/remotes/origin/HEAD -> origin/main: a duplicate label.
        if !symref.is_empty() {
            symrefs.push((full_name.to_owned(), symref.to_owned()));
            continue;
        }
        if full_name.starts_with("refs/notes/") {
            continue;
        }
        let (annotated, commit) = match (obj_type, peeled_type) {
            ("commit", _) => (false, Oid::from_hex(obj)),
            ("tag", "commit") => (true, Oid::from_hex(peeled)),
            ("tag", "tag") => (true, None),
            // Trees and blobs: nothing to draw.
            _ => continue,
        };
        refs.push(RawRef {
            full_name: full_name.to_owned(),
            commit,
            annotated,
            upstream: (!upstream.is_empty()).then(|| upstream.to_owned()),
            upstream_remote: (!remote.is_empty() && !remote_ref.is_empty())
                .then(|| (remote.to_owned(), remote_ref.to_owned())),
        });
    }
    (refs, symrefs)
}

/// A worktree in a [`Listing`], as `git worktree list --porcelain` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ListedWorktree {
    pub path: String,
    /// `None` for an unborn branch (git lists the null id) and for a bare repository.
    pub head: Option<Oid>,
    /// `refs/heads/main`; `None` when detached.
    pub branch: Option<String>,
    pub bare: bool,
    /// The lock's reason (empty when none was given), when it's locked.
    pub locked: Option<String>,
    pub prunable: bool,
}

/// Parses `git worktree list --porcelain -z`: a NUL after each attribute, and another after
/// each worktree's. `locked` and `prunable` are listed from git 2.31 on.
fn parse_worktrees(out: &str) -> Vec<ListedWorktree> {
    let mut worktrees: Vec<ListedWorktree> = Vec::new();
    for field in out.split('\0') {
        let (label, value) = field.split_once(' ').unwrap_or((field, ""));
        if label == "worktree" {
            worktrees.push(ListedWorktree {
                path: value.to_owned(),
                head: None,
                branch: None,
                bare: false,
                locked: None,
                prunable: false,
            });
            continue;
        }
        let Some(w) = worktrees.last_mut() else {
            continue;
        };
        match label {
            "HEAD" => w.head = Oid::from_hex(value).filter(|_| value.bytes().any(|b| b != b'0')),
            "branch" => w.branch = Some(value.to_owned()),
            "bare" => w.bare = true,
            "locked" => w.locked = Some(value.to_owned()),
            "prunable" => w.prunable = true,
            _ => {}
        }
    }
    worktrees
}

/// Classifies a full ref name and produces its display name.
pub fn classify_ref(full_name: &str) -> (RefKind, String) {
    let strip = |prefix: &str| full_name.strip_prefix(prefix).map(str::to_owned);
    if let Some(n) = strip("refs/heads/") {
        (RefKind::LocalBranch, n)
    } else if let Some(n) = strip("refs/remotes/") {
        (RefKind::RemoteBranch, n)
    } else if let Some(n) = strip("refs/tags/") {
        (RefKind::Tag, n)
    } else if full_name == "refs/stash" {
        (RefKind::Stash, "stash".to_owned())
    } else {
        (
            RefKind::Other,
            full_name
                .strip_prefix("refs/")
                .unwrap_or(full_name)
                .to_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_read_as_config_get_would_give_them() {
        let config = parse_config(
            b"core.bare\0user.name\nTest\0merge.ff\nfalse\0merge.ff\nonly\0\
              branch.autosetuprebase\nalways\0remote.origin.url\nhttps://x/y\0\
              remote.a.b.fetch\n+refs/heads/*:refs/remotes/a.b/*\0remote.pushdefault\norigin\0\
              merge.log\nyes\0merge.autostash\n\0rebase.autostash\nmaybe\0\
              diff.Sub Section.textconv\ncat\0",
        );
        // The last value wins; section and name match regardless of case, a subsection as is.
        assert_eq!(config.get("merge.ff"), Some("only"));
        assert_eq!(config.get("Branch.AutoSetupRebase"), Some("always"));
        assert_eq!(config.get("diff.Sub Section.textconv"), Some("cat"));
        assert_eq!(config.get("diff.sub section.textconv"), None);
        assert_eq!(config.get("user.email"), None);
        // A key set without a value is empty, as `--get` prints it, and true as a boolean.
        assert_eq!(config.get("core.bare"), Some(""));
        assert_eq!(config.bool("core.bare"), Some(true));
        assert_eq!(config.bool("merge.log"), Some(true));
        assert_eq!(config.bool("merge.autoStash"), Some(false));
        assert_eq!(config.bool("rebase.autoStash"), None);
        assert_eq!(config.bool("commit.gpgsign"), None);
        // Every `remote.<name>.<setting>` names a remote; `remote.pushDefault` doesn't.
        assert_eq!(config.remotes(), ["a.b", "origin"]);
        assert_eq!(parse_config(b""), Config::default());
    }

    #[test]
    fn a_batch_of_objects_is_read_by_size() {
        let out = b"0000000000000000000000000000000000000001 blob 3\nab\n\n\
                    0000000000000000000000000000000000000002 blob 0\n\n\
                    0000000000000000000000000000000000000003 blob 2\n\0\xff\n";
        assert_eq!(
            parse_batch(out, 3),
            Some(vec![b"ab\n".to_vec(), Vec::new(), b"\0\xff".to_vec()])
        );
        assert_eq!(parse_batch(out, 4), None);
        assert_eq!(
            parse_batch(b"0000000000000000000000000000000000000001 missing\n", 1),
            None
        );
        assert_eq!(parse_batch(b"garbage", 1), None);
    }

    #[test]
    fn a_location_is_read_from_what_rev_parse_printed_before_it_stopped() {
        let at = |git_dir: &str, common_dir: &str, work_tree: Option<&str>| Location {
            git_dir: git_dir.into(),
            common_dir: common_dir.into(),
            work_tree: work_tree.map(PathBuf::from),
        };
        let work = "true\n/r/w/.git\n/r/w/.git\n/r/w\n";
        assert_eq!(
            parse_location(work, true),
            Some(at("/r/w/.git", "/r/w/.git", Some("/r/w")))
        );
        assert_eq!(
            parse_location(work, true).unwrap().root(),
            Path::new("/r/w")
        );
        // A linked worktree keeps its refs in the main one's git dir.
        let linked = "true\n/r/w/.git/worktrees/x\n/r/w/.git\n/r/x\n";
        assert_eq!(
            parse_location(linked, true),
            Some(at("/r/w/.git/worktrees/x", "/r/w/.git", Some("/r/x")))
        );
        // Without a work tree `--show-toplevel` fails, after the rest has printed.
        let bare = "false\n/r/b\n/r/b\n";
        assert_eq!(parse_location(bare, false), Some(at("/r/b", "/r/b", None)));
        assert_eq!(
            parse_location(bare, false).unwrap().root(),
            Path::new("/r/b")
        );
        // Not a repository: nothing printed. Or cut short some other way.
        assert_eq!(parse_location("", false), None);
        assert_eq!(parse_location("true\n/r/w/.git\n/r/w/.git\n", false), None);
        assert_eq!(parse_location("true\n/r/w/.git\n", true), None);
    }

    #[test]
    fn a_folder_git_does_not_open_says_why_in_gits_terms() {
        let refusal = |found: &str| {
            format!(
                "fatal: cannot use bare repository '{found}' (safe.bareRepository is 'explicit')\n"
            )
        };
        let dir = Path::new("/r/bare.git/refs");
        assert_eq!(
            not_opened(dir, &refusal("/r/bare.git")).to_string(),
            "git won't use discovered bare repository /r/bare.git \
             (safe.bareRepository is 'explicit'); open /r/bare.git"
        );
        // Before 2.45 git refuses a work tree's `.git` too; the work tree opens.
        let dot_git = Path::new("/r/work/.git");
        assert_eq!(
            not_opened(dot_git, &refusal("/r/work/.git")).to_string(),
            "git won't use discovered bare repository /r/work/.git \
             (safe.bareRepository is 'explicit'); open /r/work"
        );
        for missing in [
            "fatal: not a git repository (or any of the parent directories): .git",
            "fatal: cannot change to '/r/bare.git/refs': No such file or directory",
        ] {
            assert!(matches!(
                not_opened(dir, missing),
                GitError::NotARepository(d) if d == dir
            ));
        }
        // Anything else in git's words, such as safe.directory's.
        let dubious = "fatal: detected dubious ownership in repository at '/r'\n\
                       To add an exception for this directory, call:";
        assert!(
            not_opened(dir, dubious)
                .to_string()
                .contains("detected dubious ownership in repository at '/r'")
        );
    }

    #[test]
    fn only_bare_repositories_git_would_not_discover_are_named_explicitly() {
        let tmp = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = git_command()
                .current_dir(tmp.path())
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q", "work"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "-C",
            "work",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ]);
        git(&["-C", "work", "worktree", "add", "-q", "../linked"]);
        git(&["init", "-q", "--bare", "bare.git"]);
        git(&["init", "-q", "--bare", "plain"]);
        git(&["init", "-q", "--bare", "work/.git/modules/sub"]);
        let dir = |p: &str| tmp.path().join(p);
        assert!(is_implicit_bare(&dir("bare.git")));
        assert!(is_implicit_bare(&dir("plain")));
        // A work tree, and the git dirs git lets through itself, where `--git-dir` would make
        // the folder its own work tree.
        assert!(!is_implicit_bare(&dir("work")));
        assert!(!is_implicit_bare(&dir("work/.git")));
        assert!(!is_implicit_bare(&dir("work/.git/hooks/..")));
        assert!(!is_implicit_bare(&dir("work/.git/worktrees/linked")));
        assert!(!is_implicit_bare(&dir("work/.git/modules/sub")));
        assert!(!is_implicit_bare(&dir("linked")));
        assert!(!is_implicit_bare(&dir("bare.git/refs")));
    }

    #[test]
    fn parses_worktree_listings() {
        let oid = "1".repeat(40);
        let null = "0".repeat(40);
        let out = [
            "worktree C:/src/main",
            &format!("HEAD {oid}"),
            "branch refs/heads/main",
            "",
            "worktree C:/src/wt space/ö",
            &format!("HEAD {oid}"),
            "detached",
            "locked on usb",
            "",
            "worktree /tmp/orphan",
            &format!("HEAD {null}"),
            "branch refs/heads/orph",
            "prunable gitdir file points to non-existent location",
            "",
            "worktree /srv/bare.git",
            "bare",
            "",
        ]
        .join("\0");
        let w = parse_worktrees(&out);
        assert_eq!(w.len(), 4);
        assert_eq!(w[0].path, "C:/src/main");
        assert_eq!(w[0].head, Oid::from_hex(&oid));
        assert_eq!(w[0].branch.as_deref(), Some("refs/heads/main"));
        assert_eq!(
            (w[1].path.as_str(), w[1].branch.as_deref()),
            ("C:/src/wt space/ö", None)
        );
        assert_eq!(w[1].locked.as_deref(), Some("on usb"));
        assert!(!w[1].prunable);
        assert_eq!(w[2].head, None, "an unborn branch has no head");
        assert!(w[2].prunable && w[2].locked.is_none());
        assert!(w[3].bare && w[3].head.is_none());
    }

    #[test]
    fn classifies_refs() {
        assert_eq!(
            classify_ref("refs/heads/feature/x"),
            (RefKind::LocalBranch, "feature/x".into())
        );
        assert_eq!(
            classify_ref("refs/remotes/origin/main"),
            (RefKind::RemoteBranch, "origin/main".into())
        );
        assert_eq!(
            classify_ref("refs/tags/v1.0"),
            (RefKind::Tag, "v1.0".into())
        );
        assert_eq!(classify_ref("refs/stash"), (RefKind::Stash, "stash".into()));
        assert_eq!(
            classify_ref("refs/pull/1/head"),
            (RefKind::Other, "pull/1/head".into())
        );
    }

    #[test]
    fn parses_log_records_and_links_parents() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let missing = "c".repeat(40);
        let tree = "d".repeat(40);
        let log = format!(
            "{b}\0{a} {missing}\0{tree}\0A\x1fnn\0ann@x\020\02024-01-02 03:04\021\0second\x1fwith\x1eseps\0\
             {a}\0\0{EMPTY_TREE_SHA1}\0Bob\0bob@x\010\02024-01-01 00:00\011\0first\0"
        );
        let (commits, by_oid) = parse_log(&log).unwrap();
        assert_eq!(commits.len(), 2);
        let second = &commits[by_oid[&Oid::from_hex(&b).unwrap()].ix()];
        assert_eq!(second.parents, vec![by_oid[&Oid::from_hex(&a).unwrap()]]);
        assert!(
            second.truncated,
            "missing parent marks the commit truncated"
        );
        assert_eq!(second.subject, "second\x1fwith\x1eseps");
        assert_eq!(second.author_name, "A\x1fnn");
        assert_eq!(second.commit_time, 21);
        let first = &commits[1];
        assert!(first.parents.is_empty() && !first.truncated);
        assert!(first.empty_tree && !second.empty_tree);
    }
}
