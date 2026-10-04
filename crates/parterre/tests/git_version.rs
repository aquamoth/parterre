//! A git older than parterre needs is refused with one clear message (#233), not run and
//! misread.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn a_git_older_than_the_minimum_is_refused_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let git = dir.path().join("git");
    std::fs::write(&git, "#!/bin/sh\necho 'git version 2.30.2'\n").unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_parterre"))
        .arg(dir.path())
        .arg("--export")
        .arg(dir.path().join("graph.svg"))
        .env("PATH", dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(
        stderr.contains("git 2.30.2 is too old; parterre needs git 2.31 or newer"),
        "{stderr}"
    );
}
