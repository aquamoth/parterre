//! The user's merge tool: the one git's config names, as `git mergetool` picks it, or one
//! installed that the user picks when none is usable. Parterre only starts it, through
//! `git mergetool`, and leaves it be: the tool does the resolving, and git stages the file.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::git::{Git, GitError};

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

/// Asks git (`--tool-help` takes about 0.3 s: call off the UI thread).
pub fn detect(git: &Git) -> Result<Detected, GitError> {
    let get = |key: &str| -> Result<Option<String>, GitError> {
        Ok(git
            .query(&["config", "--get", key])?
            .filter(|v| !v.is_empty()))
    };
    let tool = get("merge.tool")?;
    let gui = get("merge.guitool")?;
    let help = git.run(&["mergetool", "--tool-help"])?;
    let available = parse_tool_help(&help);
    let found = |name: &str| {
        app_paths(name).map(|path| Tool {
            name: name.to_owned(),
            path: Some(path),
        })
    };
    // git's `--gui` order: `merge.guitool`, then `merge.tool`.
    let named = gui.or(tool);
    let mut installed: Vec<Tool> = Vec::new();
    let configured = match &named {
        None => Configured::None,
        Some(name) if is_terminal(name) => Configured::Unusable {
            why: format!("{name} runs in a terminal"),
            terminal: true,
        },
        Some(name) if available.iter().any(|t| t == name) => Configured::Usable(name.clone()),
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
    for name in available.into_iter().filter(|n| !is_terminal(n)) {
        if !installed.iter().any(|t| t.name == name) {
            installed.push(Tool { name, path: None });
        }
    }
    for (name, _) in APP_PATHS {
        if !installed.iter().any(|t| t.name == *name)
            && let Some(tool) = found(name)
        {
            installed.push(tool);
        }
    }
    Ok(Detected {
        configured,
        installed,
    })
}

/// The tools `git mergetool --tool-help` says are available: the built-ins it found and the
/// user's own `mergetool.<name>.cmd`, indented under "may be set to one of the following:"
/// (a blank line before `user-defined:`). 2.34 prints a name per line, 2.37 a description
/// after it; user-defined ones come out as `<name>.cmd <command>`.
pub fn parse_tool_help(help: &str) -> Vec<String> {
    let mut tools = Vec::new();
    let mut lines = help.lines();
    if !lines.any(|l| l.contains("may be set to one of the following")) {
        return tools;
    }
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            // "The following tools are valid, but not currently available:"
            break;
        }
        if line.trim_end().ends_with(':') {
            // `user-defined:`
            continue;
        }
        let Some(word) = line.split_whitespace().next() else {
            continue;
        };
        let name = word.strip_suffix(".cmd").unwrap_or(word);
        if !tools.iter().any(|t| t == name) {
            tools.push(name.to_owned());
        }
    }
    tools
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
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    ["HKCU", "HKLM"].iter().find_map(|hive| {
        let key = format!(r"{hive}\Software\Microsoft\Windows\CurrentVersion\App Paths\{exe}");
        let out = std::process::Command::new("reg")
            .args(["query", &key, "/ve"])
            .stdin(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        let path = parse_reg_default(&String::from_utf8_lossy(&out.stdout))?;
        path.is_file().then_some(path)
    })
}

#[cfg(not(windows))]
fn app_paths_entry(_exe: &str) -> Option<PathBuf> {
    None
}

/// The default value in `reg query <key> /ve` output: `    (Default)    REG_SZ    C:\…`.
pub fn parse_reg_default(out: &str) -> Option<PathBuf> {
    out.lines().find_map(|line| {
        let (_, value) = line.split_once("REG_SZ")?;
        let value = value.trim().trim_matches('"');
        (!value.is_empty()).then(|| PathBuf::from(value))
    })
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

    #[test]
    fn tool_help_of_git_2_34_lists_names_only() {
        let help = "'git mergetool --tool=<tool>' may be set to one of the following:\n\
                    \t\tkdiff3\n\t\tmeld\n\t\tvimdiff\n\n\tuser-defined:\n\t\tmine.cmd true\n\n\
                    The following tools are valid, but not currently available:\n\t\tbc\n";
        assert_eq!(parse_tool_help(help), ["kdiff3", "meld", "vimdiff", "mine"]);
    }

    /// As git 2.43 printed it.
    #[test]
    fn tool_help_lists_descriptions_and_user_defined_tools() {
        let help = "'git mergetool --tool=<tool>' may be set to one of the following:\n\
                    \t\tmeld             Use Meld (requires a graphical session) with optional `auto merge`\n\
                    \n\
                    \tuser-defined:\n\
                    \t\tfake.cmd /tmp/fake.sh \"$BASE\" \"$LOCAL\"\n\
                    \n\
                    The following tools are valid, but not currently available:\n\
                    \t\tbc               Use Beyond Compare (requires a graphical session)\n\
                    \n\
                    Some of the tools listed above only work in a windowed\n";
        assert_eq!(parse_tool_help(help), ["meld", "fake"]);
    }

    #[test]
    fn tool_help_with_only_user_defined_tools() {
        let help = "'git mergetool --tool=<tool>' may be set to one of the following:\n\
                    \tuser-defined:\n\t\tmine.cmd true\n\n\
                    The following tools are valid, but not currently available:\n\t\tbc\n";
        assert_eq!(parse_tool_help(help), ["mine"]);
    }

    #[test]
    fn tool_help_without_tools_lists_none() {
        let help = "No suitable tool for 'git mergetool --tool=<tool>' found.\n\n\
                    The following tools are valid, but not currently available:\n\t\tbc\n";
        assert!(parse_tool_help(help).is_empty());
    }

    #[test]
    fn terminal_tools_are_known_by_name() {
        for t in ["vimdiff", "vimdiff3", "nvimdiff1", "emerge"] {
            assert!(is_terminal(t), "{t}");
        }
        assert!(!is_terminal("meld"));
    }

    #[test]
    fn reg_query_gives_the_default_value() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\App Paths\\WinMergeU.exe\r\n    (Default)    REG_SZ    C:\\Users\\a\\AppData\\Local\\Programs\\WinMerge\\WinMergeU.exe\r\n\r\n";
        assert_eq!(
            parse_reg_default(out),
            Some(PathBuf::from(
                r"C:\Users\a\AppData\Local\Programs\WinMerge\WinMergeU.exe"
            ))
        );
        assert_eq!(parse_reg_default("ERROR: not found"), None);
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
