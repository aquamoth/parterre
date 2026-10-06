//! Builds throwaway git repositories with scripted histories for integration tests.
//!
//! With the git parterre itself runs (`parterre_core::git::program()`: on Windows the real
//! `git.exe`, not the `cmd\git.exe` launcher), and in as few git calls as will do: starting
//! git is what the suite's time goes on, on Windows above all (#309).

#![allow(dead_code)]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use parterre_core::Repo;
use tempfile::TempDir;

/// A file's text, with the CRLF that `core.autocrlf` gives it on checkout undone: Git for
/// Windows sets it in its system config, which parterre's git reads and [`TestRepo::git`]
/// doesn't (#182). Compare files parterre checked out with this, never their bytes.
pub fn read_text(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .replace("\r\n", "\n")
}

/// Gives the repository at `dir` the test identity and turns signing off, written to its config
/// file at once rather than with a `git config` call each. parterre's own git reads the
/// identity from the repository: CI has no global one. Automatic maintenance is off too: git
/// starts it as a process of its own after every commit.
pub fn configure(dir: &Path) {
    let config = dir.join(".git").join("config");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&config)
        .unwrap_or_else(|e| panic!("open {}: {e}", config.display()));
    file.write_all(
        b"[user]\n\tname = Test\n\temail = test@example.com\n\
          [commit]\n\tgpgsign = false\n[tag]\n\tgpgsign = false\n\
          [maintenance]\n\tauto = false\n",
    )
    .expect("write config");
}

/// The hash in what `git commit` printed, `[main (root-commit) <hash>] <subject>`, in full
/// with `core.abbrev=no`; nothing if a hook or an older git printed something else first.
pub fn committed(out: &str) -> Option<String> {
    let summary = out.lines().next()?.strip_prefix('[')?;
    let hash = summary.split_once("] ")?.0.rsplit(' ').next()?;
    let full = hash.len() >= 40 && hash.bytes().all(|b| b.is_ascii_hexdigit());
    full.then(|| hash.to_owned())
}

pub struct TestRepo {
    pub dir: TempDir,
    clock: u32,
}

impl TestRepo {
    pub fn new() -> TestRepo {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = TestRepo { dir, clock: 0 };
        repo.git(&["init", "-q", "-b", "main"]);
        configure(repo.path());
        repo
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn git(&self, args: &[&str]) -> String {
        let date = format!("{} +0000", 1_700_000_000 + self.clock * 60);
        let out = Command::new(parterre_core::git::program())
            .current_dir(self.dir.path())
            .args(args)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// Runs git with `input` on its standard input; returns its trimmed output.
    pub fn git_with_input(&self, args: &[&str], input: &[u8]) -> String {
        let mut child = Command::new(parterre_core::git::program())
            .current_dir(self.dir.path())
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
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
        let out = child.wait_with_output().expect("wait for git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// Puts a file straight into the index, without the working tree, so that names the
    /// file system can't hold (`A/` beside `a/` on macOS and Windows; newlines, tabs, `:` or
    /// `"` on Windows) still get into commits. Commit with [`TestRepo::commit`].
    pub fn stage(&self, path: &str, contents: &[u8]) {
        // Git for Windows refuses such names in the index unless told not to.
        self.git(&["config", "core.protectNTFS", "false"]);
        let blob = self.git_with_input(&["hash-object", "-w", "--stdin"], contents);
        let entry = format!("100644 {blob}\t{path}\0");
        self.git_with_input(
            &["update-index", "-z", "--add", "--index-info"],
            entry.as_bytes(),
        );
    }

    /// Makes an empty commit with `message` as subject and returns its hash.
    pub fn commit(&mut self, message: &str) -> String {
        self.clock += 1;
        let out = self.git(&[
            "-c",
            "core.abbrev=no",
            "commit",
            "--allow-empty",
            "-m",
            message,
        ]);
        committed(&out).unwrap_or_else(|| self.git(&["rev-parse", "HEAD"]))
    }

    /// Sets the clock (minutes after the base date) for the commits that follow; each commit
    /// first advances it by one. Going backwards simulates clock skew.
    pub fn set_clock(&mut self, minutes: u32) {
        self.clock = minutes;
    }

    /// Writes a file in the working tree, creating folders as needed.
    pub fn write(&self, path: &str, contents: &[u8]) {
        let full = self.dir.path().join(path);
        std::fs::create_dir_all(full.parent().expect("parent")).expect("mkdir");
        std::fs::write(full, contents).expect("write file");
    }

    /// Stages everything and commits it; returns the commit's hash.
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

    /// `git merge --no-ff` of `rev` into the current branch; returns the merge commit.
    pub fn merge(&mut self, rev: &str, message: &str) -> String {
        self.clock += 1;
        self.git(&["merge", "-q", "--no-ff", "-m", message, rev]);
        self.git(&["rev-parse", "HEAD"])
    }

    pub fn load(&self) -> Repo {
        parterre_core::git::load_repo(self.path()).expect("load repo")
    }
}
