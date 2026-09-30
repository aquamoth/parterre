//! PROTOTYPE — throwaway. The confirmations, warnings and errors before losing work, for
//! "Warnings before losing work: prototype" (aquamoth/parterre#147), built from the table in
//! "What counts as losing work" (#143). Nothing runs git to change anything: the dialogs read
//! the repository to say what would be lost, and their buttons only show what would run.
//!
//! Open a dialog from its menu item (the menus prototype, #144), or from the panel at the
//! bottom left, which lists every case in `prototype_warnings_demo.sh`'s repository.
//!
//! Round 2: variant B of round 1 (a count per kind; *Show in log* opens the lost commits in
//! the log window, files unfold), in the app's dialog look (as the pull-request error
//! dialog). Deleting a branch that loses nothing is one click, even when `-d` refuses.
//! *Cancel* has the focus; Enter and Esc both cancel.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui::{self, Color32, Key, RichText, Ui};
use parterre_core::glyphs::{Glyph, Part};
use parterre_core::log::LogQuery;
use parterre_core::{Oid, Repo};

/// A case from the table in #143.
#[derive(Clone, Debug)]
pub enum Case {
    /// Delete a local branch (short name).
    DeleteLocal(String),
    /// Delete a remote branch (`origin/x`).
    DeleteRemote(String),
    /// Delete a worktree: an index into `Repo::worktrees`.
    DeleteWorktree(usize),
    /// Switch the open worktree to a branch.
    Switch(String),
    /// Switch away from a detached HEAD: in that worktree, to that branch.
    SwitchFromDetached(usize, String),
    /// Abort the operation in progress in a worktree.
    Abort(usize),
    /// Force push a local branch to its upstream.
    ForcePush(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Level {
    OneClick,
    Confirm,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct Commit {
    oid: String,
    line: String,
}

#[derive(Clone, Debug)]
enum Items {
    Commits {
        list: Vec<Commit>,
        /// The log window's query for them.
        tip: String,
        except: Vec<String>,
    },
    Files(Vec<String>),
}

impl Items {
    fn len(&self) -> usize {
        match self {
            Items::Commits { list, .. } => list.len(),
            Items::Files(f) => f.len(),
        }
    }
}

#[derive(Clone, Debug)]
struct Section {
    /// "3 commits only on fix/typo".
    title: String,
    items: Items,
}

#[derive(Clone, Debug)]
struct Dialog {
    level: Level,
    title: String,
    text: Vec<String>,
    sections: Vec<Section>,
    commands: Vec<String>,
    ok: String,
    /// A second way out, such as *Create a worktree for X instead*.
    alt: Option<String>,
    /// *Also delete branch X*.
    checkbox: Option<String>,
}

struct Open {
    case: Case,
    also_delete: bool,
    shown: HashSet<usize>,
    first_frame: bool,
    /// Worked out once per case and tick of the checkbox.
    dialog: Option<(bool, Dialog)>,
}

struct State {
    open: Option<Open>,
    toast: Option<(String, Instant)>,
    log: Option<(String, Vec<String>)>,
    panel: bool,
}

thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State {
        open: None,
        toast: None,
        log: None,
        panel: true,
    }) };
}

pub fn start(case: Case) {
    STATE.with(|s| {
        s.borrow_mut().open = Some(Open {
            case,
            also_delete: false,
            shown: HashSet::new(),
            first_frame: true,
            dialog: None,
        })
    });
}

pub(crate) fn toast(text: impl Into<String>) {
    STATE.with(|s| s.borrow_mut().toast = Some((text.into(), Instant::now())));
}

/// The menus prototype's operations that belong here: true if `command` was taken over.
pub fn intercept(repo: &Repo, command: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    let case = match words.as_slice() {
        ["git", "branch", "-d", b] => Case::DeleteLocal(b.to_string()),
        ["git", "worktree", "remove", path] => {
            let Some(w) = repo
                .worktrees
                .iter()
                .position(|w| w.path == Path::new(path))
            else {
                return false;
            };
            Case::DeleteWorktree(w)
        }
        ["git", "switch", b] => Case::Switch(b.to_string()),
        ["git", "push", lease, "--force-if-includes", _remote, b]
            if lease.starts_with("--force-with-lease") =>
        {
            Case::ForcePush(b.to_string())
        }
        ["git", "push", _lease, remote, "--delete", b] => {
            Case::DeleteRemote(format!("{remote}/{b}"))
        }
        _ => return false,
    };
    start(case);
    true
}

/// Where the log window should open, if a dialog asked: the query's tip and the refs left out.
pub fn take_log_request(repo: &Repo) -> Option<LogQuery> {
    let (tip, except) = STATE.with(|s| s.borrow_mut().log.take())?;
    let ix = |hex: &str| repo.lookup(&Oid::from_hex(hex)?);
    let mut query = LogQuery::commit(ix(&tip)?);
    query.exclude = except.iter().filter_map(|h| ix(h)).collect();
    Some(query)
}

// ---------------------------------------------------------------------------------------------
// Reading the repository.

/// git, without the console window a GUI app on Windows would open for it: several seconds a
/// call where the default terminal is Windows Terminal (as `Git::command` does).
pub(crate) fn git_command() -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    cmd.stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn git_in(dir: &Path, args: &[&str]) -> (bool, String) {
    match crate::prototype_warnings::git_command()
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
    {
        Ok(o) => (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
        ),
        Err(_) => (false, String::new()),
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    git_in(dir, args).1
}

fn rev(dir: &Path, name: &str) -> Option<String> {
    let (ok, out) = git_in(
        dir,
        &["rev-parse", "--verify", "-q", &format!("{name}^{{commit}}")],
    );
    ok.then(|| out.trim().to_owned())
}

/// The commits reachable from `tip` and from nothing else: no ref other than `except_refs`, no
/// worktree's HEAD other than `except_worktree`'s.
fn lost_commits(
    repo: &Repo,
    tip: &str,
    except_refs: &[&str],
    except_worktree: Option<usize>,
) -> Items {
    let dir = repo.path.as_path();
    let mut others: Vec<String> = git(
        dir,
        &[
            "for-each-ref",
            "--format=%(objectname)%00%(refname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )
    .lines()
    .filter_map(|l| l.split_once('\0'))
    .filter(|(_, name)| !except_refs.contains(name) && !name.ends_with("/HEAD"))
    .map(|(oid, _)| oid.to_owned())
    .collect();
    for (k, w) in repo.worktrees.iter().enumerate() {
        if Some(k) != except_worktree
            && let Some(h) = w.head
        {
            others.push(repo.commit(h).oid.to_hex());
        }
    }
    others.sort();
    others.dedup();
    let mut args = vec![
        "log".to_owned(),
        "--format=%H%x00%h  %s  · %an, %ar".to_owned(),
        tip.to_owned(),
        "--not".to_owned(),
    ];
    args.extend(others.iter().cloned());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let list = git(dir, &args)
        .lines()
        .filter_map(|l| l.split_once('\0'))
        .map(|(oid, line)| Commit {
            oid: oid.to_owned(),
            line: line.to_owned(),
        })
        .collect();
    Items::Commits {
        list,
        tip: tip.to_owned(),
        except: others,
    }
}

/// Uncommitted changes to files that aren't ignored, as `git status --short` shows them.
fn changes(dir: &Path) -> Vec<String> {
    git(dir, &["status", "--porcelain=v1", "--untracked-files=all"])
        .lines()
        .map(str::to_owned)
        .collect()
}

fn commits_title(n: usize, where_: &str) -> String {
    match n {
        1 => format!("1 commit {where_}"),
        n => format!("{n} commits {where_}"),
    }
}

fn files_title(n: usize) -> String {
    match n {
        1 => "1 file with uncommitted changes".to_owned(),
        n => format!("{n} files with uncommitted changes"),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

// ---------------------------------------------------------------------------------------------
// The cases, as dialogs.

fn dialog_for(repo: &Repo, case: &Case, also_delete: bool) -> Dialog {
    let dir = repo.path.as_path();
    let mut d = Dialog {
        level: Level::Confirm,
        title: String::new(),
        text: Vec::new(),
        sections: Vec::new(),
        commands: Vec::new(),
        ok: String::new(),
        alt: None,
        checkbox: None,
    };
    match case {
        Case::DeleteLocal(b) => {
            let full = format!("refs/heads/{b}");
            let Some(tip) = rev(dir, &full) else {
                d.level = Level::Error;
                d.title = format!("{b} is gone");
                d.ok = "Close".into();
                return d;
            };
            let upstream = git(dir, &["for-each-ref", "--format=%(upstream)", &full]);
            let upstream = upstream.trim();
            let into = if upstream.is_empty() {
                "HEAD"
            } else {
                upstream
            };
            let (merged, _) = git_in(dir, &["merge-base", "--is-ancestor", &tip, into]);
            if merged {
                d.level = Level::OneClick;
                d.commands = vec![format!("git branch -d {b}")];
                return d;
            }
            let lost = lost_commits(repo, &tip, &[full.as_str()], None);
            d.commands = vec![format!("git branch -D {b}")];
            if lost.len() == 0 {
                // Nothing is lost: no dialog, even though -d refuses.
                d.level = Level::OneClick;
            } else {
                d.level = Level::Warn;
                d.title = format!(
                    "Delete branch {b} and lose {}?",
                    plural(lost.len(), "commit", "commits")
                );
                d.text.push(format!(
                    "No other branch, tag or worktree has {}.",
                    if lost.len() == 1 {
                        "this commit"
                    } else {
                        "these commits"
                    }
                ));
                d.sections.push(Section {
                    title: commits_title(lost.len(), &format!("only on {b}")),
                    items: lost,
                });
                d.ok = "Delete anyway".into();
            }
        }
        Case::DeleteRemote(name) => {
            let (remote, b) = name.split_once('/').unwrap_or(("origin", name));
            let full = format!("refs/remotes/{name}");
            let tip = rev(dir, &full).unwrap_or_default();
            d.commands = vec![format!(
                "git push --force-with-lease=refs/heads/{b}:{} {remote} --delete {b}",
                &tip[..tip.len().min(7)]
            )];
            // The prototype asks the remote; the real thing lets git's lease refuse.
            let now = git(dir, &["ls-remote", remote, &format!("refs/heads/{b}")]);
            let now = now.split_whitespace().next().unwrap_or_default();
            if !now.is_empty() && now != tip {
                d.level = Level::Error;
                d.title = format!("Can't delete {b} on {remote}");
                d.text.push(format!(
                    "{b} on {remote} has changed since the last fetch. Fetch, look at what's new, then delete it if you still want to."
                ));
                d.ok = "Close".into();
                d.alt = Some("Fetch".into());
                return d;
            }
            let lost = lost_commits(repo, &tip, &[full.as_str()], None);
            let trackers: Vec<String> = git(
                dir,
                &[
                    "for-each-ref",
                    "--format=%(refname:short)%00%(upstream)",
                    "refs/heads",
                ],
            )
            .lines()
            .filter_map(|l| l.split_once('\0'))
            .filter(|(_, up)| *up == full)
            .map(|(n, _)| n.to_owned())
            .collect();
            let default = git(
                dir,
                &["symbolic-ref", "-q", &format!("refs/remotes/{remote}/HEAD")],
            );
            d.text.push(format!(
                "Deletes {b} on {remote}, for everyone who uses {remote}."
            ));
            match trackers.as_slice() {
                [] => {}
                [one] => d.text.push(format!("{one} loses its upstream.")),
                many => d
                    .text
                    .push(format!("{} lose their upstream.", many.join(", "))),
            }
            if default.trim() == full {
                d.text
                    .push(format!("{b} is {remote}'s default branch ({remote}/HEAD)."));
            }
            if lost.len() == 0 {
                d.title = format!("Delete {b} on {remote}?");
                d.ok = format!("Delete on {remote}");
            } else {
                d.level = Level::Warn;
                d.title = format!(
                    "Delete {b} on {remote} and lose {}?",
                    plural(lost.len(), "commit", "commits")
                );
                d.sections.push(Section {
                    title: commits_title(lost.len(), &format!("only on {name}")),
                    items: lost,
                });
                d.ok = "Delete anyway".into();
            }
        }
        Case::DeleteWorktree(w) => {
            let wt = &repo.worktrees[*w];
            let path = wt.path.display().to_string();
            let branch = wt
                .branch
                .as_deref()
                .map(|b| b.trim_start_matches("refs/heads/").to_owned());
            if wt.missing {
                d.text.push(format!("Its folder {path} is already gone: deleted outside git. Only git's record of it goes."));
            } else {
                d.text
                    .push(format!("Deletes the folder {path} and everything in it."));
            }
            let files = if wt.missing {
                Vec::new()
            } else {
                changes(&wt.path)
            };
            if !files.is_empty() {
                d.sections.push(Section {
                    title: files_title(files.len()),
                    items: Items::Files(files),
                });
            }
            let head = wt.head.map(|h| repo.commit(h).oid.to_hex());
            if branch.is_none()
                && let Some(h) = &head
            {
                let lost = lost_commits(repo, h, &[], Some(*w));
                if lost.len() > 0 {
                    d.sections.push(Section {
                        title: commits_title(lost.len(), "on its detached HEAD, on no branch"),
                        items: lost,
                    });
                }
            }
            let mut commands = vec![format!(
                "git worktree remove {}{path}",
                if d.sections.is_empty() {
                    ""
                } else {
                    "--force "
                }
            )];
            if let Some(b) = &branch {
                d.checkbox = Some(format!("Also delete branch {b}"));
                if also_delete {
                    let full = format!("refs/heads/{b}");
                    let tip = rev(dir, &full).unwrap_or_default();
                    let lost = lost_commits(repo, &tip, &[full.as_str()], Some(*w));
                    if lost.len() > 0 {
                        d.sections.push(Section {
                            title: commits_title(lost.len(), &format!("only on {b}")),
                            items: lost,
                        });
                    }
                    commands.push(format!(
                        "git branch -d {b}   (then -D if git refuses, without asking again)"
                    ));
                }
            }
            d.commands = commands;
            let (files, commits) = d.sections.iter().fold((0, 0), |(f, c), s| match s.items {
                Items::Files(ref x) => (f + x.len(), c),
                Items::Commits { ref list, .. } => (f, c + list.len()),
            });
            let what = [
                (commits > 0).then(|| plural(commits, "commit", "commits")),
                (files > 0).then(|| plural(files, "changed file", "changed files")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" and ");
            if what.is_empty() {
                d.title = format!("Delete worktree {}?", wt.name());
                d.ok = "Delete".into();
            } else {
                d.level = Level::Warn;
                d.title = format!("Delete worktree {} and lose {what}?", wt.name());
                d.ok = "Delete anyway".into();
            }
        }
        Case::Switch(target) => {
            let full = format!("refs/heads/{target}");
            let tip = rev(dir, &full).unwrap_or_default();
            let differs: HashSet<String> = git(dir, &["diff", "--name-only", "HEAD", &tip])
                .lines()
                .map(str::to_owned)
                .collect();
            let blocking: Vec<String> = changes(dir)
                .into_iter()
                .filter(|l| {
                    let path = &l[3..];
                    if l.starts_with("??") {
                        git_in(dir, &["cat-file", "-e", &format!("{tip}:{path}")]).0
                    } else {
                        differs.contains(path)
                    }
                })
                .collect();
            d.commands = vec![format!("git switch {target}")];
            if blocking.is_empty() {
                d.level = Level::OneClick;
                return d;
            }
            d.level = Level::Error;
            d.title = format!("Can't switch to {target}");
            d.text.push(format!(
                "Switching would overwrite your changes to {}. Commit them first, or work on {target} in a worktree of its own.",
                if blocking.len() == 1 { "this file" } else { "these files" }
            ));
            d.sections.push(Section {
                title: plural(blocking.len(), "file in the way", "files in the way"),
                items: Items::Files(blocking),
            });
            d.ok = "Close".into();
            d.alt = Some(format!("Create a worktree for {target}…"));
        }
        Case::SwitchFromDetached(w, target) => {
            let wt = &repo.worktrees[*w];
            let head = wt
                .head
                .map(|h| repo.commit(h).oid.to_hex())
                .unwrap_or_default();
            let lost = lost_commits(repo, &head, &[], Some(*w));
            d.commands = vec![format!("git -C {} switch {target}", wt.path.display())];
            if lost.len() == 0 {
                d.level = Level::OneClick;
                return d;
            }
            d.level = Level::Warn;
            d.title = format!(
                "Switch to {target} and leave {} behind?",
                plural(lost.len(), "commit", "commits")
            );
            d.text.push(format!(
                "HEAD is detached in {}, and no branch, tag or worktree has {}. After the switch only HEAD's reflog finds {}, until it expires.",
                wt.name(),
                if lost.len() == 1 { "this commit" } else { "these commits" },
                if lost.len() == 1 { "it" } else { "them" },
            ));
            d.sections.push(Section {
                title: commits_title(lost.len(), "on no branch"),
                items: lost,
            });
            d.ok = "Switch anyway".into();
            d.alt = Some("Create a branch here first…".into());
        }
        Case::Abort(w) => {
            let wt = &repo.worktrees[*w];
            let gitdir = PathBuf::from(git(&wt.path, &["rev-parse", "--absolute-git-dir"]).trim());
            let state = gitdir.join("rebase-merge");
            let read = |f: &str| {
                std::fs::read_to_string(state.join(f))
                    .unwrap_or_default()
                    .trim()
                    .to_owned()
            };
            let branch = read("head-name")
                .trim_start_matches("refs/heads/")
                .to_owned();
            let orig = read("orig-head");
            let done = read("msgnum");
            let total = read("end");
            let back = git(&wt.path, &["log", "-1", "--format=%h %s", &orig]);
            d.title = format!("Abort the rebase of {branch}?");
            d.text.push(format!(
                "Discards the conflict resolutions and the commits replayed so far (stopped at {done} of {total}) in worktree {}.",
                wt.name()
            ));
            d.text.push(format!(
                "{branch} goes back to {}, as before the rebase.",
                back.trim()
            ));
            d.commands = vec![format!("git -C {} rebase --abort", wt.path.display())];
            d.ok = "Abort rebase".into();
        }
        Case::ForcePush(b) => {
            let full = format!("refs/heads/{b}");
            let upstream = git(dir, &["for-each-ref", "--format=%(upstream)", &full]);
            let upstream = upstream.trim().to_owned();
            let up_short = upstream.trim_start_matches("refs/remotes/").to_owned();
            let (remote, _) = up_short.split_once('/').unwrap_or(("origin", ""));
            // Upstream commits never on the branch (its reflog), and which of them it has a
            // copy of (cherry-picked).
            let mut args = vec!["rev-list".to_owned(), format!("{full}..{upstream}")];
            args.extend(
                git(dir, &["reflog", "show", "--format=%H", &full])
                    .lines()
                    .map(|h| format!("^{h}")),
            );
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let never: Vec<String> = git(dir, &args).lines().map(str::to_owned).collect();
            let copied: HashSet<String> = git(
                dir,
                &[
                    "rev-list",
                    "--right-only",
                    "--cherry-mark",
                    &format!("{full}...{upstream}"),
                ],
            )
            .lines()
            .filter_map(|l| l.strip_prefix('=').map(str::to_owned))
            .collect();
            let describe = |oids: &[&String]| -> Vec<Commit> {
                oids.iter()
                    .map(|o| Commit {
                        oid: (*o).clone(),
                        line: git(dir, &["log", "-1", "--format=%h  %s  · %an, %ar", o])
                            .trim()
                            .to_owned(),
                    })
                    .collect()
            };
            let tip = rev(dir, &upstream).unwrap_or_default();
            let lost: Vec<&String> = never.iter().filter(|o| !copied.contains(*o)).collect();
            if never.is_empty() {
                d.title = format!("Force push {b} to {up_short}?");
                d.text.push(format!("Replaces {up_short} with your {b}."));
                d.commands = vec![format!(
                    "git push --force-with-lease=refs/heads/{b} --force-if-includes {remote} {b}"
                )];
                d.ok = "Force push".into();
            } else if !lost.is_empty() {
                d.level = Level::Error;
                d.title = format!("Can't force push {b}");
                d.text.push(format!(
                    "{up_short} has {} that {} never on your {b}. A force push would drop {}, so git refuses it. Bring {} into {b} first (pull, or cherry-pick {}), then push.",
                    plural(lost.len(), "commit", "commits"),
                    if lost.len() == 1 { "was" } else { "were" },
                    if lost.len() == 1 { "it" } else { "them" },
                    if lost.len() == 1 { "it" } else { "them" },
                    if lost.len() == 1 { "it" } else { "them" },
                ));
                d.sections.push(Section {
                    title: commits_title(lost.len(), &format!("only on {up_short}")),
                    items: Items::Commits {
                        list: describe(&lost),
                        tip: tip.clone(),
                        except: vec![rev(dir, &full).unwrap_or_default()],
                    },
                });
                d.ok = "Close".into();
            } else {
                let all: Vec<&String> = never.iter().collect();
                d.title = format!("Force push {b} to {up_short}?");
                d.text.push(format!(
                    "{up_short} has {} that {} never on your {b}, but {b} has {} (cherry-picked), so nothing is lost.",
                    plural(all.len(), "commit", "commits"),
                    if all.len() == 1 { "was" } else { "were" },
                    if all.len() == 1 { "a copy of it" } else { "copies of them" },
                ));
                d.text.push(format!(
                    "git's usual guard (--force-if-includes) can't see copies, so this push names the commit it replaces instead: it still fails if {up_short} has moved on since."
                ));
                d.sections.push(Section {
                    title: commits_title(all.len(), &format!("on {up_short}, copied onto {b}")),
                    items: Items::Commits {
                        list: describe(&all),
                        tip: tip.clone(),
                        except: vec![rev(dir, &full).unwrap_or_default()],
                    },
                });
                d.commands = vec![format!(
                    "git push --force-with-lease=refs/heads/{b}:{} {remote} {b}",
                    &tip[..tip.len().min(7)]
                )];
                d.ok = "Force push".into();
            }
        }
    }
    d
}

// ---------------------------------------------------------------------------------------------
// Showing them.

const DANGER: Color32 = Color32::from_rgb(0xc0, 0x1c, 0x28);

const TRIANGLE: Glyph = &[
    Part::Path("M12 3.5 2.5 20h19Z"),
    Part::Path("M12 9.5v5"),
    Part::Path("M12 17.2v.6"),
];
const CROSS_CIRCLE: Glyph = &[
    Part::Circle {
        center: [12.0, 12.0],
        radius: 9.0,
        filled: false,
    },
    Part::Path("M15 9l-6 6M9 9l6 6"),
];
const QUESTION: Glyph = &[
    Part::Circle {
        center: [12.0, 12.0],
        radius: 9.0,
        filled: false,
    },
    Part::Path("M9.5 9.5C9.5 6.5 14.5 6.5 14.5 9.5C14.5 11.5 12 11.5 12 13.5"),
    Part::Path("M12 16.8v.6"),
];

/// The app's dialog look (as the pull-request error dialog): the popover style, a popup frame.
pub(crate) fn dialog_style(ctx: &egui::Context) -> (egui::Style, egui::Frame) {
    let mut style = (*ctx.global_style()).clone();
    crate::menu::popover_style(&mut style);
    let frame = egui::Frame::popup(&style)
        .inner_margin(egui::Margin::same(20))
        .corner_radius(12);
    (style, frame)
}

/// A red button for *…anyway*, in the shape of the app's primary button.
fn danger_button(ui: &mut Ui, text: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::TextStyle::Button.resolve(ui.style()),
        Color32::WHITE,
    );
    let size = egui::vec2((galley.size().x + 32.0).max(80.0), 30.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let fill = if response.is_pointer_button_down_on() {
        DANGER.gamma_multiply(0.8)
    } else if response.hovered() {
        DANGER.gamma_multiply(0.9)
    } else {
        DANGER
    };
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(8), fill.to_opaque());
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    response
}

/// The keyboard focus, drawn as the app would: a ring round the button.
pub(crate) fn focus_ring(ui: &Ui, response: &egui::Response) {
    if response.has_focus() {
        let accent = crate::widgets::tones(ui).accent;
        ui.painter().rect_stroke(
            response.rect.expand(2.0),
            egui::CornerRadius::same(10),
            egui::Stroke::new(2.0, accent),
            egui::StrokeKind::Outside,
        );
    }
}

/// The git commands, under a line and a heading that folds them away (for every dialog, and
/// remembered), smaller than the text, with a button copying them for a terminal.
pub(crate) fn command_box(ui: &mut Ui, commands: &[String]) {
    ui.separator();
    let id = egui::Id::new("prototype-git-commands-shown");
    let mut shown = ui.data_mut(|d| *d.get_persisted_mut_or(id, true));
    // The heading as tall as its text, close to the box.
    let row = ui.spacing().interact_size.y;
    ui.spacing_mut().interact_size.y = 16.0;
    let head = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(12.0), egui::Sense::hover());
            let glyph = if shown {
                parterre_core::glyphs::CHEVRON_DOWN
            } else {
                parterre_core::glyphs::CHEVRON_RIGHT
            };
            let weak = ui.visuals().weak_text_color();
            crate::widgets::paint_glyph(ui.painter(), rect, glyph, weak);
            ui.label(RichText::new("Git command").small().weak());
        })
        .response
        .interact(egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.spacing_mut().interact_size.y = row;
    if head.clicked() {
        shown = !shown;
        ui.data_mut(|d| d.insert_persisted(id, shown));
    }
    if !shown {
        return;
    }
    ui.add_space(-4.0);
    let t = crate::widgets::tones(ui);
    let text = commands.join("\n");
    egui::Frame::new()
        .fill(t.seg_bg)
        .corner_radius(8)
        .inner_margin(egui::Margin {
            left: 12,
            right: 4,
            top: 4,
            bottom: 6,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                let w = ui.available_width() - 28.0;
                ui.vertical(|ui| {
                    ui.set_width(w);
                    ui.add_space(4.0);
                    wrapped_commands(ui, commands, w);
                });
                // A check mark for a moment after a click.
                let copied_id = egui::Id::new("prototype-copied").with(&text);
                let now = ui.input(|i| i.time);
                let at: Option<f64> = ui.data(|d| d.get_temp(copied_id));
                let copied = at.is_some_and(|at| now - at < 1.5);
                if copied {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(300));
                }
                let done = Color32::from_rgb(0x2e, 0xa0, 0x43);
                if crate::widgets::copy_button(ui, copied, done)
                    .on_hover_text("Copy, to run in a terminal")
                    .clicked()
                {
                    ui.ctx().copy_text(text.clone());
                    ui.data_mut(|d| d.insert_temp(copied_id, now));
                }
            });
        });
}

/// A return arrow, where a command is broken.
const RETURN: Glyph = &[Part::Path("M19 5v9H6"), Part::Path("M10 10l-4 4 4 4")];

/// Each command broken where the room ends, not at its spaces, with a return arrow at each
/// break; space between the commands, so each can be told apart.
fn wrapped_commands(ui: &mut Ui, commands: &[String], width: f32) {
    const MARK: f32 = 13.0;
    let font = egui::FontId::monospace(11.5);
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let per_line = (((width - MARK) / char_w).floor() as usize).max(10);
    let weak = ui.visuals().weak_text_color();
    ui.spacing_mut().interact_size.y = 0.0;
    ui.spacing_mut().item_spacing.y = 0.0;
    for (k, command) in commands.iter().enumerate() {
        if k > 0 {
            ui.add_space(10.0);
        }
        let chars: Vec<char> = command.chars().collect();
        let lines: Vec<String> = chars.chunks(per_line).map(|l| l.iter().collect()).collect();
        for (i, line) in lines.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.add(egui::Label::new(RichText::new(line).font(font.clone())).extend());
                if i + 1 < lines.len() {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::Vec2::splat(11.0), egui::Sense::hover());
                    crate::widgets::paint_glyph(ui.painter(), rect, RETURN, weak);
                }
            });
        }
    }
}

/// Opens the log window on `tip` (a full hash).
pub(crate) fn show_in_log(tip: String) {
    STATE.with(|s| s.borrow_mut().log = Some((tip, Vec::new())));
}

/// A section: its count, and *Show in log* for commits or *Show* for files.
fn section_ui(ui: &mut Ui, k: usize, s: &Section, o: &mut Open) {
    let t = crate::widgets::tones(ui);
    egui::Frame::new()
        .fill(t.seg_bg)
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(&s.title).strong());
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| match &s.items {
                        Items::Commits { tip, except, .. } => {
                            if ui.link("Show in log").clicked() {
                                let q = (tip.clone(), except.clone());
                                STATE.with(|st| st.borrow_mut().log = Some(q));
                            }
                        }
                        Items::Files(_) => {
                            let shown = o.shown.contains(&k);
                            if ui.link(if shown { "Hide" } else { "Show" }).clicked() {
                                if shown {
                                    o.shown.remove(&k);
                                } else {
                                    o.shown.insert(k);
                                }
                            }
                        }
                    },
                );
            });
            if o.shown.contains(&k)
                && let Items::Files(files) = &s.items
            {
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .id_salt(("section", k))
                    .max_height(180.0)
                    .show(ui, |ui| {
                        for f in files {
                            ui.label(RichText::new(f).monospace().size(13.0));
                        }
                    });
            }
        });
}

/// The open dialog, the toast, and the panel of cases.
pub fn show(ctx: &egui::Context, repo: Option<&Repo>) {
    let Some(repo) = repo else { return };
    panel(ctx, repo);
    toast_ui(ctx);
    let Some(mut o) = STATE.with(|s| s.borrow_mut().open.take()) else {
        return;
    };
    let fresh = o
        .dialog
        .as_ref()
        .is_none_or(|(tick, _)| *tick != o.also_delete);
    if fresh {
        o.dialog = Some((o.also_delete, dialog_for(repo, &o.case, o.also_delete)));
    }
    let d = o.dialog.as_ref().unwrap().1.clone();
    if d.level == Level::OneClick {
        toast(format!("One click, no dialog: {}", d.commands.join("; ")));
        return;
    }
    let (style, frame) = dialog_style(ctx);
    let mut close = false;
    let modal = egui::Modal::new(egui::Id::new("prototype-warning"))
        .frame(frame)
        .backdrop_color(Color32::from_black_alpha(if style.visuals.dark_mode {
            90
        } else {
            40
        }))
        .show(ctx, |ui| {
            ui.set_style(style.clone());
            ui.set_width(460.0);
            ui.spacing_mut().item_spacing.y = 10.0;
            let t = crate::widgets::tones(ui);
            // The badge says the level: a question, a warning, an error.
            let (glyph, bg, fg) = match d.level {
                Level::Warn => (TRIANGLE, DANGER.gamma_multiply(0.15), DANGER),
                Level::Error => (CROSS_CIRCLE, DANGER.gamma_multiply(0.15), DANGER),
                _ => (QUESTION, t.on_bg, t.on_fg),
            };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (badge, _) =
                    ui.allocate_exact_size(egui::Vec2::splat(32.0), egui::Sense::hover());
                ui.painter().circle_filled(badge.center(), 16.0, bg);
                let icon = egui::Rect::from_center_size(badge.center(), egui::Vec2::splat(18.0));
                crate::widgets::paint_glyph(ui.painter(), icon, glyph, fg);
                ui.add(egui::Label::new(RichText::new(&d.title).size(16.0).strong()).wrap());
            });
            for text in &d.text {
                ui.label(text);
            }
            if let Some(label) = &d.checkbox {
                ui.checkbox(&mut o.also_delete, label.as_str());
            }
            for (k, s) in d.sections.iter().enumerate() {
                section_ui(ui, k, s, &mut o);
            }
            if !d.commands.is_empty() && d.level != Level::Error {
                command_box(ui, &d.commands);
            }
            ui.add_space(4.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let focused = if d.level == Level::Error {
                    let b = crate::widgets::primary_button(ui, &d.ok, 80.0);
                    if b.clicked() {
                        close = true;
                    }
                    b
                } else {
                    let ok = if d.level == Level::Warn {
                        danger_button(ui, &d.ok)
                    } else {
                        crate::widgets::primary_button(ui, &d.ok, 80.0)
                    };
                    if ok.clicked() {
                        toast(format!("Would run: {}", d.commands.join("; ")));
                        close = true;
                    }
                    let cancel = crate::widgets::text_button(ui, "Cancel");
                    if cancel.clicked() {
                        close = true;
                    }
                    cancel
                };
                if o.first_frame {
                    focused.request_focus();
                }
                focus_ring(ui, &focused);
                if let Some(alt) = &d.alt
                    && crate::widgets::text_button(ui, alt).clicked()
                {
                    match alt
                        .strip_prefix("Create a worktree for ")
                        .and_then(|b| b.strip_suffix('…'))
                    {
                        Some(b) => crate::prototype_add_worktree::start_for_branch(repo, b),
                        None => toast(format!("Would open: {alt}")),
                    }
                    close = true;
                }
            });
        });
    // Enter and Esc both cancel (or close an error), whatever has the focus.
    let keys = ctx.input(|i| i.key_pressed(Key::Escape) || i.key_pressed(Key::Enter));
    if keys && !o.first_frame {
        close = true;
    }
    o.first_frame = false;
    if !(close || modal.should_close()) {
        STATE.with(|s| s.borrow_mut().open = Some(o));
    }
}

fn toast_ui(ctx: &egui::Context) {
    let Some((text, at)) = STATE.with(|s| s.borrow().toast.clone()) else {
        return;
    };
    if at.elapsed().as_secs_f32() > 5.0 {
        STATE.with(|s| s.borrow_mut().toast = None);
        return;
    }
    ctx.request_repaint();
    egui::Area::new(egui::Id::new("prototype-warning-toast"))
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 60.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(RichText::new(text).monospace());
            });
        });
}

/// Every case of the demo repository, bottom left.
fn panel(ctx: &egui::Context, repo: &Repo) {
    if !STATE.with(|s| s.borrow().panel) {
        return;
    }
    let has = |full: &str| repo.refs.iter().any(|r| r.full_name == full);
    let wt = |name: &str| repo.worktrees.iter().position(|w| w.name() == name);
    egui::Window::new("PROTOTYPE: warnings")
        .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(10.0, -40.0))
        .resizable(false)
        .collapsible(true)
        .default_open(true)
        .show(ctx, |ui| {
            ui.label(
                RichText::new("Variant B. Nothing runs: buttons only say what would.")
                    .small()
                    .weak(),
            );
            ui.separator();
            let row = |ui: &mut Ui, label: &str, cases: Vec<(String, Case)>| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(label).strong());
                    for (text, case) in cases {
                        if ui.small_button(text).clicked() {
                            start(case);
                        }
                    }
                });
            };
            let local: Vec<(String, Case)> =
                ["feature/done", "old/release-copy", "fix/typo", "spike/many"]
                    .into_iter()
                    .filter(|b| has(&format!("refs/heads/{b}")))
                    .map(|b| (b.to_owned(), Case::DeleteLocal(b.to_owned())))
                    .collect();
            row(ui, "Delete branch", local);
            let remote: Vec<(String, Case)> = [
                "origin/feature/search",
                "origin/main",
                "origin/feature/remote-only",
                "origin/feature/stale",
            ]
            .into_iter()
            .filter(|b| has(&format!("refs/remotes/{b}")))
            .map(|b| (b.to_owned(), Case::DeleteRemote(b.to_owned())))
            .collect();
            row(ui, "Delete remote", remote);
            let worktrees: Vec<(String, Case)> = ["clean", "release", "experiment", "gone", "big"]
                .into_iter()
                .filter_map(|n| Some((n.to_owned(), Case::DeleteWorktree(wt(n)?))))
                .collect();
            row(ui, "Delete worktree", worktrees);
            let mut switch: Vec<(String, Case)> = ["feature/search", "fix/typo", "feature/notes"]
                .into_iter()
                .filter(|b| has(&format!("refs/heads/{b}")))
                .map(|b| (b.to_owned(), Case::Switch(b.to_owned())))
                .collect();
            if let Some(w) = wt("experiment") {
                switch.push((
                    "from experiment (detached) to main".to_owned(),
                    Case::SwitchFromDetached(w, "main".to_owned()),
                ));
            }
            row(ui, "Switch", switch);
            let push: Vec<(String, Case)> =
                ["feature/login", "feature/shared", "feature/shared-picked"]
                    .into_iter()
                    .filter(|b| has(&format!("refs/heads/{b}")))
                    .map(|b| (b.to_owned(), Case::ForcePush(b.to_owned())))
                    .collect();
            row(ui, "Force push", push);
            if let Some(w) = wt("conflict") {
                row(
                    ui,
                    "Abort",
                    vec![("rebase in conflict".to_owned(), Case::Abort(w))],
                );
            }
        });
}

/// For screenshots: `PARTERRE_WARNING=<kind>:<name>` opens that case once.
pub fn from_env(repo: &Repo) {
    thread_local!(static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    let Ok(spec) = std::env::var("PARTERRE_WARNING") else {
        return;
    };
    let Some((kind, name)) = spec.split_once(':') else {
        return;
    };
    let wt = |n: &str| repo.worktrees.iter().position(|w| w.name() == n);
    let case = match kind {
        "local" => Case::DeleteLocal(name.into()),
        "remote" => Case::DeleteRemote(name.into()),
        "worktree" => match wt(name) {
            Some(w) => Case::DeleteWorktree(w),
            None => return,
        },
        "worktree+branch" => match wt(name) {
            Some(w) => {
                start(Case::DeleteWorktree(w));
                STATE.with(|s| {
                    if let Some(o) = s.borrow_mut().open.as_mut() {
                        o.also_delete = true;
                    }
                });
                return;
            }
            None => return,
        },
        "switch" => Case::Switch(name.into()),
        "detached" => match wt("experiment") {
            Some(w) => Case::SwitchFromDetached(w, name.into()),
            None => return,
        },
        "push" => Case::ForcePush(name.into()),
        "abort" => match wt(name) {
            Some(w) => Case::Abort(w),
            None => return,
        },
        _ => return,
    };
    start(case);
}
