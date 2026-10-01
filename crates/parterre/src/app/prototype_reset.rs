//! PROTOTYPE — throwaway. Resetting the open worktree's branch from the log window, for
//! "Resetting a branch" (aquamoth/parterre#172). Three variants, switched with the pill at the
//! bottom of the log window (or `PARTERRE_RESET_VARIANT=A|B|C`):
//!
//! - **A, only when needed:** one click when nothing can be lost (no staged, modified or
//!   in-the-way files, no commits left behind), otherwise the dialog.
//! - **B, always a dialog:** calm when the chosen mode loses nothing, a warning when it does.
//! - **C, modes in the menu:** *Reset `<b>` to here ▸ Soft … Hard*, one click per mode when it
//!   loses nothing, the dialog for that mode otherwise, git's refusal greyed out.
//!
//! All five of git's modes. Lost means gone: not on disk, on a branch or in history. Nothing
//! runs git to change anything; a notification says what would have run. The demo repository
//! is `prototype_reset_demo.sh`. `PARTERRE_RESET_DEMO=<rev>[:<mode>]` opens the dialog at
//! startup, for `--screenshot`.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use eframe::egui::{self, Color32, Id, RichText, Ui, ViewportId, vec2};
use parterre_core::branches::Catalog;
use parterre_core::{Oid, Repo};

use crate::{dialogs, menu, widgets};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Variant {
    A,
    B,
    C,
}

impl Variant {
    const ALL: [Variant; 3] = [Variant::A, Variant::B, Variant::C];

    fn name(self) -> &'static str {
        match self {
            Variant::A => "A — only when needed",
            Variant::B => "B — always a dialog",
            Variant::C => "C — modes in the menu",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Soft,
    Mixed,
    Keep,
    Merge,
    Hard,
}

impl Mode {
    const ALL: [Mode; 5] = [Mode::Soft, Mode::Mixed, Mode::Keep, Mode::Merge, Mode::Hard];

    fn name(self) -> &'static str {
        match self {
            Mode::Soft => "Soft",
            Mode::Mixed => "Mixed",
            Mode::Keep => "Keep",
            Mode::Merge => "Merge",
            Mode::Hard => "Hard",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Mode::Soft => "--soft",
            Mode::Mixed => "--mixed",
            Mode::Keep => "--keep",
            Mode::Merge => "--merge",
            Mode::Hard => "--hard",
        }
    }

    /// What happens to the changes of the commits moved away from, and to yours.
    fn gist(self) -> &'static str {
        match self {
            Mode::Soft => "their changes staged, yours kept",
            Mode::Mixed => "their changes unstaged, yours kept unstaged",
            Mode::Keep => "their changes dropped, yours kept unstaged",
            Mode::Merge => "their changes dropped, your staged changes dropped",
            Mode::Hard => "their changes and yours dropped",
        }
    }

    fn parse(s: &str) -> Option<Mode> {
        Mode::ALL
            .into_iter()
            .find(|m| m.name().eq_ignore_ascii_case(s))
    }
}

/// What a reset of the open worktree's branch to `target` would meet.
#[derive(Clone, Debug)]
#[allow(dead_code)]
struct Facts {
    repo: Arc<Repo>,
    root: PathBuf,
    branch: String,
    head: Oid,
    target: Oid,
    in_progress: Option<&'static str>,
    staged: BTreeSet<String>,
    unstaged: BTreeSet<String>,
    /// Conflicted paths of an unfinished merge; also in `staged` and `unstaged`.
    unmerged: BTreeSet<String>,
    /// Untracked files at paths the target has.
    in_the_way: BTreeSet<String>,
    /// Paths that differ between HEAD and the target.
    changed: BTreeSet<String>,
    /// Commits only the branch reaches, which the reset leaves behind.
    commits: Vec<Oid>,
}

fn git(root: &Path, args: &[&str]) -> String {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn names(root: &Path, args: &[&str]) -> BTreeSet<String> {
    git(root, args)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

impl Facts {
    fn load(repo: &Arc<Repo>, catalog: &Catalog, target: Oid) -> Option<Facts> {
        let branch = catalog.current.clone()?;
        let head = catalog.head?;
        if head == target || !catalog.has_working_tree {
            return None;
        }
        let root = catalog.root.clone();
        let (h, t) = (head.to_hex(), target.to_hex());
        let untracked = names(&root, &["ls-files", "--others", "--exclude-standard", "-z"]);
        let target_files = names(&root, &["ls-tree", "-r", "--name-only", "-z", &t]);
        let exclude = format!("--exclude={branch}");
        let mut args = vec![
            "rev-list",
            &h,
            "--not",
            &exclude,
            "--branches",
            "--tags",
            "--remotes",
            &t,
        ];
        let others: Vec<String> = catalog
            .worktrees
            .iter()
            .filter(|w| !w.open)
            .filter_map(|w| w.head.map(|o| o.to_hex()))
            .collect();
        args.extend(others.iter().map(String::as_str));
        let commits = git(&root, &args)
            .lines()
            .filter_map(Oid::from_hex)
            .collect();
        Some(Facts {
            repo: repo.clone(),
            branch,
            head,
            target,
            in_progress: catalog
                .worktrees
                .iter()
                .find(|w| w.open)
                .and_then(|w| w.in_progress),
            staged: names(&root, &["diff", "--cached", "--name-only", "-z"]),
            unstaged: names(&root, &["diff", "--name-only", "-z"]),
            unmerged: names(&root, &["diff", "--name-only", "--diff-filter=U", "-z"]),
            in_the_way: untracked.intersection(&target_files).cloned().collect(),
            changed: names(&root, &["diff", "--name-only", "-z", &h, &t]),
            commits,
            root,
        })
    }

    fn short(&self) -> String {
        self.target.short(self.repo.abbrev_len.max(7))
    }

    /// Nothing at all could be lost, whichever mode: variant A's one click.
    fn clean(&self) -> bool {
        self.staged.is_empty()
            && self.unstaged.is_empty()
            && self.in_the_way.is_empty()
            && self.commits.is_empty()
            && self.in_progress.is_none()
    }

    fn command(&self, mode: Mode) -> String {
        format!("git reset {} {}", mode.flag(), self.short())
    }

    /// What `git reset <mode>` would do here, from git-reset(1) and tests on git 2.34 and 2.43.
    fn outcome(&self, mode: Mode) -> Outcome {
        let short = self.short();
        let merging = self.in_progress == Some("a merge");
        // A conflicted file has no staged version of its own to lose.
        let partly: Vec<String> = self
            .staged
            .intersection(&self.unstaged)
            .filter(|f| !self.unmerged.contains(*f))
            .cloned()
            .collect();
        let local: BTreeSet<String> = self.staged.union(&self.unstaged).cloned().collect();
        let not_uptodate = |p: &String| {
            format!(
                "error: Entry '{p}' not uptodate. Cannot merge.\nfatal: Could not reset index file to revision '{short}'."
            )
        };
        let untracked = |p: &String| {
            format!(
                "error: Untracked working tree file '{p}' would be overwritten by merge.\nfatal: Could not reset index file to revision '{short}'."
            )
        };
        let staged_version = |files: Vec<String>| {
            files
                .into_iter()
                .map(|f| (f, "its staged version"))
                .collect::<Vec<_>>()
        };
        let (refusal, files) = match mode {
            Mode::Soft => (
                merging.then(|| "fatal: Cannot do a soft reset in the middle of a merge.".into()),
                Vec::new(),
            ),
            Mode::Mixed => (None, staged_version(partly)),
            Mode::Keep => {
                let refusal = if merging {
                    Some("fatal: Cannot do a keep reset in the middle of a merge.".into())
                } else if let Some(p) = local.intersection(&self.changed).next() {
                    Some(not_uptodate(p))
                } else {
                    self.in_the_way.iter().next().map(untracked)
                };
                (refusal, staged_version(partly))
            }
            Mode::Merge => {
                let touched: BTreeSet<String> =
                    self.changed.union(&self.staged).cloned().collect();
                // Conflicts don't stop it: `reset --merge` is how a merge is aborted.
                let refusal = match self
                    .unstaged
                    .intersection(&touched)
                    .find(|f| !self.unmerged.contains(*f))
                {
                    Some(p) => Some(not_uptodate(p)),
                    None => self.in_the_way.iter().next().map(untracked),
                };
                let files = self
                    .staged
                    .iter()
                    .map(|f| {
                        let why = if self.unmerged.contains(f) {
                            "conflicted"
                        } else {
                            "staged changes"
                        };
                        (f.clone(), why)
                    })
                    .collect();
                (refusal, files)
            }
            Mode::Hard => {
                let mut files: Vec<(String, &'static str)> = local
                    .iter()
                    .map(|f| {
                        let why = match (self.staged.contains(f), self.unstaged.contains(f)) {
                            _ if self.unmerged.contains(f) => "conflicted",
                            (true, true) => "staged and modified",
                            (true, false) => "staged",
                            _ => "modified",
                        };
                        (f.clone(), why)
                    })
                    .collect();
                files.extend(
                    self.in_the_way
                        .iter()
                        .map(|f| (f.clone(), "untracked, overwritten")),
                );
                (None, files)
            }
        };
        Outcome {
            refusal,
            files,
            commits_kept: matches!(mode, Mode::Soft | Mode::Mixed),
        }
    }

    /// The dialog's first mode: Keep when it's calm, else the first one git allows that loses
    /// nothing, else the first one git allows.
    fn default_mode(&self) -> Mode {
        let calm = |m: Mode| self.outcome(m).level(self) == Level::Calm;
        let allowed = |m: &Mode| self.outcome(*m).refusal.is_none();
        if calm(Mode::Keep) {
            return Mode::Keep;
        }
        Mode::ALL
            .into_iter()
            .filter(allowed)
            .find(|&m| self.outcome(m).level(self) != Level::Loses)
            .or_else(|| Mode::ALL.into_iter().find(allowed))
            .unwrap_or(Mode::Keep)
    }
}

#[derive(Clone, Debug)]
struct Outcome {
    refusal: Option<String>,
    files: Vec<(String, &'static str)>,
    /// The commits left behind keep their changes in the files (Soft, Mixed).
    commits_kept: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Level {
    Calm,
    /// Commits leave every branch, but their changes stay in the files.
    Caution,
    Loses,
    Refused,
}

impl Outcome {
    fn level(&self, facts: &Facts) -> Level {
        if self.refusal.is_some() {
            Level::Refused
        } else if !self.files.is_empty() || !facts.commits.is_empty() && !self.commits_kept {
            Level::Loses
        } else if !facts.commits.is_empty() {
            Level::Caution
        } else {
            Level::Calm
        }
    }
}

#[derive(Debug)]
struct Open {
    facts: Facts,
    mode: Mode,
    /// Variant C: the mode was picked in the menu.
    fixed: bool,
    opener: ViewportId,
    fresh: bool,
    show_files: bool,
}

#[derive(Debug)]
struct Notice {
    title: String,
    command: String,
    at: f64,
}

#[derive(Debug, Default)]
struct State {
    variant: Option<Variant>,
    /// The facts for the row whose menu is open, and when they were read.
    cache: Option<(PathBuf, Oid, f64, Option<Facts>)>,
    open: Option<Open>,
    notices: Vec<Notice>,
    log: Option<(Arc<Repo>, Vec<Oid>)>,
    demo_done: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

fn variant(state: &mut State) -> Variant {
    *state.variant.get_or_insert_with(|| {
        match std::env::var("PARTERRE_RESET_VARIANT").as_deref() {
            Ok("B" | "b") => Variant::B,
            Ok("C" | "c") => Variant::C,
            _ => Variant::A,
        }
    })
}

fn notify(state: &mut State, ctx: &egui::Context, facts: &Facts, mode: Mode) {
    state.notices.push(Notice {
        title: format!("Reset {} to {}", facts.branch, facts.short()),
        command: facts.command(mode),
        at: ctx.input(|i| i.time),
    });
}

/// The reset item of a log row's menu: only for the open worktree's branch, never for a
/// detached HEAD (which includes a branch being rebased).
pub fn menu(ui: &mut Ui, repo: &Arc<Repo>, commit: Oid, catalog: Option<&Catalog>) {
    let Some(catalog) = catalog else { return };
    let Some(branch) = catalog.current.clone() else {
        return;
    };
    if catalog.head == Some(commit) || !catalog.has_working_tree {
        return;
    }
    STATE.with_borrow_mut(|state| {
        let now = ui.input(|i| i.time);
        let stale = !matches!(&state.cache,
            Some((root, oid, at, _)) if *root == catalog.root && *oid == commit && now - at < 2.0);
        if stale {
            let facts = Facts::load(repo, catalog, commit);
            state.cache = Some((catalog.root.clone(), commit, now, facts));
        }
        let Some((_, _, _, Some(facts))) = state.cache.clone() else {
            return;
        };
        let opener = ui.ctx().viewport_id();
        menu::separator(ui);
        let open = |state: &mut State, mode: Mode, fixed: bool| {
            state.open = Some(Open {
                facts: facts.clone(),
                mode,
                fixed,
                opener,
                fresh: true,
                show_files: false,
            });
        };
        match variant(state) {
            Variant::A => {
                let quick = facts.clean();
                let dots = if quick { "" } else { "…" };
                if ui.button(format!("Reset {branch} to here{dots}")).clicked() {
                    if quick {
                        notify(state, ui.ctx(), &facts, Mode::Keep);
                    } else {
                        open(state, facts.default_mode(), false);
                    }
                    ui.close();
                }
            }
            Variant::B => {
                if ui.button(format!("Reset {branch} to here…")).clicked() {
                    open(state, facts.default_mode(), false);
                    ui.close();
                }
            }
            Variant::C => {
                menu::plain_submenu(ui, &format!("Reset {branch} to here"), |ui| {
                    for mode in Mode::ALL {
                        let outcome = facts.outcome(mode);
                        let level = outcome.level(&facts);
                        let (label, hint) = match level {
                            Level::Calm => (mode.name().to_owned(), ""),
                            Level::Caution => (format!("{}…", mode.name()), ""),
                            Level::Loses => (format!("{}…", mode.name()), "loses work"),
                            Level::Refused => (mode.name().to_owned(), "refused"),
                        };
                        let button = egui::Button::new(label).shortcut_text(hint);
                        let mut response = ui
                            .add_enabled(level != Level::Refused, button)
                            .on_hover_text(mode.gist());
                        if let Some(refusal) = &outcome.refusal {
                            response = response
                                .on_disabled_hover_text(RichText::new(refusal).monospace());
                        }
                        if response.clicked() {
                            if level == Level::Calm {
                                notify(state, ui.ctx(), &facts, mode);
                            } else {
                                open(state, mode, true);
                            }
                            ui.close();
                        }
                    }
                });
            }
        }
    });
}

/// The variant switcher and the notifications, over the log window.
pub fn overlay(ctx: &egui::Context) {
    STATE.with_borrow_mut(|state| {
        let current = variant(state);
        egui::Area::new(Id::new("prototype-reset-switcher"))
            .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -10.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(30, 30, 36))
                    .corner_radius(16)
                    .inner_margin(egui::Margin::symmetric(10, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.visuals_mut().override_text_color = Some(Color32::WHITE);
                            let at = Variant::ALL.iter().position(|v| *v == current).unwrap_or(0);
                            let n = Variant::ALL.len();
                            if ui.small_button("◀").clicked() {
                                state.variant = Some(Variant::ALL[(at + n - 1) % n]);
                            }
                            ui.label(format!("Reset prototype: {}", current.name()));
                            if ui.small_button("▶").clicked() {
                                state.variant = Some(Variant::ALL[(at + 1) % n]);
                            }
                        });
                    });
            });
        let now = ctx.input(|i| i.time);
        state.notices.retain(|n| now - n.at < 5.0);
        if state.notices.is_empty() {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        egui::Area::new(Id::new("prototype-reset-notices"))
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-16.0, -48.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(360.0);
                let green = if ui.visuals().dark_mode {
                    Color32::from_rgb(75, 165, 105)
                } else {
                    Color32::from_rgb(35, 120, 65)
                };
                for n in &state.notices {
                    egui::Frame::popup(ui.style())
                        .stroke(egui::Stroke::new(1.0, green))
                        .show(ui, |ui| {
                            ui.label(RichText::new(&n.title).color(green));
                            ui.label(
                                RichText::new(format!("{}  (prototype: not run)", n.command))
                                    .monospace()
                                    .small()
                                    .weak(),
                            );
                        });
                }
            });
    });
}

/// Commits to show in the log window, from *Show in log*.
pub fn take_log_request() -> Option<(Arc<Repo>, Vec<Oid>)> {
    STATE.with_borrow_mut(|state| state.log.take())
}

/// The dialog, and the startup demo.
pub fn show(
    ctx: &egui::Context,
    repo: Option<&Arc<Repo>>,
    catalog: Option<&Catalog>,
    log: ViewportId,
) {
    STATE.with_borrow_mut(|state| {
        if !state.demo_done
            && let (Some(repo), Some(catalog)) = (repo, catalog)
        {
            state.demo_done = true;
            if let Ok(spec) = std::env::var("PARTERRE_RESET_DEMO") {
                let (rev, mode) = spec.split_once(':').unwrap_or((&spec, ""));
                let hex = git(&catalog.root, &["rev-parse", "--verify", rev]);
                if let Some(facts) =
                    Oid::from_hex(hex.trim()).and_then(|t| Facts::load(repo, catalog, t))
                {
                    let fixed = Mode::parse(mode);
                    state.open = Some(Open {
                        mode: fixed.unwrap_or_else(|| facts.default_mode()),
                        fixed: fixed.is_some(),
                        facts,
                        opener: log,
                        fresh: true,
                        show_files: true,
                    });
                }
            }
        }
        let Some(mut open) = state.open.take() else {
            return;
        };
        let answer = dialog(ctx, &mut open, &mut state.log);
        match answer {
            dialogs::Answer::Primary => notify(state, ctx, &open.facts, open.mode),
            dialogs::Answer::Cancel => {}
            dialogs::Answer::Open => {
                open.fresh = false;
                state.open = Some(open);
            }
        }
    });
}

const TRIANGLE: parterre_core::glyphs::Glyph = &[parterre_core::glyphs::Part::Path(
    "M12 3 2 21h20ZM12 9v5m0 3v1",
)];

fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

fn dialog(
    ctx: &egui::Context,
    open: &mut Open,
    log: &mut Option<(Arc<Repo>, Vec<Oid>)>,
) -> dialogs::Answer {
    let facts = &open.facts;
    let outcome = facts.outcome(open.mode);
    let level = outcome.level(facts);
    let files = plural(outcome.files.len(), "file");
    let commits = plural(facts.commits.len(), "commit");
    let title = if level == Level::Loses {
        let cost = match (outcome.files.is_empty(), outcome.commits_kept) {
            (false, false) if !facts.commits.is_empty() => format!("{files} and {commits}"),
            (false, _) => files.clone(),
            (true, _) => commits.clone(),
        };
        format!("Reset {} and lose {cost}?", facts.branch)
    } else if open.fixed {
        format!("Reset {} to {} ({})", facts.branch, facts.short(), open.mode.name())
    } else {
        format!("Reset {} to {}", facts.branch, facts.short())
    };
    let mut dialog = dialogs::Dialog::new("prototype-reset", &title)
        .opener(open.opener)
        .raise(open.fresh);
    if matches!(level, Level::Loses | Level::Refused) {
        dialog = dialog.icon(TRIANGLE, true);
    }
    let mut mode = open.mode;
    let show_files = &mut open.show_files;
    let fixed = open.fixed;
    let fresh = open.fresh;
    let mut show_log = false;
    let shown = dialog.show(ctx, |ui| {
        dialogs::fields(ui, |ui| {
            if let Some(ix) = facts.repo.lookup(&facts.target) {
                dialogs::commit_line(ui, facts.repo.commit(ix), facts.repo.abbrev_len);
            }
            if let Some(what) = facts.in_progress {
                ui.label(format!("{} is in progress.", capitalized(what)));
            }
            if !fixed {
                ui.add_space(4.0);
                for m in Mode::ALL {
                    let o = facts.outcome(m);
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut mode, m, m.name());
                        ui.label(RichText::new(m.gist()).weak().small());
                        match o.level(facts) {
                            Level::Refused => {
                                ui.label(RichText::new("refused").small().color(
                                    ui.visuals().weak_text_color().gamma_multiply(0.8),
                                ));
                            }
                            Level::Loses => {
                                ui.label(
                                    RichText::new("loses work")
                                        .small()
                                        .color(ui.visuals().error_fg_color),
                                );
                            }
                            _ => {}
                        }
                    });
                }
                ui.add_space(4.0);
            } else {
                ui.label(RichText::new(open_gist(mode)).weak());
            }
            let row = |ui: &mut Ui, content: &mut dyn FnMut(&mut Ui)| {
                egui::Frame::new()
                    .fill(widgets::tones(ui).seg_bg)
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| ui.horizontal(|ui| content(ui)));
            };
            if let Some(refusal) = &outcome.refusal {
                ui.label("Git refuses:");
                ui.label(
                    RichText::new(refusal)
                        .monospace()
                        .color(ui.visuals().error_fg_color),
                );
            } else {
                if !outcome.files.is_empty() {
                    row(ui, &mut |ui| {
                        ui.label(&files);
                        let label = if *show_files { "Hide" } else { "Show" };
                        if ui.link(label).clicked() {
                            *show_files = !*show_files;
                        }
                    });
                    if *show_files {
                        for (file, why) in &outcome.files {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(file).monospace());
                                ui.label(RichText::new(*why).weak().small());
                            });
                        }
                    }
                }
                if !facts.commits.is_empty() {
                    if outcome.commits_kept {
                        caution(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(format!(
                                    "{commits} will be on no branch; their changes stay in your files."
                                ));
                                show_log |= ui.link("Show in log").clicked();
                            });
                        });
                    } else {
                        ui.label(
                            "These commits are not reachable from any surviving branch, tag or worktree.",
                        );
                        row(ui, &mut |ui| {
                            ui.label(&commits);
                            show_log |= ui.link("Show in log").clicked();
                        });
                    }
                }
            }
        });
        dialogs::command_box(ui, &[facts.command(mode)]);
        let loses = level == Level::Loses;
        let label = if loses { "Reset anyway" } else { "Reset" };
        let answer = dialogs::actions(ui, label, level != Level::Refused, loses, fresh && loses);
        // Enter runs a reset that loses nothing.
        if answer == dialogs::Answer::Open
            && matches!(level, Level::Calm | Level::Caution)
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
        {
            return dialogs::Answer::Primary;
        }
        answer
    });
    open.mode = mode;
    if show_log {
        *log = Some((facts.repo.clone(), facts.commits.clone()));
    }
    if shown.should_close() {
        return dialogs::Answer::Cancel;
    }
    shown.inner
}

fn open_gist(mode: Mode) -> String {
    format!("{}: {}", mode.flag(), mode.gist())
}

fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// An amber box for something to be aware of that isn't lost work.
fn caution(ui: &mut Ui, content: impl FnOnce(&mut Ui)) {
    let color = if ui.visuals().dark_mode {
        Color32::from_rgb(240, 191, 95)
    } else {
        Color32::from_rgb(139, 86, 0)
    };
    egui::Frame::new()
        .fill(color.gamma_multiply(0.10))
        .corner_radius(6)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.visuals_mut().override_text_color = Some(color);
            content(ui);
        });
}
