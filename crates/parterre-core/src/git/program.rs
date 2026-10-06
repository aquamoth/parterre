//! Which `git` to run.
//!
//! Everywhere but Windows that's plain `git`. On Windows it's the real `git.exe` of Git for
//! Windows, `<install>\mingw64\bin\git.exe` (`clangarm64` or `mingw32` for other builds),
//! found once: through the `git.exe` on PATH, which is usually the `cmd\git.exe` launcher,
//! or when there is none, through the install the registry or the default folders name
//! (a terminal opened before Git for Windows was installed, or an installer told to leave
//! PATH alone). The launchers in `cmd\` and `bin\` start the real git as a second process,
//! which on Windows costs more than the first: run directly, each git call starts about
//! 40 ms sooner (#309). The real git.exe does the launcher's work itself, putting its
//! `usr\bin` on its children's PATH so that hooks, `sh` and `ssh` still run. The copy in
//! `libexec\git-core` is slower to start and isn't used.
//!
//! Some other `git.exe` on PATH, such as a package manager's shim, is run as found.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The program parterre runs for git, looked up once. Public for the test helpers, which build
/// their repositories with the same git.
pub fn program() -> &'static Path {
    static PROGRAM: OnceLock<PathBuf> = OnceLock::new();
    PROGRAM.get_or_init(find)
}

#[cfg(not(windows))]
fn find() -> PathBuf {
    PathBuf::from("git")
}

#[cfg(windows)]
fn find() -> PathBuf {
    locate(
        std::env::var_os("PATH").as_deref(),
        &windows::install_dirs(),
        Path::is_file,
    )
}

/// The folders an install may hold the real `git.exe` under `bin\` of, one per build; an
/// install has one of them.
#[cfg(any(windows, test))]
const BUILD_DIRS: &[&str] = &["mingw64", "clangarm64", "mingw32"];

/// The first `git.exe` on `path` (a PATH value), replaced by the real git of its install when
/// it is a `cmd\` or `bin\` launcher; otherwise the real git, or else the launcher, of the
/// first of `install_dirs` that has one. Plain `git` if there is none at all, so that
/// spawning it fails with the usual error.
#[cfg(any(windows, test))]
fn locate(
    path: Option<&std::ffi::OsStr>,
    install_dirs: &[PathBuf],
    is_file: impl Fn(&Path) -> bool,
) -> PathBuf {
    let real_git = |install: &Path| {
        BUILD_DIRS
            .iter()
            .map(|build| install.join(build).join("bin").join("git.exe"))
            .find(|exe| is_file(exe))
    };
    let on_path = path
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join("git.exe"))
        .find(|exe| is_file(exe));
    if let Some(exe) = on_path {
        // The launchers sit in `<install>\cmd` and `<install>\bin`; so does the real git of
        // an install, under `<build>\bin`, which has no real git beside it.
        let dir = exe.parent();
        let in_cmd_or_bin = dir
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.eq_ignore_ascii_case("cmd") || name.eq_ignore_ascii_case("bin")
            });
        if in_cmd_or_bin && let Some(real) = dir.and_then(Path::parent).and_then(real_git) {
            return real;
        }
        return exe;
    }
    let installed = install_dirs.iter().find_map(|dir| {
        real_git(dir).or_else(|| Some(dir.join("cmd").join("git.exe")).filter(|exe| is_file(exe)))
    });
    installed.unwrap_or_else(|| PathBuf::from("git"))
}

#[cfg(windows)]
mod windows {
    use std::path::PathBuf;

    use winreg::RegKey;
    use winreg::enums::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    };

    /// Where Git for Windows may be installed, most likely first: the `InstallPath` its installer
    /// records (per-user, then machine-wide; 64-bit, then 32-bit), then its default folders.
    pub(super) fn install_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
                let path = RegKey::predef(root)
                    .open_subkey_with_flags(r"SOFTWARE\GitForWindows", KEY_READ | view)
                    .and_then(|key| key.get_value::<String, _>("InstallPath"));
                dirs.extend(path.map(PathBuf::from));
            }
        }
        let default = |var, rest: &str| std::env::var_os(var).map(|d| PathBuf::from(d).join(rest));
        dirs.extend(default("ProgramFiles", "Git"));
        dirs.extend(default("LOCALAPPDATA", r"Programs\Git"));
        dirs.extend(default("ProgramFiles(x86)", "Git"));
        dirs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashSet;
    use std::ffi::OsString;

    fn path_var(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    fn locate_with(path: &[&Path], install_dirs: &[&Path], files: &[PathBuf]) -> PathBuf {
        let files: HashSet<_> = files.iter().collect();
        let install_dirs: Vec<_> = install_dirs.iter().map(PathBuf::from).collect();
        locate(Some(&path_var(path)), &install_dirs, |p| {
            files.contains(&p.to_path_buf())
        })
    }

    /// `<install>\cmd\git.exe`, the launcher.
    fn launcher(install: &str) -> PathBuf {
        Path::new(install).join("cmd").join("git.exe")
    }

    /// `<install>\<build>\bin\git.exe`, the real git.
    fn real(install: &str, build: &str) -> PathBuf {
        Path::new(install).join(build).join("bin").join("git.exe")
    }

    #[test]
    fn the_launcher_on_path_gives_way_to_the_real_git_beside_it() {
        let files = [
            launcher("Git"),
            real("Git", "mingw64"),
            real("Other", "mingw64"),
        ];
        let cmd = Path::new("Git").join("cmd");
        assert_eq!(
            locate_with(&[Path::new("tools"), &cmd], &[Path::new("Other")], &files),
            real("Git", "mingw64")
        );
        // `bin\git.exe` is a launcher too.
        let bin = Path::new("Git").join("bin");
        let bin_launcher = bin.join("git.exe");
        assert_eq!(
            locate_with(&[&bin], &[], &[bin_launcher, real("Git", "clangarm64")]),
            real("Git", "clangarm64")
        );
    }

    #[test]
    fn git_on_path_is_otherwise_run_as_found() {
        // The real git itself.
        let bin = Path::new("Git").join("mingw64").join("bin");
        assert_eq!(
            locate_with(&[&bin], &[], &[real("Git", "mingw64")]),
            real("Git", "mingw64")
        );
        // Some other git, say a shim: PATH wins over an install.
        let shim = Path::new("shims").join("git.exe");
        assert_eq!(
            locate_with(
                &[Path::new("shims")],
                &[Path::new("Git")],
                &[shim.clone(), real("Git", "mingw64")]
            ),
            shim
        );
        // A launcher with nothing beside it.
        let cmd = Path::new("Git").join("cmd");
        assert_eq!(
            locate_with(
                &[&cmd],
                &[Path::new("Other")],
                &[launcher("Git"), real("Other", "mingw64")]
            ),
            launcher("Git")
        );
    }

    #[test]
    fn otherwise_the_first_install_dir_with_git() {
        let files = [
            launcher("second"),
            real("second", "mingw64"),
            real("third", "mingw64"),
        ];
        let installs = [Path::new("first"), Path::new("second"), Path::new("third")];
        assert_eq!(
            locate_with(&[Path::new("bin")], &installs, &files),
            real("second", "mingw64")
        );
        // Its launcher if the real git isn't where it should be.
        assert_eq!(
            locate_with(
                &[Path::new("bin")],
                &installs,
                &[launcher("second"), real("third", "mingw64")]
            ),
            launcher("second")
        );
    }

    /// What this machine resolves: never a `cmd` launcher, which is what PATH usually leads
    /// to and what the `windows-latest` runner's does (#309).
    #[cfg(windows)]
    #[test]
    fn the_launcher_is_not_what_runs_here() {
        let exe = find();
        let folder = exe
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        assert!(
            !folder.eq_ignore_ascii_case("cmd"),
            "parterre would run the launcher {}",
            exe.display()
        );
    }

    #[test]
    fn plain_git_when_nothing_is_found() {
        assert_eq!(
            locate_with(&[Path::new("bin")], &[Path::new("first")], &[]),
            Path::new("git")
        );
        assert_eq!(locate(None, &[], |_| false), Path::new("git"));
    }
}
