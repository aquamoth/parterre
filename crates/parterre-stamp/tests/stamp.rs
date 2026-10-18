//! `parterre-stamp` on a throwaway repository and file.

use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", dir.join("no-gitconfig"))
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn stamp(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new(env!("CARGO_BIN_EXE_parterre-stamp"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    let text = |b: Vec<u8>| String::from_utf8(b).unwrap().trim().to_owned();
    if out.status.success() {
        Ok(text(out.stdout))
    } else {
        Err(text(out.stderr))
    }
}

#[test]
fn versions_count_the_commits_since_the_minors_first_tag() {
    let dir = tempfile_dir("versions");
    git(&dir, &["init", "-q", "-b", "main"]);
    let commit = |n: &str| git(&dir, &["commit", "-q", "--allow-empty", "-m", n]);
    commit("first");
    git(&dir, &["tag", "v1.0.0-rc1"]);
    commit("second");
    git(&dir, &["tag", "v1.0.0"]);
    commit("third");
    let head = git(&dir, &["rev-parse", "--short=7", "HEAD"]);
    let version = |args: &[&str]| stamp(&dir, &[&["version"], args].concat());
    assert_eq!(
        version(&["--ring", "canary"]),
        Ok(format!("1.0.2-canary+{head}"))
    );
    assert_eq!(
        version(&["--ring", "nightly", "--date", "20261016"]),
        Ok(format!("1.0.2-nightly.20261016+{head}"))
    );
    // A tag the pipeline made doesn't count.
    git(&dir, &["tag", "v1.0.2-canary"]);
    git(&dir, &["tag", "v1.1-rc1"]);
    assert_eq!(version(&["--ring", "rc"]), Ok(format!("1.1.0-rc+{head}")));
    commit("fourth");
    git(&dir, &["tag", "v1.1"]);
    let head = git(&dir, &["rev-parse", "--short=7", "HEAD"]);
    assert_eq!(version(&["--ring", "stable"]), Ok(format!("1.1.1+{head}")));
    assert_eq!(
        version(&["--ring", "stable", "--rev", "HEAD~1"]).map(|v| v.starts_with("1.1.0+")),
        Ok(true)
    );
    assert!(version(&["--ring", "nightly"]).is_err());
    assert!(version(&["--ring", "nightly", "--date", "2026-10-16"]).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn write_puts_the_version_in_the_slot_and_read_reads_it() {
    let dir = tempfile_dir("write");
    let binary = dir.join("parterre");
    let mut file = b"\x7fELF and code ".to_vec();
    file.extend(b"@parterre-slot@1\x001.2.3-dev+a1b2c3d");
    file.resize(file.len() + 64, 0);
    std::fs::write(&binary, &file).unwrap();
    let path = binary.to_str().unwrap();
    assert_eq!(stamp(&dir, &["read", path]), Ok("1.2.3-dev+a1b2c3d".into()));
    stamp(&dir, &["write", path, "1.2.3-canary+a1b2c3d"]).unwrap();
    assert_eq!(
        stamp(&dir, &["read", path]),
        Ok("1.2.3-canary+a1b2c3d written".into())
    );
    assert_eq!(std::fs::read(&binary).unwrap().len(), file.len());
    std::fs::remove_dir_all(&dir).unwrap();
}

fn tempfile_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("parterre-stamp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
