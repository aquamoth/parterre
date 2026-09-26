//! Loading a [`Repo`] snapshot by running the `git` command-line tool.
//!
//! We shell out to `git` rather than linking a git library: it is always present where
//! parterre is useful, honours every repository configuration (worktrees, alternates,
//! packed refs, sha256, ...), and `git log` streams tens of thousands of commits in
//! milliseconds.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::changed_files::{ChangedFile, parse_diff_tree};
use crate::file_diff::{Content, FileDiffSpec, LoadedDiff, Version, decode};
use crate::oid::Oid;
use crate::repo::{Commit, CommitIx, DEFAULT_ABBREV_LEN, GitRef, Head, RefKind, Repo};

mod program;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("could not run git ({0}); is git installed and on PATH?")]
    Spawn(#[source] std::io::Error),
    #[error("`git {args}` failed: {stderr}")]
    Failed { args: String, stderr: String },
    #[error("{0} is not inside a git repository")]
    NotARepository(PathBuf),
    #[error("unexpected output from git: {0}")]
    Parse(String),
}

/// A handle for running git commands against one repository.
#[derive(Clone, Debug)]
pub struct Git {
    dir: PathBuf,
}

/// Separator for `for-each-ref` fields (ref names cannot contain control characters).
const FIELD: char = '\x1f';
/// `git log -z` format: NUL-separated fields, so subjects and names may contain anything.
const LOG_FORMAT: &str = "--format=%H%x00%P%x00%T%x00%an%x00%ae%x00%at%x00%ad%x00%ct%x00%s";
const LOG_FIELDS: usize = 9;
const EMPTY_TREE_SHA1: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const EMPTY_TREE_SHA256: &str = "6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321";

impl Git {
    pub fn new(dir: impl Into<PathBuf>) -> Git {
        Git { dir: dir.into() }
    }

    fn command<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new(program::git());
        cmd.arg("-C")
            .arg(&self.dir)
            .args(["-c", "core.quotepath=off"])
            .args(["-c", "log.showSignature=false"])
            .args(["-c", "i18n.logOutputEncoding=UTF-8"])
            .args(["-c", "color.ui=false"])
            .args(args)
            // Read-only tool: never take the index lock for opportunistic refreshes.
            .env("GIT_OPTIONAL_LOCKS", "0")
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

    fn output(&self, args: &[&str]) -> Result<Output, GitError> {
        self.command(args).output().map_err(GitError::Spawn)
    }

    /// Runs git and returns stdout, failing on a non-zero exit status.
    pub(crate) fn run(&self, args: &[&str]) -> Result<String, GitError> {
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
    fn run_bytes(&self, args: &[&str]) -> Result<Vec<u8>, GitError> {
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
    fn run_with_input(&self, args: &[&str], input: String) -> Result<String, GitError> {
        use std::io::Write as _;
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .spawn()
            .map_err(GitError::Spawn)?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        // Write from another thread so a large output cannot deadlock against a full pipe.
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
        let out = child.wait_with_output().map_err(GitError::Spawn)?;
        let _ = writer.join();
        if !out.status.success() {
            return Err(GitError::Failed {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Runs git and returns trimmed stdout, or `None` on a non-zero exit status (for queries
    /// such as `symbolic-ref -q` that signal "no" through the exit code).
    pub(crate) fn query(&self, args: &[&str]) -> Result<Option<String>, GitError> {
        let out = self.output(args)?;
        Ok(out
            .status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned()))
    }

    /// Resolves the repository root: the working tree, or the git dir for a bare repository.
    pub fn repo_root(&self) -> Result<PathBuf, GitError> {
        let Some(out) = self.query(&["rev-parse", "--is-bare-repository", "--absolute-git-dir"])?
        else {
            return Err(GitError::NotARepository(self.dir.clone()));
        };
        let mut lines = out.lines();
        let bare = lines.next() == Some("true");
        let git_dir = lines
            .next()
            .ok_or_else(|| GitError::Parse("rev-parse printed no git dir".into()))?;
        if bare {
            return Ok(PathBuf::from(git_dir));
        }
        // Inside a `.git` directory there is no work tree: use the git dir itself.
        Ok(match self.query(&["rev-parse", "--show-toplevel"])? {
            Some(top) if !top.is_empty() => PathBuf::from(top),
            _ => PathBuf::from(git_dir),
        })
    }

    /// The repository's git dir and common dir: the same directory, except in a linked
    /// worktree, whose own git dir holds its HEAD while the refs are shared.
    pub fn git_dirs(&self) -> Result<(PathBuf, PathBuf), GitError> {
        let Some(out) = self.query(&["rev-parse", "--absolute-git-dir", "--git-common-dir"])?
        else {
            return Err(GitError::NotARepository(self.dir.clone()));
        };
        let mut lines = out.lines();
        let (Some(git_dir), Some(common)) = (lines.next(), lines.next()) else {
            return Err(GitError::Parse("rev-parse printed no git dir".into()));
        };
        // The common dir is printed relative to the directory git ran in.
        Ok((PathBuf::from(git_dir), self.dir.join(common)))
    }

    /// Loads all refs (notes excluded) and every commit reachable from them.
    ///
    /// Refs and HEAD are read first and the log is then walked from exactly those commits, so
    /// a concurrent fetch cannot leave refs pointing at commits that were not loaded.
    pub fn load(&self) -> Result<Repo, GitError> {
        let root = self.repo_root()?;
        let git = Git::new(&root);

        let ref_format = format!(
            "--format=%(refname){FIELD}%(objecttype){FIELD}%(objectname){FIELD}%(*objecttype){FIELD}%(*objectname){FIELD}%(symref)"
        );
        let listing = git.run(&["for-each-ref", &ref_format])?;
        let mut raw_refs = parse_refs(&listing);
        // Tags of tags: let git peel them all the way to a commit.
        let nested: Vec<usize> = (0..raw_refs.len())
            .filter(|&i| raw_refs[i].commit.is_none())
            .collect();
        if !nested.is_empty() {
            let input: String = nested
                .iter()
                .map(|&i| format!("{}^{{commit}}\n", raw_refs[i].full_name))
                .collect();
            let out = git.run_with_input(&["cat-file", "--batch-check"], input)?;
            for (&i, line) in nested.iter().zip(out.lines()) {
                if let Some((oid, "commit")) = line
                    .split_once(' ')
                    .map(|(o, rest)| (o, rest.split(' ').next().unwrap_or("")))
                {
                    raw_refs[i].commit = Oid::from_hex(oid);
                }
            }
        }
        raw_refs.retain(|r| r.commit.is_some());

        let head_branch = git.query(&["symbolic-ref", "-q", "HEAD"])?;
        let head_oid = git
            .query(&["rev-parse", "-q", "--verify", "HEAD^{commit}"])?
            .and_then(|s| Oid::from_hex(&s));

        let mut starts: Vec<Oid> = raw_refs.iter().filter_map(|r| r.commit).collect();
        starts.extend(head_oid);
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
        let head = match (&head_branch, head_oid) {
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

        let mut refs: Vec<GitRef> = raw_refs
            .into_iter()
            .filter_map(|r| {
                let target = lookup(&r.commit?)?;
                let (kind, name) = classify_ref(&r.full_name);
                Some(GitRef {
                    is_head: Some(r.full_name.as_str()) == head_branch.as_deref(),
                    full_name: r.full_name,
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
        let mut repo = Repo::new(root, commits, refs, head);
        repo.abbrev_len = abbrev_len;
        Ok(repo)
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

impl Git {
    /// The full commit message (subject and body) of a commit.
    pub fn message(&self, oid: &Oid) -> Result<String, GitError> {
        let out = self.run(&["log", "-1", "--no-color", "--format=%B", &oid.to_hex()])?;
        Ok(out.trim_end().to_owned())
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
                let out = self.run(&["rev-parse", &object_name(v)])?;
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
            let bytes = self.run_bytes(&["cat-file", "--textconv", &object_name(v)])?;
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

/// `<rev>:<path>`, the name git reads a version by.
fn object_name(v: &Version) -> String {
    format!("{}:{}", v.rev.to_hex(), v.path)
}

/// Convenience wrapper: load the repository containing `dir`.
pub fn load_repo(dir: &Path) -> Result<Repo, GitError> {
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

/// A ref as listed by `for-each-ref`, before its commit is looked up.
#[derive(Debug)]
struct RawRef {
    full_name: String,
    /// The commit it points at (tags peeled), or `None` for a tag of a tag, which still needs
    /// peeling.
    commit: Option<Oid>,
    annotated: bool,
}

/// Parses `for-each-ref` output. Symbolic refs (duplicates), notes, and refs to trees or
/// blobs are skipped.
fn parse_refs(out: &str) -> Vec<RawRef> {
    let mut refs = Vec::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split(FIELD).collect();
        let [full_name, obj_type, obj, peeled_type, peeled, symref] = f[..] else {
            continue;
        };
        // e.g. refs/remotes/origin/HEAD -> origin/main: a duplicate label.
        if !symref.is_empty() || full_name.starts_with("refs/notes/") {
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
        });
    }
    refs
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
