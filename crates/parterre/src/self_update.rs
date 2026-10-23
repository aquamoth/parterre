//! *Update now* (#431): parterre downloads its update itself, so that the file carries no Mark
//! of the Web or quarantine for SmartScreen or Gatekeeper to stop, installs it and starts again.
//! Each step waits for the user's click: the download, then the install (#433).
//!
//! - **MSI:** `msiexec /passive` while parterre keeps running. The Restart Manager closes every
//!   parterre window and starts each again on its repository (`restart.rs`, #424); closed
//!   first, nothing would start it again.
//! - **Zip and tarballs:** the binary is swapped for the archive's by renaming, then started on
//!   the repository shown.
//! - **Disk image:** the app is copied beside the running one, swapped by renaming, then opened.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, SystemTime};

use eframe::egui;
use parterre_telemetry::{Channel, DownloadError, Install};

/// How a channel's update is installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// The Windows installer: for all users with `ALLUSERS=1`, as it was installed.
    Msi {
        machine: bool,
    },
    /// The zip on Windows, the tarballs elsewhere.
    Archive,
    DiskImage,
}

impl Method {
    /// `None` where parterre doesn't install the update itself.
    pub fn of(channel: Channel) -> Option<Method> {
        match channel {
            Channel::Msi if cfg!(windows) => Some(Method::Msi { machine: false }),
            Channel::MsiMachine if cfg!(windows) => Some(Method::Msi { machine: true }),
            Channel::Zip if cfg!(windows) => Some(Method::Archive),
            Channel::Tarball if cfg!(unix) => Some(Method::Archive),
            Channel::Dmg if cfg!(target_os = "macos") => Some(Method::DiskImage),
            _ => None,
        }
    }
}

/// Where an update is, as the toolbar's button and the dialog show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Downloading {
        done: u64,
        size: Option<u64>,
    },
    /// Checked and waiting for the click that installs it (#433).
    Downloaded,
    Installing,
    /// Replaced: start this and close.
    Restart(Relaunch),
    /// The installer finished while parterre still runs.
    Installed {
        reboot: bool,
    },
    Failed(Failure),
    Cancelled,
}

/// The new copy of parterre to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Relaunch {
    Binary(PathBuf),
    App(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// "Couldn't …", for the dialog.
    pub message: String,
    /// The checked download, to install by hand: it carries no mark either.
    pub fallback: Option<Fallback>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// Opens the installer or the disk image, as a double-click would.
    Open(PathBuf),
    /// Shows the folder of the archive and what it unpacked to.
    Show(PathBuf),
}

impl Failure {
    fn new(message: impl Into<String>, fallback: Option<Fallback>) -> Failure {
        Failure {
            message: message.into(),
            fallback,
        }
    }
}

/// An update in two steps, each started by the user (#433): the download, then the install.
/// Each runs on a thread of its own.
#[derive(Debug)]
pub struct Updating {
    method: Method,
    dir: PathBuf,
    progress: Arc<Progress>,
    outcome: Receiver<Result<PathBuf, Stage>>,
    /// The checked download, once it is.
    file: Option<PathBuf>,
    installed: Option<Receiver<Stage>>,
    stage: Stage,
}

#[derive(Debug)]
struct Progress {
    done: AtomicU64,
    /// `u64::MAX` while unknown.
    size: AtomicU64,
    cancelled: AtomicBool,
}

impl Updating {
    /// Downloads into a folder of its own, named after `version`.
    pub fn start(version: &str, install: Install, method: Method, ctx: egui::Context) -> Updating {
        let progress = Arc::new(Progress {
            done: AtomicU64::new(0),
            size: AtomicU64::new(u64::MAX),
            cancelled: AtomicBool::new(false),
        });
        let (send, outcome) = mpsc::channel();
        let dir = downloads().join(format!("{version}-{}", std::process::id()));
        let shared = Arc::clone(&progress);
        let into = dir.clone();
        std::thread::spawn(move || {
            let _ = send.send(download(&install, &into, &shared, &ctx));
            ctx.request_repaint();
        });
        Updating {
            method,
            dir,
            progress,
            outcome,
            file: None,
            installed: None,
            stage: Stage::Downloading {
                done: 0,
                size: None,
            },
        }
    }

    pub fn method(&self) -> Method {
        self.method
    }

    /// Installs the download, once it is [`Stage::Downloaded`].
    pub fn install(&mut self, ctx: &egui::Context) {
        let Some(file) = self
            .file
            .clone()
            .filter(|_| self.stage == Stage::Downloaded)
        else {
            return;
        };
        let (send, installed) = mpsc::channel();
        let (method, dir, ctx) = (self.method, self.dir.clone(), ctx.clone());
        std::thread::spawn(move || {
            let stage = match method {
                Method::Msi { machine } => run_installer(&file, machine),
                Method::Archive => replace_from_archive(&file, &dir),
                Method::DiskImage => replace_from_disk_image(&file, &dir),
            };
            let _ = send.send(stage);
            ctx.request_repaint();
        });
        self.installed = Some(installed);
        self.stage = Stage::Installing;
    }

    pub fn stage(&mut self) -> &Stage {
        if let Some(installed) = &self.installed {
            if let Ok(stage) = installed.try_recv() {
                self.stage = stage;
            }
        } else if let Ok(outcome) = self.outcome.try_recv() {
            self.stage = match outcome {
                Ok(file) => {
                    self.file = Some(file);
                    Stage::Downloaded
                }
                Err(stage) => stage,
            };
        } else if matches!(self.stage, Stage::Downloading { .. }) {
            let size = self.progress.size.load(Ordering::Relaxed);
            self.stage = Stage::Downloading {
                done: self.progress.done.load(Ordering::Relaxed),
                size: (size != u64::MAX).then_some(size),
            };
        }
        &self.stage
    }
}

/// An update let go stops downloading.
impl Drop for Updating {
    fn drop(&mut self) {
        self.progress.cancelled.store(true, Ordering::Relaxed);
    }
}

/// The checked file, or the stage it ended in instead.
fn download(
    install: &Install,
    dir: &Path,
    progress: &Progress,
    ctx: &egui::Context,
) -> Result<PathBuf, Stage> {
    parterre_telemetry::download(
        install,
        dir,
        &|done, size| {
            progress.done.store(done, Ordering::Relaxed);
            progress
                .size
                .store(size.unwrap_or(u64::MAX), Ordering::Relaxed);
            ctx.request_repaint_after(Duration::from_millis(100));
        },
        &|| progress.cancelled.load(Ordering::Relaxed),
    )
    .map_err(|e| match e {
        DownloadError::Cancelled => Stage::Cancelled,
        DownloadError::Failed(e) => Stage::Failed(Failure::new(
            format!("Couldn't download the update: {e}."),
            None,
        )),
    })
}

/// Runs the installer while parterre runs on, for the Restart Manager to close and start again.
fn run_installer(msi: &Path, machine: bool) -> Stage {
    let fallback = Some(Fallback::Open(msi.to_owned()));
    match Command::new(system_program("msiexec.exe"))
        .args(installer_arguments(msi, machine))
        .status()
    {
        Ok(status) => installer_outcome(status.code(), fallback),
        Err(e) => Stage::Failed(Failure::new(
            format!("Couldn't start the installer: {e}."),
            fallback,
        )),
    }
}

/// Basic UI: msiexec's progress bar, no questions. Under it the Restart Manager closes and
/// starts again the programs that hold files in use, as no dialog may ask (#431).
fn installer_arguments(msi: &Path, machine: bool) -> Vec<std::ffi::OsString> {
    let mut arguments: Vec<std::ffi::OsString> = vec![
        "/i".into(),
        msi.into(),
        "/passive".into(),
        "/norestart".into(),
    ];
    // Windows Installer updates an installation in its own context only.
    if machine {
        arguments.push("ALLUSERS=1".into());
    }
    arguments
}

/// msiexec's exit code (learn.microsoft.com/windows/win32/msi/error-codes).
fn installer_outcome(code: Option<i32>, fallback: Option<Fallback>) -> Stage {
    match code {
        Some(0) => Stage::Installed { reboot: false },
        Some(3010 | 1641) => Stage::Installed { reboot: true },
        // Also a cancelled UAC prompt.
        Some(1602) => Stage::Cancelled,
        Some(1618) => Stage::Failed(Failure::new(
            "Another installation is running. Update again once it has finished.",
            fallback,
        )),
        Some(code) => Stage::Failed(Failure::new(
            format!("The installer failed (error {code}), and left parterre as it was."),
            fallback,
        )),
        None => Stage::Failed(Failure::new("The installer was stopped.", fallback)),
    }
}

fn replace_from_archive(archive: &Path, dir: &Path) -> Stage {
    let unpacked = dir.join("unpacked");
    let show = Some(Fallback::Show(dir.to_owned()));
    let new = match unpack(archive, &unpacked) {
        Ok(new) => new,
        Err(e) => return Stage::Failed(Failure::new(format!("Couldn't unpack it: {e}."), show)),
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            return Stage::Failed(Failure::new(
                format!("Couldn't tell where parterre is: {e}."),
                show,
            ));
        }
    };
    match replace_binary(&exe, &new) {
        Ok(()) => Stage::Restart(Relaunch::Binary(exe)),
        Err(e) => Stage::Failed(Failure::new(
            format!("Couldn't replace {}: {e}.", exe.display()),
            show,
        )),
    }
}

/// The name of parterre's binary, in the archives as installed.
const BINARY: &str = if cfg!(windows) {
    "parterre.exe"
} else {
    "parterre"
};

/// Unpacks `archive` into `into` with the system's tar, which reads zip files on Windows too:
/// the binary it holds, in the archive's one folder.
fn unpack(archive: &Path, into: &Path) -> Result<PathBuf, String> {
    let _ = std::fs::remove_dir_all(into);
    std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
    let mut tar = Command::new(tar_program());
    tar.arg("-xf").arg(archive).arg("-C").arg(into);
    hide_console(&mut tar);
    let output = tar.output().map_err(|e| format!("tar: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "tar: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    find_binary(into).ok_or_else(|| format!("the archive holds no {BINARY}"))
}

/// `BINARY` in `dir` or in a folder of it, as the release archives hold it.
fn find_binary(dir: &Path) -> Option<PathBuf> {
    let direct = dir.join(BINARY);
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join(BINARY))
        .find(|path| path.is_file())
}

/// Puts `new` where `exe` is. A copy beside it first, so that the swap is a rename on the same
/// volume: atomic on Unix. Windows lets a running program be renamed but not replaced, so the
/// old one is moved aside, and removed by a later start ([`clean_up`]).
fn replace_binary(exe: &Path, new: &Path) -> std::io::Result<()> {
    let dir = exe.parent().ok_or(std::io::ErrorKind::NotFound)?;
    let staged = dir.join(format!("{BINARY}.new"));
    std::fs::copy(new, &staged)?;
    #[cfg(windows)]
    {
        let aside = dir.join(format!("{BINARY}{OLD}{}", std::process::id()));
        if let Err(e) = std::fs::rename(exe, &aside) {
            let _ = std::fs::remove_file(&staged);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&staged, exe) {
            let _ = std::fs::rename(&aside, exe);
            let _ = std::fs::remove_file(&staged);
            return Err(e);
        }
        Ok(())
    }
    #[cfg(not(windows))]
    std::fs::rename(&staged, exe).inspect_err(|_| {
        let _ = std::fs::remove_file(&staged);
    })
}

/// Before the process ID in the name of a binary moved aside.
const OLD: &str = ".old-";

/// Copies the image's app beside the running one with `ditto`, then swaps the two by renaming.
fn replace_from_disk_image(image: &Path, dir: &Path) -> Stage {
    let open = Some(Fallback::Open(image.to_owned()));
    let bundle = match std::env::current_exe()
        .map_err(|e| e.to_string())
        .and_then(|exe| running_app(&exe))
    {
        Ok(bundle) => bundle,
        Err(e) => return Stage::Failed(Failure::new(e, open)),
    };
    let mount = dir.join("mount");
    if let Err(e) = std::fs::create_dir_all(&mount) {
        return Stage::Failed(Failure::new(format!("Couldn't open the image: {e}."), open));
    }
    let attach = run(Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-quiet"])
        .arg("-mountpoint")
        .arg(&mount)
        .arg(image));
    if let Err(e) = attach {
        return Stage::Failed(Failure::new(format!("Couldn't open the image: {e}."), open));
    }
    let replaced = replace_app(&mount.join("parterre.app"), &bundle);
    let _ = run(Command::new("hdiutil")
        .args(["detach", "-quiet"])
        .arg(&mount));
    match replaced {
        Ok(()) => Stage::Restart(Relaunch::App(bundle)),
        Err(e) => Stage::Failed(Failure::new(
            format!("Couldn't replace {}: {e}.", bundle.display()),
            open,
        )),
    }
}

/// The app bundle `exe` runs from, if parterre may replace it: not one run from the disk image,
/// nor one macOS runs from a hidden copy because it was quarantined (App Translocation).
fn running_app(exe: &Path) -> Result<PathBuf, String> {
    let bundle = exe
        .ancestors()
        .nth(3)
        .filter(|b| b.extension().is_some_and(|e| e == "app"))
        .ok_or("parterre doesn't run from an app")?;
    let path = bundle.to_string_lossy();
    if path.starts_with("/Volumes/") || path.contains("/AppTranslocation/") {
        return Err("parterre runs from the disk image. Drag it to Applications first.".to_owned());
    }
    Ok(bundle.to_owned())
}

fn replace_app(new: &Path, bundle: &Path) -> Result<(), String> {
    let parent = bundle.parent().ok_or("no folder")?;
    let staged = parent.join(".parterre-update.app");
    let aside = parent.join(".parterre-old.app");
    let _ = std::fs::remove_dir_all(&staged);
    let _ = std::fs::remove_dir_all(&aside);
    run(Command::new("ditto").arg(new).arg(&staged))?;
    if let Err(e) = std::fs::rename(bundle, &aside) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(e.to_string());
    }
    if let Err(e) = std::fs::rename(&staged, bundle) {
        let _ = std::fs::rename(&aside, bundle);
        let _ = std::fs::remove_dir_all(&staged);
        return Err(e.to_string());
    }
    let _ = std::fs::remove_dir_all(&aside);
    Ok(())
}

/// Runs `cmd` to its end: what it said on stderr if it failed.
fn run(cmd: &mut Command) -> Result<(), String> {
    let output = cmd.output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// Starts the new copy on `repo`, as parterre was started on it.
pub fn relaunch(target: &Relaunch, repo: Option<&Path>) -> std::io::Result<()> {
    let cmd = match target {
        Relaunch::Binary(exe) => {
            let mut cmd = Command::new(exe);
            cmd.args(repo);
            cmd
        }
        // A new instance: the running one would otherwise only be brought to the front.
        Relaunch::App(bundle) => {
            let mut cmd = Command::new("open");
            cmd.arg("-n").arg(bundle);
            if let Some(repo) = repo {
                cmd.arg("--args").arg(repo);
            }
            cmd
        }
    };
    crate::browser::spawn_detached(cmd)
}

/// The folder downloads go to, one per update in it.
fn downloads() -> PathBuf {
    std::env::temp_dir().join("parterre-update")
}

/// Removes, on a thread of its own, what earlier updates left: downloads from before today, which
/// the installer may have still held, and on Windows the binaries moved aside.
pub fn clean_up() {
    std::thread::spawn(|| {
        let day_ago = SystemTime::now() - Duration::from_secs(24 * 60 * 60);
        for entry in std::fs::read_dir(downloads())
            .into_iter()
            .flatten()
            .flatten()
        {
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|modified| modified < day_ago);
            if old {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
        if cfg!(windows)
            && let Some(dir) = std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(Path::to_owned))
        {
            let prefix = format!("{BINARY}{OLD}");
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                // One still running, in another window, can't be removed yet.
                if entry.file_name().to_string_lossy().starts_with(&prefix) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    });
}

/// `name` in Windows' own folder, so that no program of that name earlier on `PATH` runs: Git
/// for Windows puts a tar there that doesn't read zip files.
fn system_program(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    Path::new(&root).join("System32").join(name)
}

fn tar_program() -> PathBuf {
    if cfg!(windows) {
        system_program("tar.exe")
    } else {
        PathBuf::from("tar")
    }
}

/// No console window flashes up for a console program parterre starts.
fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// The size of a download so far, in megabytes with one decimal: "4.1 of 12.0 MB".
pub fn progress_text(done: u64, size: Option<u64>) -> String {
    let mb = |bytes: u64| bytes as f64 / 1_000_000.0;
    match size {
        Some(size) => format!("{:.1} of {:.1} MB", mb(done), mb(size)),
        None => format!("{:.1} MB", mb(done)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_channel_whose_download_is_marked_installs_itself() {
        let method = |channel| Method::of(channel);
        if cfg!(windows) {
            assert_eq!(method(Channel::Msi), Some(Method::Msi { machine: false }));
            assert_eq!(
                method(Channel::MsiMachine),
                Some(Method::Msi { machine: true })
            );
            assert_eq!(method(Channel::Zip), Some(Method::Archive));
            assert_eq!(method(Channel::Tarball), None);
        } else {
            assert_eq!(method(Channel::Msi), None);
            assert_eq!(method(Channel::Zip), None);
            assert_eq!(method(Channel::Tarball), Some(Method::Archive));
        }
        let dmg = cfg!(target_os = "macos").then_some(Method::DiskImage);
        assert_eq!(method(Channel::Dmg), dmg);
        for channel in [
            Channel::Deb,
            Channel::Rpm,
            Channel::Cargo,
            Channel::Snap,
            Channel::Flatpak,
        ] {
            assert_eq!(method(channel), None, "{channel:?}");
        }
    }

    #[test]
    fn a_machine_wide_installation_is_updated_for_all_users() {
        let msi = Path::new(r"C:\Temp\parterre update\parterre-1.2.3.msi");
        let args = |machine| -> Vec<String> {
            installer_arguments(msi, machine)
                .into_iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(
            args(false),
            [
                "/i",
                r"C:\Temp\parterre update\parterre-1.2.3.msi",
                "/passive",
                "/norestart"
            ]
        );
        assert_eq!(args(true)[4..], ["ALLUSERS=1"]);
    }

    #[test]
    fn the_installers_exit_code_says_how_it_went() {
        let outcome = |code| installer_outcome(code, None);
        assert_eq!(outcome(Some(0)), Stage::Installed { reboot: false });
        assert_eq!(outcome(Some(3010)), Stage::Installed { reboot: true });
        assert_eq!(outcome(Some(1641)), Stage::Installed { reboot: true });
        assert_eq!(outcome(Some(1602)), Stage::Cancelled);
        let message = |code| match outcome(code) {
            Stage::Failed(f) => f.message,
            other => panic!("{other:?}"),
        };
        assert!(message(Some(1618)).starts_with("Another installation is running"));
        assert!(message(Some(1603)).contains("error 1603"));
        assert_eq!(message(None), "The installer was stopped.");
    }

    #[test]
    fn the_binary_is_found_in_the_archives_folder() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(find_binary(dir.path()), None);
        let folder = dir.path().join("parterre-1.2.3-x86_64-pc-windows-msvc");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("README.md"), "").unwrap();
        assert_eq!(find_binary(dir.path()), None);
        std::fs::write(folder.join(BINARY), "").unwrap();
        assert_eq!(find_binary(dir.path()), Some(folder.join(BINARY)));
    }

    #[test]
    fn the_running_binary_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join(BINARY);
        let new = dir.path().join("downloaded");
        std::fs::write(&exe, "old").unwrap();
        std::fs::write(&new, "new").unwrap();
        replace_binary(&exe, &new).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        let left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != BINARY && name != "downloaded")
            .collect();
        if cfg!(windows) {
            assert_eq!(left, [format!("{BINARY}{OLD}{}", std::process::id())]);
        } else {
            assert!(left.is_empty(), "{left:?}");
        }
    }

    #[test]
    fn an_archive_is_unpacked_with_the_systems_tar() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("parterre-1.2.3");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join(BINARY), "new").unwrap();
        let archive = dir
            .path()
            .join(if cfg!(windows) { "a.zip" } else { "a.tar.gz" });
        // bsdtar writes zip files with -a, from the name; GNU tar a gzipped tarball with -z.
        let create = if cfg!(windows) { "-acf" } else { "-czf" };
        let status = Command::new(tar_program())
            .arg(create)
            .arg(&archive)
            .arg("-C")
            .arg(dir.path())
            .arg("parterre-1.2.3")
            .status()
            .unwrap();
        assert!(status.success());
        let into = dir.path().join("unpacked");
        let binary = unpack(&archive, &into).unwrap();
        assert_eq!(binary, into.join("parterre-1.2.3").join(BINARY));
        assert_eq!(std::fs::read_to_string(binary).unwrap(), "new");
        assert!(unpack(&dir.path().join("none.zip"), &into).is_err());
    }

    #[test]
    fn only_an_app_of_its_own_is_replaced() {
        let at = |exe: &str| running_app(Path::new(exe));
        assert_eq!(
            at("/Applications/parterre.app/Contents/MacOS/parterre"),
            Ok(PathBuf::from("/Applications/parterre.app"))
        );
        assert!(at("/Volumes/parterre/parterre.app/Contents/MacOS/parterre").is_err());
        assert!(
            at("/private/var/folders/x/AppTranslocation/1234/d/parterre.app/Contents/MacOS/parterre")
                .is_err()
        );
        assert!(at("/usr/local/bin/parterre").is_err());
    }

    #[test]
    fn the_progress_is_told_in_megabytes() {
        assert_eq!(progress_text(4_100_000, Some(12_000_000)), "4.1 of 12.0 MB");
        assert_eq!(progress_text(0, None), "0.0 MB");
    }
}
