//! PROTOTYPE — throwaway. Resetting the open worktree's branch from the log window, for
//! "Resetting a branch" (aquamoth/parterre#172).
//!
//! Round 2: variant B of round 1, always a dialog. Above the modes, what the reset does to the
//! branch and what uncommitted changes there are. Only the modes that make a difference here
//! can be picked; one that would do the same as another, or that git refuses, is greyed out.
//! Below them, what the picked mode does, as information, or as a warning when it loses work.
//! Lost commits are always lost work. Not offered while a merge, rebase, cherry-pick or revert
//! is in progress. Asking again while the dialog is open brings it to the front.
//!
//! Nothing runs git to change anything; a notification says what would have run. The demo
//! repository is `prototype_reset_demo.sh`. `PARTERRE_RESET_DEMO=<rev>[:<mode>]` opens the
//! dialog at startup, for `--screenshot`.

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
enum Mode {
    Soft,
    Mixed,
    Keep,
    Hard,
}

impl Mode {
    /// In order of how much they touch, which is also which of several modes that do the same
    /// here stays: the one that does least harm if the repository changed meanwhile. `--merge`
    /// is left to the command line: it's for aborting a merge.
    const ALL: [Mode; 4] = [Mode::Soft, Mode::Mixed, Mode::Keep, Mode::Hard];

    fn name(self) -> &'static str {
        match self {
            Mode::Soft => "Soft",
            Mode::Mixed => "Mixed",
            Mode::Keep => "Keep",
            Mode::Hard => "Hard",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Mode::Soft => "--soft",
            Mode::Mixed => "--mixed",
            Mode::Keep => "--keep",
            Mode::Hard => "--hard",
        }
    }

    fn parse(s: &str) -> Option<Mode> {
        Mode::ALL
            .into_iter()
            .find(|m| m.name().eq_ignore_ascii_case(s))
    }
}

/// What happens to one kind of change; `None` where there is none of that kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fate {
    Staged,
    Unstaged,
    Kept,
    Dropped,
    Refused,
}

/// What a reset of the open worktree's branch to `target` would meet.
#[derive(Clone, Debug)]
struct Facts {
    repo: Arc<Repo>,
    branch: String,
    head: Oid,
    target: Oid,
    /// Commits the branch leaves behind, and gains (`rev-list --left-right --count`).
    behind: usize,
    gained: usize,
    staged: BTreeSet<String>,
    unstaged: BTreeSet<String>,
    /// Untracked files at paths the target has.
    in_the_way: BTreeSet<String>,
    /// Paths that differ between HEAD and the target.
    changed: BTreeSet<String>,
    /// Commits only the branch reaches, which nothing reaches afterwards.
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

fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

impl Facts {
    /// `None` when there's nothing to offer: a detached HEAD (which includes a branch being
    /// rebased), an operation in progress, or the commit HEAD is at.
    fn load(repo: &Arc<Repo>, catalog: &Catalog, target: Oid) -> Option<Facts> {
        let branch = catalog.current.clone()?;
        let head = catalog.head?;
        let open = catalog.worktrees.iter().find(|w| w.open)?;
        if head == target || !catalog.has_working_tree || open.in_progress.is_some() {
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
        let range = format!("{h}...{t}");
        let counts = git(&root, &["rev-list", "--left-right", "--count", &range]);
        let mut counts = counts.split_whitespace().map(|n| n.parse().unwrap_or(0));
        Some(Facts {
            repo: repo.clone(),
            branch,
            head,
            target,
            behind: counts.next().unwrap_or(0),
            gained: counts.next().unwrap_or(0),
            staged: names(&root, &["diff", "--cached", "--name-only", "-z"]),
            unstaged: names(&root, &["diff", "--name-only", "-z"]),
            in_the_way: untracked.intersection(&target_files).cloned().collect(),
            changed: names(&root, &["diff", "--name-only", "-z", &h, &t]),
            commits,
        })
    }

    fn short(&self) -> String {
        self.target.short(self.repo.abbrev_len.max(7))
    }

    fn head_short(&self) -> String {
        self.head.short(self.repo.abbrev_len.max(7))
    }

    fn command(&self, mode: Mode) -> String {
        format!("git reset {} {}", mode.flag(), self.short())
    }

    /// Files both staged and modified since: their staged version exists only in the index.
    fn partly_staged(&self) -> Vec<String> {
        self.staged.intersection(&self.unstaged).cloned().collect()
    }

    /// What happens to the commits' changes, your staged changes, your unstaged changes and
    /// untracked files in the way. Two modes with the same fates do the same here.
    fn fates(&self, mode: Mode) -> [Option<Fate>; 4] {
        use Fate::*;
        let commits = match mode {
            Mode::Soft => Staged,
            Mode::Mixed => Unstaged,
            _ => Dropped,
        };
        let staged = match mode {
            Mode::Soft => Staged,
            Mode::Mixed | Mode::Keep => Unstaged,
            Mode::Hard => Dropped,
        };
        let unstaged = match mode {
            Mode::Hard => Dropped,
            _ => Kept,
        };
        let in_the_way = match mode {
            Mode::Soft | Mode::Mixed => Kept,
            Mode::Keep => Refused,
            Mode::Hard => Dropped,
        };
        [
            (!self.changed.is_empty()).then_some(commits),
            (!self.staged.is_empty()).then_some(staged),
            (!self.unstaged.is_empty()).then_some(unstaged),
            (!self.in_the_way.is_empty()).then_some(in_the_way),
        ]
    }

    /// Git's refusal, as git words it (git 2.34 and 2.43).
    fn refusal(&self, mode: Mode) -> Option<String> {
        let short = self.short();
        let fail = |what: String| {
            Some(format!(
                "{what}\nfatal: Could not reset index file to revision '{short}'."
            ))
        };
        let not_uptodate = |p: &String| fail(format!("error: Entry '{p}' not uptodate. Cannot merge."));
        let untracked = |p: &String| {
            fail(format!(
                "error: Untracked working tree file '{p}' would be overwritten by merge."
            ))
        };
        match mode {
            Mode::Soft | Mode::Mixed | Mode::Hard => None,
            Mode::Keep => {
                let local: BTreeSet<String> = self.staged.union(&self.unstaged).cloned().collect();
                match local.intersection(&self.changed).next() {
                    Some(p) => not_uptodate(p),
                    None => self.in_the_way.iter().next().and_then(untracked),
                }
            }
        }
    }

    /// Files whose content is gone afterwards, and why.
    fn lost_files(&self, mode: Mode) -> Vec<(String, &'static str)> {
        match mode {
            Mode::Soft => Vec::new(),
            Mode::Mixed | Mode::Keep => self
                .partly_staged()
                .into_iter()
                .map(|f| (f, "staged version"))
                .collect(),
            Mode::Hard => {
                let local: BTreeSet<&String> = self.staged.union(&self.unstaged).collect();
                let mut files: Vec<_> = local
                    .into_iter()
                    .map(|f| {
                        let why = match (self.staged.contains(f), self.unstaged.contains(f)) {
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
                files
            }
        }
    }

    fn loses(&self, mode: Mode) -> bool {
        !self.commits.is_empty() || !self.lost_files(mode).is_empty()
    }

    /// The mode a greyed-out one would do the same as, when it isn't refused.
    fn same_as(&self, mode: Mode) -> Option<Mode> {
        let fates = self.fates(mode);
        Mode::ALL
            .into_iter()
            .take_while(|&m| m != mode)
            .find(|&m| self.refusal(m).is_none() && self.fates(m) == fates)
    }

    fn enabled(&self, mode: Mode) -> bool {
        self.refusal(mode).is_none() && self.same_as(mode).is_none()
    }

    /// The first of Keep, Mixed and Soft that loses nothing, else the first that can be picked.
    fn default_mode(&self) -> Mode {
        let modes = [Mode::Keep, Mode::Mixed, Mode::Soft];
        modes
            .into_iter()
            .find(|&m| self.enabled(m) && !self.loses(m))
            .or_else(|| modes.into_iter().find(|&m| self.enabled(m)))
            .unwrap_or(Mode::Mixed)
    }

    /// What the reset does to the branch.
    fn movement(&self) -> String {
        let (b, from, to) = (&self.branch, self.head_short(), self.short());
        match (self.behind, self.gained) {
            (n, 0) => format!("Moves {b} back {} from {from} to {to}.", plural(n, "commit")),
            (0, n) => format!("Moves {b} forward {} from {from} to {to}.", plural(n, "commit")),
            (l, g) => format!(
                "Moves {b} from {from} to {to}, on another line: it leaves {} behind and gains {g}.",
                plural(l, "commit")
            ),
        }
    }

    /// The uncommitted changes, in git status's words.
    fn changes(&self) -> String {
        let mut parts = Vec::new();
        let only_staged = self.staged.len() - self.partly_staged().len();
        if only_staged > 0 {
            parts.push(format!("{only_staged} staged"));
        }
        let partly = self.partly_staged().len();
        if partly > 0 {
            parts.push(format!("{partly} staged and modified since"));
        }
        let only_unstaged = self.unstaged.len() - partly;
        if only_unstaged > 0 {
            parts.push(format!("{only_unstaged} modified"));
        }
        if !self.in_the_way.is_empty() {
            parts.push(format!(
                "{} untracked where {} has a file",
                self.in_the_way.len(),
                self.short()
            ));
        }
        if parts.is_empty() {
            "No uncommitted changes.".into()
        } else {
            format!("Uncommitted: {}.", parts.join(", "))
        }
    }

    /// What `mode` does here, sentence by sentence.
    fn explanation(&self, mode: Mode) -> Vec<String> {
        let to = self.short();
        let mut lines = Vec::new();
        let files = plural(self.changed.len(), "file");
        let theirs = if self.behind == 0 {
            format!("The difference to {to} ({files})")
        } else {
            format!(
                "The changes of the {} left behind ({files})",
                if self.behind == 1 { "commit" } else { "commits" }
            )
        };
        let local = !self.staged.is_empty() || !self.unstaged.is_empty();
        match mode {
            Mode::Soft => {
                if !self.changed.is_empty() {
                    lines.push(format!("{theirs} become staged, ready to commit again."));
                }
                lines.push("Your files stay as they are.".into());
                if !self.staged.is_empty() {
                    lines.push("Your staged changes stay staged.".into());
                }
            }
            Mode::Mixed => {
                if !self.changed.is_empty() {
                    lines.push(format!("{theirs} show up as modified, not staged."));
                }
                lines.push("Your files stay as they are.".into());
                if !self.staged.is_empty() {
                    lines.push("Your staged changes become unstaged.".into());
                }
            }
            Mode::Keep => {
                lines.push(format!("Your files are updated to {to}."));
                if local {
                    lines.push("Your changes are kept, as modified files; nothing stays staged.".into());
                }
            }
            Mode::Hard => {
                lines.push(format!("Your files are updated to {to}."));
                if local {
                    lines.push("All your uncommitted changes are dropped.".into());
                }
                if !self.in_the_way.is_empty() {
                    lines.push("Untracked files where it has a file are overwritten.".into());
                }
            }
        }
        lines
    }
}

#[derive(Debug)]
struct Open {
    facts: Facts,
    mode: Mode,
    opener: ViewportId,
    fresh: bool,
    /// Asked for again while open: left out for a frame, so its window opens anew in front.
    reopen: bool,
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

fn notify(state: &mut State, ctx: &egui::Context, facts: &Facts, mode: Mode) {
    state.notices.push(Notice {
        title: format!("Reset {} to {}", facts.branch, facts.short()),
        command: facts.command(mode),
        at: ctx.input(|i| i.time),
    });
}

/// The reset item of a log row's menu: only for the open worktree's branch.
pub fn menu(ui: &mut Ui, repo: &Arc<Repo>, commit: Oid, catalog: Option<&Catalog>) {
    let Some(catalog) = catalog else { return };
    if catalog.current.is_none() || catalog.head == Some(commit) {
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
        menu::separator(ui);
        if ui
            .button(format!("Reset {} to here…", facts.branch))
            .clicked()
        {
            let reopen = state.open.is_some();
            state.open = Some(Open {
                mode: facts.default_mode(),
                facts,
                opener: ui.ctx().viewport_id(),
                fresh: true,
                reopen,
                show_files: false,
            });
            ui.close();
        }
    });
}

/// The notifications, over the log window.
pub fn overlay(ctx: &egui::Context) {
    STATE.with_borrow_mut(|state| {
        let now = ctx.input(|i| i.time);
        state.notices.retain(|n| now - n.at < 5.0);
        if state.notices.is_empty() {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        egui::Area::new(Id::new("prototype-reset-notices"))
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-16.0, -16.0))
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
                    state.open = Some(Open {
                        mode: Mode::parse(mode).unwrap_or_else(|| facts.default_mode()),
                        facts,
                        opener: log,
                        fresh: true,
                        reopen: false,
                        show_files: true,
                    });
                }
            }
        }
        let Some(mut open) = state.open.take() else {
            return;
        };
        if std::mem::take(&mut open.reopen) {
            ctx.request_repaint();
            state.open = Some(open);
            return;
        }
        match dialog(ctx, &mut open, &mut state.log) {
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

fn dialog(
    ctx: &egui::Context,
    open: &mut Open,
    log: &mut Option<(Arc<Repo>, Vec<Oid>)>,
) -> dialogs::Answer {
    let facts = &open.facts;
    let loses = facts.loses(open.mode);
    let lost_files = facts.lost_files(open.mode);
    let files = plural(lost_files.len(), "file");
    let commits = plural(facts.commits.len(), "commit");
    let title = if loses {
        let cost = match (lost_files.is_empty(), facts.commits.is_empty()) {
            (false, false) => format!("{files} and {commits}"),
            (false, true) => files.clone(),
            _ => commits.clone(),
        };
        format!("Reset {} and lose {cost}?", facts.branch)
    } else {
        format!("Reset {} to {}", facts.branch, facts.short())
    };
    let mut dialog = dialogs::Dialog::new("prototype-reset", &title)
        .opener(open.opener)
        .raise(open.fresh);
    if loses {
        dialog = dialog.icon(TRIANGLE, true);
    }
    let mut mode = open.mode;
    let show_files = &mut open.show_files;
    let fresh = open.fresh;
    let mut show_log = false;
    let shown = dialog.show(ctx, |ui| {
        dialogs::fields(ui, |ui| {
            if let Some(ix) = facts.repo.lookup(&facts.target) {
                dialogs::commit_line(ui, facts.repo.commit(ix), facts.repo.abbrev_len);
            }
            ui.label(facts.movement());
            ui.label(RichText::new(facts.changes()).weak());
            ui.add_space(4.0);
            for m in Mode::ALL {
                let enabled = facts.enabled(m);
                let response = ui
                    .add_enabled(enabled, egui::RadioButton::new(mode == m, m.name()))
                    .on_hover_text(m.flag());
                let response = match (facts.refusal(m), facts.same_as(m)) {
                    (Some(refusal), _) => response.on_disabled_hover_text(
                        RichText::new(format!("git reset {} refuses:\n{refusal}", m.flag()))
                            .monospace(),
                    ),
                    (None, Some(same)) => response
                        .on_disabled_hover_text(format!("Same as {} here", same.name())),
                    _ => response,
                };
                if response.clicked() {
                    mode = m;
                }
            }
            ui.add_space(4.0);
            let lost_files = facts.lost_files(mode);
            let loses = facts.loses(mode);
            explanation_box(ui, loses, |ui| {
                for line in facts.explanation(mode) {
                    ui.label(line);
                }
                if !lost_files.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(format!("Lost: {}", plural(lost_files.len(), "file")));
                        let label = if *show_files { "Hide" } else { "Show" };
                        if ui.link(label).clicked() {
                            *show_files = !*show_files;
                        }
                    });
                    if *show_files {
                        for (file, why) in &lost_files {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(file).monospace());
                                ui.label(RichText::new(*why).small());
                            });
                        }
                    }
                }
                if !facts.commits.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "Lost: {}, on no branch, tag or worktree afterwards.",
                            plural(facts.commits.len(), "commit")
                        ));
                        show_log |= ui.link("Show in log").clicked();
                    });
                }
            });
        });
        dialogs::command_box(ui, &[facts.command(mode)]);
        let loses = facts.loses(mode);
        let label = if loses { "Reset anyway" } else { "Reset" };
        let answer = dialogs::actions(ui, label, facts.enabled(mode), loses, fresh && loses);
        // Enter runs a reset that loses nothing.
        if answer == dialogs::Answer::Open
            && !loses
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

/// Information in a quiet box, or a warning in a red one.
fn explanation_box(ui: &mut Ui, warning: bool, content: impl FnOnce(&mut Ui)) {
    let color = if warning {
        ui.visuals().error_fg_color
    } else {
        widgets::tones(ui).accent
    };
    egui::Frame::new()
        .fill(color.gamma_multiply(0.10))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.5)))
        .corner_radius(6)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if warning {
                ui.visuals_mut().override_text_color = Some(color);
            }
            content(ui);
        });
}
