//! The user's merge tool: the one git's config names, as `git mergetool` picks it, or one
//! installed that the user picks when none is usable. Parterre only starts it, through
//! `git mergetool`, and leaves it be: the tool does the resolving, and git stages the file.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::git::{Config, Git, GitError};

/// Tools that run in a terminal, which parterre has none of.
fn is_terminal(name: &str) -> bool {
    name == "emerge" || name.starts_with("vimdiff") || name.starts_with("nvimdiff")
}

/// What git's config says, for opening a file in a GUI merge tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Configured {
    /// `merge.guitool`, or else `merge.tool`, names an installed GUI tool.
    Usable(String),
    /// A tool is named but can't be used here, and why, in a line.
    Unusable {
        why: String,
        /// The configured tool was a terminal one: a choice goes to `merge.guitool`.
        terminal: bool,
    },
    /// Neither is set.
    None,
}

/// A tool the user can pick, and where parterre found it when git wouldn't.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tool {
    pub name: String,
    /// Found by Windows' App Paths, not by git: passed as `mergetool.<name>.path`.
    pub path: Option<PathBuf>,
}

/// The config and the installed GUI tools.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detected {
    pub configured: Configured,
    /// For the picker: installed, not terminal ones; a configured one App Paths found first.
    pub installed: Vec<Tool>,
}

impl Detected {
    /// The tool to open files in without asking.
    pub fn usable(&self) -> Option<Tool> {
        match &self.configured {
            Configured::Usable(name) => Some(Tool {
                name: name.clone(),
                path: None,
            }),
            _ => None,
        }
    }
}

/// Reads git's config in one call and looks for the tools' programs as git would, where
/// `git mergetool --tool-help` starts git over a hundred times, which takes a minute on
/// Windows (#355).
pub fn detect(git: &Git) -> Result<Detected, GitError> {
    let lookup = Lookup {
        path: crate::shell_path::path()
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default(),
        program_files: program_files(),
        is_file: Path::is_file,
    };
    Ok(decide(&git.config()?, &lookup, app_paths))
}

/// What `config` says and `lookup` finds; `app_paths` finds a tool by Windows' App Paths.
fn decide(
    config: &Config,
    lookup: &Lookup<impl Fn(&Path) -> bool>,
    app_paths: impl Fn(&str) -> Option<PathBuf>,
) -> Detected {
    let mut user = config.subsections("mergetool", "cmd");
    user.sort();
    let available = |name: &str| {
        if user.iter().any(|u| u == name) {
            return true;
        }
        let Some(tool) = BUILT_IN.iter().find(|t| t.names.contains(&name)) else {
            return false;
        };
        // git runs the program `mergetool.<name>.path` names, if set, and no other.
        match config
            .get(&format!("mergetool.{name}.path"))
            .filter(|p| !p.is_empty())
        {
            Some(path) => lookup.finds(path),
            None => lookup.finds_built_in(tool),
        }
    };
    let found = |name: &str| {
        app_paths(name).map(|path| Tool {
            name: name.to_owned(),
            path: Some(path),
        })
    };
    let get = |key: &str| config.get(key).filter(|v| !v.is_empty()).map(str::to_owned);
    // git's `--gui` order: `merge.guitool`, then `merge.tool`.
    let named = get("merge.guitool").or_else(|| get("merge.tool"));
    let mut installed: Vec<Tool> = Vec::new();
    let configured = match &named {
        None => Configured::None,
        Some(name) if is_terminal(name) => Configured::Unusable {
            why: format!("{name} runs in a terminal"),
            terminal: true,
        },
        Some(name) if available(name) => Configured::Usable(name.clone()),
        Some(name) => {
            if let Some(tool) = found(name) {
                installed.push(tool);
            }
            Configured::Unusable {
                why: format!("{name} is not available"),
                terminal: false,
            }
        }
    };
    // In `--tool-help`'s order: git's own, then the user's, by name.
    let names = BUILT_IN.iter().flat_map(|t| t.names.iter().copied());
    for name in names.chain(user.iter().map(String::as_str)) {
        if !is_terminal(name) && available(name) && !installed.iter().any(|t| t.name == name) {
            installed.push(Tool {
                name: name.to_owned(),
                path: None,
            });
        }
    }
    for (name, _) in APP_PATHS {
        if !installed.iter().any(|t| t.name == *name)
            && let Some(tool) = found(name)
        {
            installed.push(tool);
        }
    }
    Detected {
        configured,
        installed,
    }
}

/// A GUI tool git has a script for in `mergetools/`: the names it goes by, and the programs
/// the script runs, the first one found on `PATH`. Terminal tools, and `kompare`, which
/// can't merge, aren't here.
struct BuiltIn {
    names: &'static [&'static str],
    programs: &'static [&'static str],
    /// On Windows, git looks for the programs in `<Program Files>\<folder>` too.
    program_files: Option<&'static str>,
}

/// A tool that git finds on `PATH` only.
const fn built_in(names: &'static [&'static str], programs: &'static [&'static str]) -> BuiltIn {
    BuiltIn {
        names,
        programs,
        program_files: None,
    }
}

/// git's GUI tools, by name, as of git 2.53.
const BUILT_IN: &[BuiltIn] = &[
    built_in(&["araxis"], &["compare"]),
    built_in(&["bc", "bc3", "bc4"], &["bcomp", "bcompare"]),
    built_in(&["codecompare"], &["CodeMerge"]),
    built_in(&["deltawalker"], &["DeltaWalker"]),
    built_in(&["diffmerge"], &["diffmerge"]),
    built_in(&["diffuse"], &["diffuse"]),
    built_in(&["ecmerge"], &["ecmerge"]),
    BuiltIn {
        program_files: Some("ExamDiff Pro"),
        ..built_in(&["examdiff"], &["ExamDiff.com"])
    },
    built_in(&["guiffy"], &["guiffy"]),
    built_in(
        &["gvimdiff", "gvimdiff1", "gvimdiff2", "gvimdiff3"],
        &["gvim"],
    ),
    BuiltIn {
        program_files: Some("Kdiff3"),
        ..built_in(&["kdiff3"], &["kdiff3"])
    },
    built_in(&["meld"], &["meld"]),
    built_in(&["opendiff"], &["opendiff"]),
    built_in(&["p4merge"], &["p4merge"]),
    built_in(&["smerge"], &["smerge"]),
    built_in(&["tkdiff"], &["tkdiff"]),
    built_in(&["tortoisemerge"], &["tortoisegitmerge", "tortoisemerge"]),
    built_in(&["vscode"], &["code"]),
    BuiltIn {
        program_files: Some("WinMerge"),
        ..built_in(&["winmerge"], &["WinMergeU.exe"])
    },
    built_in(&["xxdiff"], &["xxdiff"]),
];

/// Where git's `sh` finds programs: in the folders of `PATH`, and for some tools on Windows,
/// in the Program Files folders.
struct Lookup<F> {
    path: Vec<PathBuf>,
    program_files: Vec<PathBuf>,
    is_file: F,
}

impl<F: Fn(&Path) -> bool> Lookup<F> {
    /// `program` as `sh` finds it: a path as it is, a name in a folder of `PATH`.
    fn finds(&self, program: &str) -> bool {
        let program = Path::new(program);
        if program.components().count() > 1 {
            return self.is_program(program);
        }
        self.path
            .iter()
            .any(|dir| self.is_program(&dir.join(program)))
    }

    /// Any of `tool`'s programs on `PATH`, or in its Program Files folder.
    fn finds_built_in(&self, tool: &BuiltIn) -> bool {
        tool.programs.iter().any(|p| self.finds(p))
            || tool.program_files.is_some_and(|folder| {
                self.program_files.iter().any(|dir| {
                    tool.programs
                        .iter()
                        .any(|p| self.is_program(&dir.join(folder).join(p)))
                })
            })
    }

    /// `path`, or on Windows `path.exe`, which `sh` runs for it there.
    fn is_program(&self, path: &Path) -> bool {
        (self.is_file)(path) || (cfg!(windows) && (self.is_file)(&with_exe(path)))
    }
}

/// `path` with `.exe` added, as Windows names a program.
fn with_exe(path: &Path) -> PathBuf {
    let mut path = path.as_os_str().to_owned();
    path.push(".exe");
    PathBuf::from(path)
}

/// The Program Files folders, as git's `mergetool_find_win32_cmd` takes them from the
/// environment; none outside Windows.
fn program_files() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
        .iter()
        .filter_map(std::env::var_os)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

/// git's built-ins that a per-user install hides from git's own lookup, by App Paths entry.
const APP_PATHS: &[(&str, &str)] = &[("winmerge", "WinMergeU.exe")];

/// Where Windows' App Paths registry says `name`'s program is.
fn app_paths(name: &str) -> Option<PathBuf> {
    let exe = APP_PATHS.iter().find(|(n, _)| *n == name)?.1;
    app_paths_entry(exe)
}

#[cfg(windows)]
fn app_paths_entry(exe: &str) -> Option<PathBuf> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
        .into_iter()
        .find_map(|hive| {
            let key = RegKey::predef(hive)
                .open_subkey(format!(
                    r"Software\Microsoft\Windows\CurrentVersion\App Paths\{exe}"
                ))
                .ok()?;
            // The key's default value.
            let value: String = key.get_value("").ok()?;
            let path = PathBuf::from(value.trim().trim_matches('"'));
            path.is_file().then_some(path)
        })
}

#[cfg(not(windows))]
fn app_paths_entry(_exe: &str) -> Option<PathBuf> {
    None
}

/// The `git config --global` commands that remember `tool` for good: `merge.guitool` when a
/// terminal tool is configured (the user's terminal workflow stays), else `merge.tool`; and
/// the path App Paths found it at.
pub fn remember_commands(tool: &Tool, terminal: bool) -> Vec<Vec<String>> {
    let key = if terminal {
        "merge.guitool"
    } else {
        "merge.tool"
    };
    let mut commands = vec![vec![
        "config".to_owned(),
        "--global".to_owned(),
        key.to_owned(),
        tool.name.clone(),
    ]];
    if let Some(path) = &tool.path {
        commands.push(vec![
            "config".to_owned(),
            "--global".to_owned(),
            format!("mergetool.{}.path", tool.name),
            path.to_string_lossy().into_owned(),
        ]);
    }
    commands
}

/// Remembers `tool` in the user's global git config.
pub fn remember(git: &Git, tool: &Tool, terminal: bool) -> Result<(), GitError> {
    for args in remember_commands(tool, terminal) {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        git.run(&args)?;
    }
    Ok(())
}

/// The arguments that open `path` in `tool`, without the leading `git`.
pub fn open_args(tool: &Tool, path: &str) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(p) = &tool.path {
        args.push("-c".to_owned());
        args.push(format!(
            "mergetool.{}.path={}",
            tool.name,
            p.to_string_lossy()
        ));
    }
    args.extend([
        "mergetool".to_owned(),
        "--no-prompt".to_owned(),
        format!("--tool={}", tool.name),
        "--".to_owned(),
        path.to_owned(),
    ]);
    args
}

/// Starts `git mergetool` on `path` in the worktree at `root`, and leaves it: no output is
/// read (nothing to break when parterre closes first), nothing waits for it. git stages the
/// file when the tool is done; the index shows it.
pub fn open(root: &Path, tool: &Tool, path: &str) -> Result<(), GitError> {
    let mut child = Git::new(root)
        .operation_command(&open_args(tool, path))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(GitError::Spawn)?;
    // Only so the finished process doesn't linger as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashSet;

    use crate::git::parse_config;

    /// `decide` with `settings` (`key=value` each) and only `files` there, `bin` on `PATH`
    /// and `pf` the Program Files folder; App Paths names `C:/W/WinMergeU.exe`
    /// for WinMerge, if it is among `files`.
    fn decide_with(settings: &[&str], files: &[&str]) -> Detected {
        let config: Vec<u8> = settings
            .iter()
            .flat_map(|s| format!("{}\0", s.replacen('=', "\n", 1)).into_bytes())
            .collect();
        let files: HashSet<PathBuf> = files.iter().map(PathBuf::from).collect();
        let lookup = Lookup {
            path: vec![PathBuf::from("bin")],
            program_files: vec![PathBuf::from("pf")],
            is_file: |p: &Path| files.contains(p),
        };
        decide(&parse_config(&config), &lookup, |name| {
            (name == "winmerge" && files.contains(Path::new("C:/W/WinMergeU.exe")))
                .then(|| PathBuf::from("C:/W/WinMergeU.exe"))
        })
    }

    fn names(d: &Detected) -> Vec<&str> {
        d.installed.iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn git_s_tools_are_installed_by_their_programs_on_path() {
        let meld = Path::new("bin").join("meld");
        let code = Path::new("bin").join("code");
        let d = decide_with(
            &["merge.tool=meld"],
            &[meld.to_str().unwrap(), code.to_str().unwrap()],
        );
        assert_eq!(d.configured, Configured::Usable("meld".into()));
        assert_eq!(names(&d), ["meld", "vscode"]);
        // Every name a tool goes by, as `--tool-help` lists them.
        let bcompare = Path::new("bin").join("bcompare");
        let d = decide_with(&["merge.tool=bc4"], &[bcompare.to_str().unwrap()]);
        assert_eq!(d.configured, Configured::Usable("bc4".into()));
        assert_eq!(names(&d), ["bc", "bc3", "bc4"]);
    }

    #[test]
    fn some_are_found_in_program_files_too() {
        let kdiff3 = Path::new("pf").join("Kdiff3").join("kdiff3");
        let d = decide_with(&["merge.tool=kdiff3"], &[kdiff3.to_str().unwrap()]);
        assert_eq!(d.configured, Configured::Usable("kdiff3".into()));
        // meld isn't looked for there.
        let meld = Path::new("pf").join("Meld").join("meld");
        assert!(
            decide_with(&[], &[meld.to_str().unwrap()])
                .installed
                .is_empty()
        );
    }

    #[test]
    fn a_configured_path_is_the_one_program_looked_for() {
        let mine = Path::new("opt").join("meld");
        let mine = mine.to_str().unwrap();
        let usable = decide_with(
            &["merge.tool=meld", &format!("mergetool.meld.path={mine}")],
            &[mine],
        );
        assert_eq!(usable.configured, Configured::Usable("meld".into()));
        let on_path = Path::new("bin").join("meld");
        let missing = decide_with(
            &["merge.tool=meld", "mergetool.meld.path=/nowhere/meld"],
            &[on_path.to_str().unwrap()],
        );
        assert_eq!(
            missing.configured,
            Configured::Unusable {
                why: "meld is not available".into(),
                terminal: false
            }
        );
    }

    #[test]
    fn the_user_s_tools_follow_git_s_by_name_and_terminal_ones_are_left_out() {
        let tkdiff = Path::new("bin").join("tkdiff");
        let gvim = Path::new("bin").join("gvim");
        let d = decide_with(
            &[
                "merge.tool=vimdiff",
                "mergetool.zed.cmd=zed",
                "mergetool.alpha.cmd=a",
                "mergetool.vimdiff3.cmd=v",
            ],
            &[tkdiff.to_str().unwrap(), gvim.to_str().unwrap()],
        );
        assert_eq!(
            d.configured,
            Configured::Unusable {
                why: "vimdiff runs in a terminal".into(),
                terminal: true
            }
        );
        assert_eq!(
            names(&d),
            [
                "gvimdiff",
                "gvimdiff1",
                "gvimdiff2",
                "gvimdiff3",
                "tkdiff",
                "alpha",
                "zed"
            ]
        );
    }

    #[test]
    fn app_paths_offers_a_tool_git_does_not_find() {
        let d = decide_with(&["merge.tool=winmerge"], &["C:/W/WinMergeU.exe"]);
        assert_eq!(
            d.configured,
            Configured::Unusable {
                why: "winmerge is not available".into(),
                terminal: false
            }
        );
        assert_eq!(
            d.installed,
            [Tool {
                name: "winmerge".into(),
                path: Some(PathBuf::from("C:/W/WinMergeU.exe"))
            }]
        );
        // Found by git, it needs no path.
        let found = Path::new("pf").join("WinMerge").join("WinMergeU.exe");
        let d = decide_with(&[], &[found.to_str().unwrap()]);
        assert_eq!(d.configured, Configured::None);
        assert_eq!(
            d.installed,
            [Tool {
                name: "winmerge".into(),
                path: None
            }]
        );
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_a_program_is_found_by_its_exe() {
        let meld = Path::new("bin").join("meld.exe");
        let d = decide_with(&["merge.guitool=meld"], &[meld.to_str().unwrap()]);
        assert_eq!(d.configured, Configured::Usable("meld".into()));
    }

    #[test]
    fn terminal_tools_are_known_by_name() {
        for t in ["vimdiff", "vimdiff3", "nvimdiff1", "emerge"] {
            assert!(is_terminal(t), "{t}");
        }
        assert!(!is_terminal("meld"));
    }

    #[test]
    fn remembering_writes_the_gui_key_beside_a_terminal_tool() {
        let tool = Tool {
            name: "winmerge".into(),
            path: Some(PathBuf::from("C:/W/WinMergeU.exe")),
        };
        assert_eq!(
            remember_commands(&tool, true),
            vec![
                vec!["config", "--global", "merge.guitool", "winmerge"],
                vec![
                    "config",
                    "--global",
                    "mergetool.winmerge.path",
                    "C:/W/WinMergeU.exe"
                ],
            ]
        );
        let meld = Tool {
            name: "meld".into(),
            path: None,
        };
        assert_eq!(
            remember_commands(&meld, false),
            vec![vec!["config", "--global", "merge.tool", "meld"]]
        );
    }

    #[test]
    fn a_tool_opens_one_file_without_prompts() {
        let tool = Tool {
            name: "winmerge".into(),
            path: Some(PathBuf::from("W.exe")),
        };
        assert_eq!(
            open_args(&tool, "-a.txt"),
            [
                "-c",
                "mergetool.winmerge.path=W.exe",
                "mergetool",
                "--no-prompt",
                "--tool=winmerge",
                "--",
                "-a.txt"
            ]
        );
    }
}
