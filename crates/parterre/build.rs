//! Every build is a dev build: the pipeline writes the version it publishes a build under into
//! the binary afterwards (`parterre-stamp`, docs/releasing.md).
//!
//! The sources are either a git checkout of the workspace, or a crate packaged by `cargo
//! package` (from crates.io by `cargo install`, say), which has no `.git` but records its
//! commit in `.cargo_vcs_info.json`.

#[path = "src/notice.rs"]
mod notice;
#[path = "src/slot.rs"]
#[allow(dead_code)]
mod slot;
#[path = "src/version.rs"]
#[allow(dead_code)]
mod version;
#[path = "src/win_resource.rs"]
mod win_resource;

use std::path::{Path, PathBuf};
use std::process::Command;

use version::{GitState, Source};

/// Relative to the workspace root. Only changes here make a build dirty, and they rerun this
/// script so that the flag stays current.
const SOURCES: [&str; 6] = [
    "crates",
    "packaging/icon",
    "Cargo.toml",
    "Cargo.lock",
    ".cargo",
    "rust-toolchain.toml",
];

fn main() {
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest_dir.join("../..");
    let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap();

    let packaged = manifest_dir.join("Cargo.toml.orig").exists();

    // A packaged crate carries its own copies of LICENSE, NOTICE and the icon. In the
    // workspace the crate's are symlinks, which Windows checkouts may turn into text files
    // holding the link's target, so read the originals.
    let (legal_dir, icon_dir) = if packaged {
        (manifest_dir.clone(), manifest_dir.clone())
    } else {
        (root.clone(), root.join("packaging/icon"))
    };
    for name in ["LICENSE", "NOTICE"] {
        let path = legal_dir.join(name);
        println!("cargo:rustc-env=PARTERRE_{name}={}", path.display());
    }

    let source = if packaged {
        // Packaged: the workspace around it (if any) isn't what is being built.
        let info = std::fs::read_to_string(manifest_dir.join(".cargo_vcs_info.json"));
        Source::Package(
            info.map(|s| version::parse_vcs_info(&s))
                .unwrap_or_default(),
        )
    } else if let Some(git) = git_state(&root) {
        watch(&root);
        Source::Git(git)
    } else {
        Source::Unknown
    };
    let version = version::describe(&pkg_version, &source);
    println!("cargo:rustc-env=PARTERRE_VERSION={version}");

    windows_resources(
        &icon_dir.join("parterre.ico"),
        &legal_dir.join("NOTICE"),
        &version,
    );
}

/// `ico` comes from `cargo run --release -p parterre-core --example icon_assets`. Cosmetic, so
/// a missing resource compiler is skipped: `rc.exe` comes with the Windows SDK, and checking the
/// `windows-gnu` target from Linux needs `x86_64-w64-mingw32-windres`.
fn windows_resources(ico: &Path, notice_path: &Path, version: &str) {
    println!("cargo:rerun-if-changed={}", ico.display());
    println!("cargo:rerun-if-changed={}", notice_path.display());
    let numbers = version.split(['-', '+']).next().unwrap();
    let mut parts = numbers.split('.');
    let mut part = || {
        parts
            .next()
            .and_then(|number| number.parse().ok())
            .unwrap_or_else(|| panic!("{numbers} doesn't fit a Windows version number"))
    };
    let notice_text = std::fs::read_to_string(notice_path)
        .unwrap_or_else(|e| panic!("{}: {e}", notice_path.display()));
    let script = win_resource::script(&win_resource::Resources {
        icon_path: &ico.display().to_string(),
        numeric_version: [part(), part(), part()],
        // Padded, so that `parterre-stamp` can write a published version in its place.
        version: &format!("{version:<width$}", width = slot::ROOM),
        description: &std::env::var("CARGO_PKG_DESCRIPTION").unwrap(),
        copyright: notice::copyright_line(&notice_text),
    });
    let rc = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("parterre.rc");
    std::fs::write(&rc, script).unwrap();
    embed_resource::compile(rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap_or_else(|e| panic!("{e}"));
}

/// `None` if this isn't a git checkout (a source archive, say) or git isn't installed.
fn git_state(dir: &Path) -> Option<GitState> {
    // Only a repository of our own counts. Unpacked into some other checkout (a packaging
    // repository, say), git would describe that one instead.
    let top = git(dir, &["rev-parse", "--show-toplevel"])?;
    if Path::new(&top).canonicalize().ok()? != dir.canonicalize().ok()? {
        return None;
    }
    let commit = git(dir, &["rev-parse", "--short=7", "HEAD"])?;
    // No optional locks: a plain `git status` may refresh the index, and building shouldn't
    // write to the repository.
    let mut status = vec![
        "--no-optional-locks",
        "status",
        "--porcelain",
        "--untracked-files=no",
        "--",
    ];
    status.extend(SOURCES);
    let status = git(dir, &status)?;
    Some(GitState {
        short_hash: commit,
        dirty: !status.is_empty(),
        // None in a shallow clone without tags.
        base: version::base(|args| git(dir, args), "HEAD"),
    })
}

/// So that the hash and dirty flag stay current without rerunning on every build.
fn watch(dir: &Path) {
    let mut paths = Vec::new();
    // HEAD's reflog changes on every commit, checkout and reset, including in worktrees; a new
    // tag lands in refs/tags.
    for name in ["HEAD", "logs/HEAD", "packed-refs", "refs/tags"] {
        paths.extend(git(dir, &["rev-parse", "--git-path", name]));
    }
    if let Some(branch) = git(dir, &["rev-parse", "--symbolic-full-name", "HEAD"])
        && branch != "HEAD"
    {
        paths.extend(git(dir, &["rev-parse", "--git-path", &branch]));
    }
    paths.extend(SOURCES.map(String::from));
    // Cargo reruns every build for a path that doesn't exist, so skip those.
    for p in paths.iter().map(|p| dir.join(p)).filter(|p| p.exists()) {
        println!("cargo:rerun-if-changed={}", p.display());
    }
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
