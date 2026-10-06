//! What opening a repository costs: the graph load, the branch catalogue and the watcher's
//! lookup, as criterion measures them, with warm-up, many samples, outlier detection and a
//! comparison with a saved baseline. Against a small throwaway repository by default, or the
//! one `PARTERRE_BENCH_REPO` names. To compare a change with `main`:
//!
//! ```text
//! git switch main
//! PARTERRE_BENCH_REPO=C:/Source/Apps cargo bench -p parterre-core -- --save-baseline main
//! git switch my-branch
//! PARTERRE_BENCH_REPO=C:/Source/Apps cargo bench -p parterre-core -- --baseline main
//! ```
//!
//! Run from a shell whose PATH is the user's (PowerShell, not Git Bash, on Windows), so that
//! git is found the way it is for them. More of what the app does belongs here as it is
//! measured: opening a diff, say (#309).

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};

/// The repository to measure against, and the folder to keep alive when it is a throwaway one.
fn repository() -> (PathBuf, Option<tempfile::TempDir>) {
    if let Some(path) = std::env::var_os("PARTERRE_BENCH_REPO") {
        return (PathBuf::from(path), None);
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let git = |args: &[&str]| {
        // Plain `git`: building the repository isn't measured, and the bench then compiles
        // against older trees too, for a baseline.
        let out = Command::new("git")
            .current_dir(dir.path())
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Bench")
            .env("GIT_AUTHOR_EMAIL", "bench@example.com")
            .env("GIT_COMMITTER_NAME", "Bench")
            .env("GIT_COMMITTER_EMAIL", "bench@example.com")
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    for i in 0..40 {
        git(&[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            &format!("Commit {i}"),
        ]);
        if i % 10 == 9 {
            git(&["branch", &format!("topic-{i}")]);
            git(&["tag", &format!("v0.{i}")]);
        }
    }
    (dir.path().to_owned(), Some(dir))
}

fn open(c: &mut Criterion) {
    let (path, _keep) = repository();
    let mut group = c.benchmark_group("open");
    // Each sample is a whole open, half a second on a large repository.
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(15));
    group.bench_function("graph load", |b| {
        b.iter(|| parterre_core::git::load_repo(&path).expect("load"))
    });
    // Built from the graph's snapshot, as the app does (#310).
    let repo = parterre_core::git::load_repo(&path).expect("load");
    group.bench_function("branch catalogue", |b| {
        b.iter(|| parterre_core::branches::Catalog::of(&repo).expect("catalogue"))
    });
    group.bench_function("watcher lookup", |b| {
        b.iter(|| parterre_core::watch::RefStorage::locate(&path).expect("locate"))
    });
    group.finish();
}

criterion_group!(benches, open);
criterion_main!(benches);
