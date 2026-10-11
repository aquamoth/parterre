//! Throwaway git repositories with scripted histories, made with the git parterre itself runs
//! (`parterre_core::git::program()`: on Windows the real `git.exe`, not the `cmd\git.exe`
//! launcher), and in as few git calls as will do: starting git is what the suite's time goes
//! on, on Windows above all (#309).

#![allow(dead_code)]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use parterre_core::blame::BlameSpec;
use parterre_core::branches::{Action, Branches, Outcome, Report, Warning};
use parterre_core::file_diff::Rev;
use parterre_core::{CommitIx, Oid, Repo};
use tempfile::TempDir;

/// Unix seconds that [`git_at`]'s minutes count from.
const BASE_DATE: u32 = 1_700_000_000;

/// A file's text, with the CRLF that `core.autocrlf` gives it on checkout undone: Git for
/// Windows sets it in its system config, which parterre's git reads and [`git_command`]'s
/// doesn't (#182). Compare files parterre checked out with this, never their bytes.
pub fn read_text(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .replace("\r\n", "\n")
}

/// git in `dir`, without the user's or the system's config.
pub fn git_command(dir: &Path) -> Command {
    let mut cmd = Command::new(parterre_core::git::program());
    cmd.current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    cmd
}

/// Runs git in `dir`; returns its trimmed output.
pub fn git_in(dir: &Path, args: &[&str]) -> String {
    succeeded(args, git_command(dir).args(args).output().expect("run git"))
}

/// Runs git in `dir`, committing at `minutes` past the base date.
pub fn git_at(dir: &Path, minutes: u32, args: &[&str]) -> String {
    let date = format!("{} +0000", BASE_DATE + minutes * 60);
    let out = git_command(dir)
        .args(args)
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .output()
        .expect("run git");
    succeeded(args, out)
}

/// Runs git in `dir` where it must fail, as a merge that stops on conflicts does. An editor
/// it opens instead exits at once.
pub fn git_fails(dir: &Path, args: &[&str]) {
    let out = git_command(dir)
        .args(args)
        .env("GIT_EDITOR", "true")
        .output()
        .expect("run git");
    assert!(!out.status.success(), "git {args:?} succeeded");
}

/// Runs git in `dir` as parterre's own git does, with the system config: Git for Windows sets
/// `core.autocrlf` there (#182). Returns its output untrimmed.
pub fn git_with_system_config(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(parterre_core::git::program())
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("run git");
    stdout(args, out)
}

fn stdout(args: &[&str], out: Output) -> String {
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn succeeded(args: &[&str], out: Output) -> String {
    stdout(args, out).trim().to_owned()
}

/// Written to the repository's config file at once rather than with a `git config` call each.
fn append_config(dir: &Path, text: &str) {
    let config = dir.join(".git").join("config");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&config)
        .unwrap_or_else(|e| panic!("open {}: {e}", config.display()));
    file.write_all(text.as_bytes()).expect("write config");
}

/// The test identity, as parterre's own git reads it from the repository: CI has no global
/// one. Automatic maintenance is off too: git starts it as a process of its own after every
/// commit.
fn configure(dir: &Path) {
    append_config(
        dir,
        "[user]\n\tname = Test\n\temail = test@example.com\n\
         [commit]\n\tgpgsign = false\n[tag]\n\tgpgsign = false\n\
         [maintenance]\n\tauto = false\n",
    );
}

/// The hash in what `git commit` printed, `[main (root-commit) <hash>] <subject>`, in full
/// with `core.abbrev=no`; nothing if a hook or an older git printed something else first.
fn committed_hash(out: &str) -> Option<String> {
    let summary = out.lines().next()?.strip_prefix('[')?;
    let hash = summary.split_once("] ")?.0.rsplit(' ').next()?;
    let full = hash.len() >= 40 && hash.bytes().all(|b| b.is_ascii_hexdigit());
    full.then(|| hash.to_owned())
}

pub struct TestRepo {
    pub dir: TempDir,
    clock_minutes: u32,
}

impl TestRepo {
    pub fn new() -> TestRepo {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = TestRepo {
            dir,
            clock_minutes: 0,
        };
        repo.git(&["init", "-q", "-b", "main"]);
        configure(repo.path());
        repo
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn git(&self, args: &[&str]) -> String {
        git_at(self.path(), self.clock_minutes, args)
    }

    /// Returns git's trimmed output.
    pub fn git_with_input(&self, args: &[&str], input: &[u8]) -> String {
        let mut child = git_command(self.path())
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run git");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input)
            .expect("write to git");
        succeeded(args, child.wait_with_output().expect("wait for git"))
    }

    /// Puts a file straight into the index, without the working tree, so that names the
    /// file system can't hold (`A/` beside `a/` on macOS and Windows; newlines, tabs, `:` or
    /// `"` on Windows) still get into commits. Commit with [`TestRepo::commit`].
    pub fn stage(&self, path: &str, contents: &[u8]) {
        // Git for Windows refuses such names in the index unless told not to.
        if !read_text(&self.path().join(".git").join("config")).contains("protectNTFS") {
            append_config(self.path(), "[core]\n\tprotectNTFS = false\n");
        }
        let blob = self.git_with_input(&["hash-object", "-w", "--stdin"], contents);
        let entry = format!("100644 {blob}\t{path}\0");
        self.git_with_input(
            &["update-index", "-z", "--add", "--index-info"],
            entry.as_bytes(),
        );
    }

    /// An empty commit; returns its hash.
    pub fn commit(&mut self, message: &str) -> String {
        self.clock_minutes += 1;
        let out = self.git(&[
            "-c",
            "core.abbrev=no",
            "commit",
            "--allow-empty",
            "-m",
            message,
        ]);
        committed_hash(&out).unwrap_or_else(|| self.git(&["rev-parse", "HEAD"]))
    }

    /// For the commits that follow; each commit first advances it by one. Going backwards
    /// simulates clock skew.
    pub fn set_clock(&mut self, minutes: u32) {
        self.clock_minutes = minutes;
    }

    /// Creates folders as needed.
    pub fn write(&self, path: &str, contents: &[u8]) {
        let full = self.dir.path().join(path);
        std::fs::create_dir_all(full.parent().expect("parent")).expect("mkdir");
        std::fs::write(full, contents).expect("write file");
    }

    /// Stages everything; returns the commit's hash.
    pub fn commit_all(&mut self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.commit(message)
    }

    pub fn checkout(&self, rev: &str) {
        self.git(&["checkout", "-q", rev]);
    }

    pub fn branch(&self, name: &str) {
        self.git(&["checkout", "-q", "-b", name]);
    }

    /// `git merge --no-ff`; returns the merge commit.
    pub fn merge(&mut self, rev: &str, message: &str) -> String {
        self.clock_minutes += 1;
        self.git(&["merge", "-q", "--no-ff", "-m", message, rev]);
        self.git(&["rev-parse", "HEAD"])
    }

    pub fn load(&self) -> Repo {
        parterre_core::git::load_repo(self.path()).expect("load repo")
    }
}

/// main: A - B - C ------- M
///            \           /
/// feature:    D ------- E
pub fn feature_merge() -> TestRepo {
    let mut r = TestRepo::new();
    r.commit("A");
    r.commit("B");
    r.branch("feature");
    r.commit("D");
    r.commit("E");
    r.checkout("main");
    r.commit("C");
    r.merge("feature", "M");
    r
}

impl AsRef<Path> for TestRepo {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

pub fn oid(hex: &str) -> Oid {
    Oid::from_hex(hex).unwrap_or_else(|| panic!("not a hash: {hex:?}"))
}

pub fn rev(r: &TestRepo, rev: &str) -> Oid {
    oid(&r.git(&["rev-parse", rev]))
}

pub fn blame_at(hash: &str, path: &str) -> BlameSpec {
    BlameSpec {
        rev: Rev::Commit(oid(hash)),
        path: path.into(),
    }
}

pub fn ix(repo: &Repo, hash: &str) -> CommitIx {
    repo.lookup(&oid(hash)).expect("commit in snapshot")
}

pub fn execute(worktree: impl AsRef<Path>, action: Action, approval: Option<&Warning>) -> Outcome {
    Branches::new(worktree.as_ref()).execute(action, approval, &Default::default())
}

pub fn done(outcome: Outcome) -> Report {
    match outcome {
        Outcome::Done(report) => report,
        other => panic!("expected it done: {other:?}"),
    }
}

pub fn warning(outcome: Outcome) -> Warning {
    match outcome {
        Outcome::Warning(warning) => warning,
        other => panic!("expected a warning: {other:?}"),
    }
}

pub fn failed(outcome: Outcome) -> String {
    failed_with_report(outcome).0
}

pub fn failed_with_report(outcome: Outcome) -> (String, Report) {
    match outcome {
        Outcome::Failed { error, report } => (error.to_string(), report),
        other => panic!("expected a failure: {other:?}"),
    }
}

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }

    pub fn chance(&mut self, p: f64) -> bool {
        (self.next() % 1_000_000) as f64 / 1e6 < p
    }

    pub fn unit(&mut self) -> f32 {
        (self.next() % 10_000) as f32 / 10_000.0
    }
}
