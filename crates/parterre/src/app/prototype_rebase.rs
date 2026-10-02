//! PROTOTYPE — throwaway. Rebasing the open worktree's branch (#184), with real git:
//! *Rebase main onto X* in the graph's node menu and *Rebase main onto abc1234* in the log's row
//! menu, the confirmation, and the banner for a worktree an operation in progress has stuck.
//! The demo repository is made by `prototype_rebase_demo.sh`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, mpsc};

use eframe::egui::{self, Color32, RichText, Ui, ViewportId};
use parterre_core::branches::{Catalog, Report, Step, command_text};
use parterre_core::log::is_ancestor;
use parterre_core::{Oid, RefKind, Repo};

use super::branches::Tool;
use crate::{dialogs, menu};

/// The operation in progress that has stuck the open worktree, such as "a rebase".
pub fn stuck(catalog: &Catalog) -> Option<&'static str> {
    catalog
        .worktrees
        .iter()
        .find(|w| w.open)
        .and_then(|w| w.in_progress)
}

/// Why an item is greyed out while the open worktree is stuck.
pub fn stuck_reason(what: &str) -> String {
    format!("{} is in progress in this worktree", capitalized(what))
}

fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// Runs git in `dir`, as an operation does: the user's locale, no prompts, no editor.
fn git(dir: &Path, args: &[&str]) -> (bool, String) {
    match Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_EDITOR", ":")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
    {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            (out.status.success(), text.trim().to_owned())
        }
        Err(e) => (false, e.to_string()),
    }
}

fn query(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

fn conflicted(root: &Path) -> Vec<String> {
    query(root, &["diff", "--name-only", "--diff-filter=U"])
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The rebase's administrative folder, while one is in progress.
fn rebase_dir(root: &Path) -> Option<PathBuf> {
    ["rebase-merge", "rebase-apply"]
        .into_iter()
        .find_map(|name| {
            let path = root.join(query(root, &["rev-parse", "--git-path", name]));
            path.is_dir().then_some(path)
        })
}

#[derive(Default)]
struct State {
    confirm: Option<Confirm>,
    /// For screenshots: press Rebase as soon as the confirmation shows.
    autorun: bool,
    running: Option<mpsc::Receiver<Done>>,
    /// The banner, read once per loaded repository.
    banner: Option<(usize, Option<Banner>)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

struct Confirm {
    repo: Arc<Repo>,
    root: PathBuf,
    branch: String,
    onto: Oid,
    onto_name: String,
    replays: usize,
    drops: usize,
    merges: usize,
    dirty: bool,
    /// `rebase.autoStash` is set.
    config_stash: bool,
    stash: bool,
    opener: ViewportId,
    fresh: bool,
}

impl Confirm {
    fn load(
        repo: Arc<Repo>,
        catalog: &Catalog,
        onto: Oid,
        onto_name: String,
        opener: ViewportId,
    ) -> Option<Self> {
        let root = catalog.root.clone();
        let branch = catalog.current.clone()?;
        let range = format!("{}...HEAD", onto.to_hex());
        let marks = query(
            &root,
            &[
                "rev-list",
                "--right-only",
                "--cherry-mark",
                "--no-merges",
                &range,
            ],
        );
        let replays = marks.lines().filter(|l| l.starts_with('+')).count();
        let drops = marks.lines().filter(|l| l.starts_with('=')).count();
        let merges = query(
            &root,
            &[
                "rev-list",
                "--count",
                "--merges",
                &format!("{}..HEAD", onto.to_hex()),
            ],
        )
        .parse()
        .unwrap_or(0);
        let dirty = !query(&root, &["status", "--porcelain", "--untracked-files=no"]).is_empty();
        let config_stash = query(&root, &["config", "--bool", "rebase.autoStash"]) == "true";
        Some(Self {
            repo,
            root,
            branch,
            onto,
            onto_name,
            replays,
            drops,
            merges,
            dirty,
            config_stash,
            stash: config_stash,
            opener,
            fresh: true,
        })
    }

    fn args(&self) -> Vec<String> {
        let mut args = vec!["rebase".to_owned()];
        match (self.dirty, self.stash, self.config_stash) {
            (true, true, false) => args.push("--autostash".into()),
            (true, false, true) => args.push("--no-autostash".into()),
            _ => {}
        }
        args.push(self.onto_name.clone());
        args
    }

    fn blocked(&self) -> bool {
        self.dirty && !self.stash
    }
}

/// What a finished rebase left behind.
struct Done {
    root: PathBuf,
    title: String,
    args: Vec<String>,
    ok: bool,
    output: String,
    stuck: Option<usize>,
    /// The autostash couldn't be put back, and stays in the stash.
    stash_left: bool,
}

/// `git rebase X` would do something other than fast-forward: the branch has commits of its
/// own, and X has commits the branch hasn't.
fn rebases(repo: &Repo, catalog: &Catalog, onto: Oid) -> bool {
    let (Some(head), Some(onto)) = (
        catalog.head.and_then(|h| repo.lookup(&h)),
        repo.lookup(&onto),
    ) else {
        return false;
    };
    catalog.current.is_some()
        && catalog.has_working_tree
        && !is_ancestor(repo, onto, head)
        && !is_ancestor(repo, head, onto)
}

/// The branch being rebased, for the greyed-out item's label while stuck.
fn stuck_branch(catalog: &Catalog) -> String {
    rebase_dir(&catalog.root)
        .and_then(|dir| std::fs::read_to_string(dir.join("head-name")).ok())
        .map(|h| h.trim().trim_start_matches("refs/heads/").to_owned())
        .or_else(|| catalog.current.clone())
        .unwrap_or_else(|| "HEAD".into())
}

fn item(ui: &mut Ui, label: String, blocked: Option<&String>, busy: bool) -> bool {
    let enabled = blocked.is_none() && !busy;
    let response = ui.add_enabled(enabled, egui::Button::new(label));
    let response = match blocked {
        Some(reason) => response.on_disabled_hover_text(reason),
        None => response.on_disabled_hover_text("A Git operation is running"),
    };
    if response.clicked() {
        ui.close();
    }
    response.clicked()
}

/// *Rebase main onto X* for each branch on the node, in a submenu when there are several.
pub fn graph_menu(
    ui: &mut Ui,
    repo: &Arc<Repo>,
    commit: Oid,
    catalog: Option<&Catalog>,
    busy: bool,
) {
    let Some(catalog) = catalog else { return };
    let mut names: Vec<String> = repo
        .refs
        .iter()
        .filter(|r| repo.commit(r.target).oid == commit)
        .filter(|r| {
            matches!(r.kind, RefKind::LocalBranch | RefKind::RemoteBranch)
                && !r.name.ends_with("/HEAD")
        })
        .map(|r| r.name.clone())
        .filter(|name| catalog.current.as_deref() != Some(name))
        .collect();
    names.sort();
    names.dedup();
    let blocked = stuck(catalog).map(stuck_reason);
    let branch = match &blocked {
        Some(_) => stuck_branch(catalog),
        None => {
            if names.is_empty() || !rebases(repo, catalog, commit) {
                return;
            }
            catalog.current.clone().unwrap_or_default()
        }
    };
    if names.is_empty() {
        return;
    }
    menu::separator(ui);
    let mut chosen = None;
    match names.as_slice() {
        [name] => {
            if item(
                ui,
                format!("Rebase {branch} onto {name}"),
                blocked.as_ref(),
                busy,
            ) {
                chosen = Some(name.clone());
            }
        }
        _ if blocked.is_some() => {
            ui.add_enabled(false, egui::Button::new(format!("Rebase {branch} onto")))
                .on_disabled_hover_text(blocked.clone().unwrap_or_default());
        }
        many => menu::plain_submenu(ui, &format!("Rebase {branch} onto"), |ui| {
            for name in many {
                if item(ui, name.clone(), blocked.as_ref(), busy) {
                    chosen = Some(name.clone());
                }
            }
        }),
    }
    if let Some(name) = chosen {
        open(repo, catalog, commit, name, ViewportId::ROOT);
    }
}

/// *Rebase main onto abc1234*, for any commit in the log.
pub fn log_menu(
    ui: &mut Ui,
    repo: &Arc<Repo>,
    commit: Oid,
    catalog: Option<&Catalog>,
    busy: bool,
    opener: ViewportId,
) {
    let Some(catalog) = catalog else { return };
    let short = commit.short(repo.abbrev_len.max(7));
    let blocked = stuck(catalog).map(stuck_reason);
    let branch = match &blocked {
        Some(_) => stuck_branch(catalog),
        None if rebases(repo, catalog, commit) => catalog.current.clone().unwrap_or_default(),
        None => return,
    };
    menu::separator(ui);
    if item(
        ui,
        format!("Rebase {branch} onto {short}"),
        blocked.as_ref(),
        busy,
    ) {
        open(repo, catalog, commit, commit.to_hex(), opener);
    }
}

/// For screenshots.
pub fn demo_open(repo: &Arc<Repo>, catalog: &Catalog, onto: Oid, name: String, run: bool) {
    open(repo, catalog, onto, name, ViewportId::ROOT);
    STATE.with_borrow_mut(|s| s.autorun = run);
}

fn open(repo: &Arc<Repo>, catalog: &Catalog, onto: Oid, name: String, opener: ViewportId) {
    let confirm = Confirm::load(repo.clone(), catalog, onto, name, opener);
    STATE.with_borrow_mut(|s| s.confirm = confirm);
}

/// The confirmation, and what a finished rebase reports. Call every frame.
pub fn show(ctx: &egui::Context, tool: &mut Tool) {
    STATE.with_borrow_mut(|state| {
        if let Some(rx) = &state.running
            && let Ok(done) = rx.try_recv()
        {
            state.running = None;
            state.banner = None;
            report(ctx, tool, done);
        }
        let Some(mut confirm) = state.confirm.take() else {
            return;
        };
        let busy = state.running.is_some() || tool.busy();
        let title = format!("Rebase {} onto {}", confirm.branch, short_name(&confirm));
        let mut log = false;
        let shown = dialogs::Dialog::new("prototype-rebase", &title)
            .opener(confirm.opener)
            .raise(confirm.fresh)
            .show(ctx, |ui| {
                dialogs::fields(ui, |ui| {
                    if let Some(ix) = confirm.repo.lookup(&confirm.onto) {
                        log = dialogs::commit_line(
                            ui,
                            confirm.repo.commit(ix),
                            confirm.repo.abbrev_len,
                        );
                    }
                    ui.add_space(4.0);
                    ui.label(format!("Replays {}", plural(confirm.replays, "commit")));
                    if confirm.drops > 0 {
                        ui.label(format!(
                            "Drops {} already in {}",
                            confirm.drops,
                            short_name(&confirm)
                        ));
                    }
                    if confirm.merges > 0 {
                        ui.label(format!("Flattens {}", plural(confirm.merges, "merge")));
                    }
                    if confirm.dirty {
                        ui.add_space(6.0);
                        ui.checkbox(&mut confirm.stash, "Stash changes")
                            .on_hover_text(
                                "Set your uncommitted changes aside first, and put them back \
                             afterwards (git rebase --autostash).",
                            );
                        if confirm.blocked() {
                            ui.colored_label(
                                ui.visuals().error_fg_color,
                                "Commit or stash your changes first.",
                            );
                        }
                    }
                });
                dialogs::command_box(ui, &[command_text(&confirm.args())]);
                dialogs::actions(ui, "Rebase", !confirm.blocked() && !busy, false, false)
            });
        confirm.fresh = false;
        if log {
            tool.log_request = Some((confirm.repo.clone(), vec![confirm.onto], false));
        }
        let answer = if std::mem::take(&mut state.autorun) {
            dialogs::Answer::Primary
        } else {
            shown.inner
        };
        match answer {
            dialogs::Answer::Primary => {
                state.running = Some(run(ctx, &confirm, title));
            }
            dialogs::Answer::Open if !shown.should_close() => state.confirm = Some(confirm),
            _ => {}
        }
    });
}

/// `origin/main`, or the short hash for a commit.
fn short_name(confirm: &Confirm) -> String {
    if confirm.onto_name == confirm.onto.to_hex() {
        confirm.onto.short(confirm.repo.abbrev_len.max(7))
    } else {
        confirm.onto_name.clone()
    }
}

fn run(ctx: &egui::Context, confirm: &Confirm, title: String) -> mpsc::Receiver<Done> {
    let (tx, rx) = mpsc::channel();
    let root = confirm.root.clone();
    let args = confirm.args();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let stashes = || query(&root, &["stash", "list"]).lines().count();
        let before = stashes();
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let (ok, output) = git(&root, &refs);
        let stuck = rebase_dir(&root).map(|_| conflicted(&root).len());
        let stash_left = stuck.is_none() && stashes() > before;
        let _ = tx.send(Done {
            root,
            title,
            args,
            ok,
            output,
            stuck,
            stash_left,
        });
        ctx.request_repaint();
    });
    rx
}

fn report(ctx: &egui::Context, tool: &mut Tool, done: Done) {
    let report = Report {
        steps: vec![Step {
            args: done.args,
            output: done.output.clone(),
            success: done.ok,
        }],
    };
    let path = done.root.clone();
    let (title, message, orange) = match (done.stuck, done.ok, done.stash_left) {
        (Some(0), ..) => ("Rebase stopped".to_owned(), None, true),
        (Some(n), ..) => (
            format!("Rebase stopped on conflicts in {}", plural(n, "file")),
            Some("Finish or abort it with git, or go to another worktree.".to_owned()),
            true,
        ),
        (None, true, true) => (
            done.title.replace("Rebase", "Rebased"),
            Some(
                "Putting your changes back conflicted, so git kept them in a stash entry. \
                 Recover them with git stash pop."
                    .to_owned(),
            ),
            true,
        ),
        (None, true, false) => (done.title.replace("Rebase", "Rebased"), None, false),
        (None, false, _) => (
            format!("{} failed", done.title),
            Some(
                done.output
                    .lines()
                    .rfind(|l| !l.trim().is_empty())
                    .unwrap_or("Git failed.")
                    .to_owned(),
            ),
            false,
        ),
    };
    tool.prototype_notice(ctx, path.clone(), title, report, message, orange);
    tool.reload = Some(path);
}

struct Banner {
    text: String,
    files: Vec<String>,
}

fn read_banner(repo: &Repo, catalog: &Catalog, what: &str) -> Banner {
    let root = &catalog.root;
    let files = conflicted(root);
    let mut text = match rebase_dir(root) {
        Some(dir) => {
            let read = |name: &str| {
                std::fs::read_to_string(dir.join(name))
                    .map(|s| s.trim().to_owned())
                    .unwrap_or_default()
            };
            let branch = read("head-name")
                .trim_start_matches("refs/heads/")
                .to_owned();
            // Git records only the commit, and says it the same way.
            let onto = read("onto");
            let onto = Oid::from_hex(&onto)
                .map(|oid| oid.short(repo.abbrev_len.max(7)))
                .unwrap_or(onto);
            let (at, of) = if dir.ends_with("rebase-merge") {
                (read("msgnum"), read("end"))
            } else {
                (read("next"), read("last"))
            };
            format!("Rebasing {branch} onto {onto} stopped at {at}/{of}")
        }
        None => format!("{} is in progress in this worktree", capitalized(what)),
    };
    if !files.is_empty() {
        text.push_str(&format!(": {}", plural(files.len(), "conflicted file")));
    }
    Banner { text, files }
}

/// The banner across the graph while the open worktree is stuck. Call before the central panel.
pub fn banner(ui: &mut Ui, repo: Option<&Arc<Repo>>, catalog: Option<&Catalog>) {
    let (Some(repo), Some(catalog)) = (repo, catalog) else {
        return;
    };
    let Some(what) = stuck(catalog) else {
        return;
    };
    let key = Arc::as_ptr(repo) as usize;
    let banner = STATE.with_borrow_mut(|s| {
        if s.banner.as_ref().is_none_or(|(k, _)| *k != key) {
            s.banner = Some((key, Some(read_banner(repo, catalog, what))));
        }
        s.banner
            .as_ref()
            .and_then(|(_, b)| b.as_ref())
            .map(|b| (b.text.clone(), b.files.clone()))
    });
    let Some((text, files)) = banner else { return };
    let (fill, text_color, stroke) = if ui.visuals().dark_mode {
        (
            Color32::from_rgb(75, 45, 10),
            Color32::from_rgb(255, 200, 130),
            Color32::from_rgb(240, 160, 60),
        )
    } else {
        (
            Color32::from_rgb(255, 232, 196),
            Color32::from_rgb(110, 55, 0),
            Color32::from_rgb(220, 130, 30),
        )
    };
    egui::Panel::top("prototype-rebase-banner")
        .frame(
            egui::Frame::new()
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, stroke))
                .inner_margin(egui::Margin::symmetric(12, 6)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let button = 130.0;
                ui.allocate_ui(egui::vec2(ui.available_width() - button, 0.0), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("⚠").color(text_color).strong());
                        let label = ui.label(RichText::new(&text).color(text_color).strong());
                        if !files.is_empty() {
                            label.on_hover_text(files.join("\n"));
                        }
                        ui.label(
                            RichText::new(
                                "Finish or abort it with git, or go to another worktree.",
                            )
                            .color(text_color),
                        );
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Open in terminal").clicked()
                        && let Err(e) = crate::file_manager::open_terminal(&catalog.root)
                    {
                        eprintln!("{e}");
                    }
                });
            });
        });
}
