//! PROTOTYPE — throwaway. The *Add worktree…* dialog, for "Add worktree dialog: prototype"
//! (aquamoth/parterre#158). Nothing runs git to change anything: *Create* only says what would
//! run. The dialog reads the repository for the commit's branches, the suggested folder,
//! existing folders and whether a folder inside the repository is ignored.
//!
//! Four variants of the same dialog, switched with the bar at the bottom (or ← and → while no
//! text field has the focus), or `PARTERRE_ADD_WORKTREE_VARIANT=A|B|C|D`:
//! - A: every choice in view, as radio buttons; the folder as one path with *Browse…*.
//! - B: one branch field: a branch of the commit, or a new name. Suggestions under it, a line
//!   saying what it will do, and the folder folded into one line with *Change…*.
//! - C: tabs for *Branch here*, *New branch* and *Remote branch*; the folder in two parts, where
//!   it goes (*Browse…*) and its name.
//! - D (round 2, the default): C's folder on top; under it B's branch field as a dropdown of the
//!   commit's branches that no worktree has, to type a new name in, or to empty for a detached
//!   HEAD (named after the commit); the command at the bottom.
//!
//! Opened from *Add worktree here…* (the menus prototype, #144), from *Create a worktree for X…*
//! when a switch is blocked (#147), from the panel at the bottom left, or with
//! `PARTERRE_ADD_WORKTREE=<rev>` for `--screenshot`.
//!
//! Defaults, from #158: the commit's local branch when it isn't checked out anywhere; else its
//! remote branch as a new tracking branch; else a new branch. Read as: *New branch* is the
//! default only when the commit has no usable branch.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, Key, RichText, Ui};
use parterre_core::{CommitIx, Oid, RefKind, Repo};

use crate::file_dialog::Pending;

pub const VARIANTS: [(&str, &str); 4] = [
    ("A", "Every choice in view"),
    ("B", "One branch field"),
    ("C", "Tabs, the folder in two parts"),
    ("D", "Round 2: C's folder, a branch dropdown, the command"),
];

/// What the new worktree checks out, in variants A and C.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sel {
    Local(usize),
    New,
    Remote(usize),
}

/// The tabs of variant C.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Tab {
    Local,
    New,
    Remote,
}

/// What would run.
#[derive(Clone, Debug)]
enum Plan {
    Local(String),
    New(String),
    Track {
        remote: String,
        local: String,
    },
    /// D: the branch field left empty.
    Detached,
}

impl Plan {
    fn branch(&self) -> &str {
        match self {
            Plan::Local(n) | Plan::New(n) => n,
            Plan::Track { local, .. } => local,
            Plan::Detached => "",
        }
    }
}

struct LocalBranch {
    name: String,
    /// Why it can't be checked out: the worktree that has it.
    blocked: Option<String>,
}

struct RemoteBranch {
    /// `origin/x`.
    name: String,
    /// `x`.
    local: String,
    blocked: Option<String>,
}

struct Dialog {
    hash: String,
    subject: String,
    locals: Vec<LocalBranch>,
    remotes: Vec<RemoteBranch>,
    sel: Sel,
    tab: Tab,
    /// The new branch's name (A, C), or what's typed in the branch field (B).
    name: String,
    typed: String,
    /// The folder typed as a whole path (A, B); overrides the rest.
    edited: Option<String>,
    /// Where it goes: picked with *Browse…*, or typed in C.
    parent: Option<PathBuf>,
    /// Its name, typed in C.
    leaf: Option<String>,
    /// B's folder line unfolded.
    folder_open: bool,
    first_frame: bool,
    browse: Option<Pending<()>>,
}

struct State {
    variant: usize,
    open: Option<Dialog>,
    panel: bool,
    /// `git check-ref-format --branch`, by name.
    valid: HashMap<String, bool>,
    /// `git check-ignore`, by worktree and path.
    ignored: HashMap<(PathBuf, String), bool>,
    /// Patterns *Exclude it* would have added (nothing is written).
    excluded: HashSet<(PathBuf, String)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        variant: match std::env::var("PARTERRE_ADD_WORKTREE_VARIANT").as_deref() {
            Ok("B" | "b") => 1,
            Ok("A" | "a") => 0,
            Ok("C" | "c") => 2,
            _ => 3,
        },
        open: None,
        panel: true,
        valid: HashMap::new(),
        ignored: HashMap::new(),
        excluded: HashSet::new(),
    });
}

fn variant() -> usize {
    STATE.with(|s| s.borrow().variant)
}

// ---------------------------------------------------------------------------------------------
// Reading the repository.

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

fn resolve(repo: &Repo, rev: &str) -> Option<CommitIx> {
    let (ok, out) = git_in(
        &repo.path,
        &["rev-parse", "--verify", "-q", &format!("{rev}^{{commit}}")],
    );
    if !ok {
        return None;
    }
    repo.lookup(&Oid::from_hex(out.trim())?)
}

fn valid_name(name: &str) -> bool {
    if let Some(v) = STATE.with(|s| s.borrow().valid.get(name).copied()) {
        return v;
    }
    let v = crate::prototype_warnings::git_command()
        .args(["check-ref-format", "--branch", name])
        .output()
        .is_ok_and(|o| o.status.success());
    STATE.with(|s| s.borrow_mut().valid.insert(name.to_owned(), v));
    v
}

/// The worktree (by name) that has `branch` (`refs/heads/…`) checked out.
fn checked_out_in(repo: &Repo, full: &str) -> Option<String> {
    repo.worktrees
        .iter()
        .find(|w| w.branch.as_deref() == Some(full))
        .map(|w| {
            if w.open {
                format!("{} (this window)", w.name())
            } else {
                w.name()
            }
        })
}

fn local_exists(repo: &Repo, name: &str) -> Option<CommitIx> {
    let full = format!("refs/heads/{name}");
    repo.refs
        .iter()
        .find(|r| r.full_name == full)
        .map(|r| r.target)
}

fn hash(repo: &Repo, c: CommitIx) -> String {
    repo.commit(c).oid.short(repo.abbrev_len)
}

/// Paths as the user's platform writes them.
fn shown(p: &Path) -> String {
    let s = p.display().to_string();
    if cfg!(windows) {
        s.replace('/', "\\")
    } else {
        s
    }
}

/// For comparing paths: forward slashes, no trailing one, case folded on Windows.
fn norm(p: &Path) -> String {
    let s = p.display().to_string().replace('\\', "/");
    let s = s.trim_end_matches('/').to_owned();
    if cfg!(windows) { s.to_lowercase() } else { s }
}

fn slug(branch: &str) -> String {
    branch.replace('/', "-")
}

/// VS Code's suggestion: `<parent>/<repo>.worktrees/` from the main worktree, beside it from a
/// linked one.
fn default_parent(repo: &Repo) -> PathBuf {
    let Some(main) = repo.worktrees.first() else {
        return repo.path.parent().unwrap_or(&repo.path).to_owned();
    };
    match repo.worktrees.iter().position(|w| w.open) {
        Some(o) if o != 0 => {
            let open = &repo.worktrees[o].path;
            open.parent().unwrap_or(open).to_owned()
        }
        _ => main
            .path
            .parent()
            .unwrap_or(&main.path)
            .join(format!("{}.worktrees", main.name())),
    }
}

/// A folder git would refuse, or that isn't empty: a worktree's, or anything with files in it.
fn taken(repo: &Repo, p: &Path) -> bool {
    let n = norm(p);
    repo.worktrees.iter().any(|w| norm(&w.path) == n)
        || p.is_file()
        || p.read_dir().is_ok_and(|mut d| d.next().is_some())
}

/// `p`, or `p-1`, `p-2`… if it's taken.
fn free(repo: &Repo, p: &Path) -> PathBuf {
    if !taken(repo, p) {
        return p.to_owned();
    }
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (1..)
        .map(|k| p.with_file_name(format!("{name}-{k}")))
        .find(|q| !taken(repo, q))
        .unwrap()
}

/// The worktree the folder is inside, and the pattern *Exclude it* would add for it, when git
/// status there would list it.
fn inside(repo: &Repo, p: &Path) -> Option<(PathBuf, String, String)> {
    let n = norm(p);
    let wt = repo
        .worktrees
        .iter()
        .filter(|w| !w.missing)
        .filter(|w| n.starts_with(&format!("{}/", norm(&w.path))))
        .max_by_key(|w| norm(&w.path).len())?;
    let rel = n[norm(&wt.path).len() + 1..].to_owned();
    let top = rel.split('/').next().unwrap_or(&rel).to_owned();
    let pattern = format!("/{top}/");
    let key = (wt.path.clone(), rel.clone());
    let excluded = STATE.with(|s| {
        s.borrow()
            .excluded
            .contains(&(wt.path.clone(), pattern.clone()))
    });
    if excluded {
        return None;
    }
    let ignored = STATE.with(|s| s.borrow().ignored.get(&key).copied());
    let ignored = ignored.unwrap_or_else(|| {
        let (ok, _) = git_in(&wt.path, &["check-ignore", "-q", &format!("{rel}/")]);
        STATE.with(|s| s.borrow_mut().ignored.insert(key, ok));
        ok
    });
    (!ignored).then(|| (wt.path.clone(), wt.name(), pattern))
}

// ---------------------------------------------------------------------------------------------
// Opening it.

fn dialog_for(repo: &Repo, commit: CommitIx, prefer: Option<&str>) -> Dialog {
    let mut locals: Vec<LocalBranch> = repo
        .refs
        .iter()
        .filter(|r| r.target == commit && r.kind == RefKind::LocalBranch)
        .map(|r| LocalBranch {
            name: r.name.clone(),
            blocked: checked_out_in(repo, &r.full_name).map(|w| format!("checked out in {w}")),
        })
        .collect();
    locals.sort_by(|a, b| a.name.cmp(&b.name));
    let mut remotes: Vec<RemoteBranch> = repo
        .refs
        .iter()
        .filter(|r| r.target == commit && r.kind == RefKind::RemoteBranch)
        .filter(|r| !r.name.ends_with("/HEAD"))
        // One whose local branch is here too is that branch.
        .filter_map(|r| {
            let local = r.name.split_once('/').map_or(r.name.as_str(), |(_, b)| b);
            let blocked = match local_exists(repo, local) {
                Some(c) if c == commit => return None,
                Some(c) => Some(format!(
                    "a branch {local} exists already, at {}",
                    hash(repo, c)
                )),
                None => None,
            };
            Some(RemoteBranch {
                name: r.name.clone(),
                local: local.to_owned(),
                blocked,
            })
        })
        .collect();
    remotes.sort_by(|a, b| a.name.cmp(&b.name));
    let preferred = prefer.and_then(|p| {
        locals
            .iter()
            .position(|l| l.name == p && l.blocked.is_none())
    });
    let sel = preferred
        .or_else(|| locals.iter().position(|l| l.blocked.is_none()))
        .map(Sel::Local)
        .or_else(|| {
            remotes
                .iter()
                .position(|r| r.blocked.is_none())
                .map(Sel::Remote)
        })
        .unwrap_or(Sel::New);
    let tab = match sel {
        Sel::Local(_) => Tab::Local,
        Sel::New => Tab::New,
        Sel::Remote(_) => Tab::Remote,
    };
    let typed = match sel {
        Sel::Local(i) => locals[i].name.clone(),
        Sel::Remote(i) => remotes[i].name.clone(),
        Sel::New => String::new(),
    };
    let c = repo.commit(commit);
    Dialog {
        hash: hash(repo, commit),
        subject: c.subject.clone(),
        locals,
        remotes,
        sel,
        tab,
        name: String::new(),
        typed,
        edited: None,
        parent: None,
        leaf: None,
        folder_open: false,
        first_frame: true,
        browse: None,
    }
}

pub fn start(repo: &Repo, commit: CommitIx, prefer: Option<&str>) {
    let d = dialog_for(repo, commit, prefer);
    STATE.with(|s| s.borrow_mut().open = Some(d));
}

/// The menus prototype's *Add worktree here…*: true if `command` was taken over.
pub fn intercept(repo: &Repo, command: &str) -> bool {
    if !command.starts_with("git worktree add") {
        return false;
    }
    let Some(commit) = command
        .split_whitespace()
        .last()
        .and_then(|h| resolve(repo, h))
    else {
        return false;
    };
    start(repo, commit, None);
    true
}

/// *Create a worktree for X…* from a blocked switch.
pub fn start_for_branch(repo: &Repo, branch: &str) {
    if let Some(commit) = resolve(repo, &format!("refs/heads/{branch}")) {
        start(repo, commit, Some(branch));
    }
}

/// For screenshots: `PARTERRE_ADD_WORKTREE=<rev>` opens the dialog on that commit, once.
pub fn from_env(repo: &Repo) {
    thread_local!(static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    let Ok(rev) = std::env::var("PARTERRE_ADD_WORKTREE") else {
        return;
    };
    if let Some(c) = resolve(repo, &rev) {
        start(repo, c, None);
    }
    // And `PARTERRE_ADD_WORKTREE_FOLDER=<path>` as if typed in the folder field.
    if let Ok(folder) = std::env::var("PARTERRE_ADD_WORKTREE_FOLDER") {
        STATE.with(|s| {
            if let Some(d) = s.borrow_mut().open.as_mut() {
                d.edited = Some(folder);
                d.folder_open = true;
            }
        });
    }
}

// ---------------------------------------------------------------------------------------------
// What it would do.

impl Dialog {
    /// What would run, or why *Create* is greyed out.
    fn plan(&self, repo: &Repo, v: usize) -> Result<Plan, String> {
        if v == 3 && self.typed.trim().is_empty() {
            return Ok(Plan::Detached);
        }
        if v == 1 || v == 3 {
            return self.plan_typed(repo);
        }
        let sel = if v == 2 {
            match self.tab {
                Tab::Local => match self.sel {
                    Sel::Local(i) => Sel::Local(i),
                    _ => Sel::Local(0),
                },
                Tab::New => Sel::New,
                Tab::Remote => match self.sel {
                    Sel::Remote(i) => Sel::Remote(i),
                    _ => Sel::Remote(0),
                },
            }
        } else {
            self.sel
        };
        match sel {
            Sel::Local(i) => {
                let l = self.locals.get(i).ok_or("No branch here")?;
                match &l.blocked {
                    Some(why) => Err(format!("{} is {why}", l.name)),
                    None => Ok(Plan::Local(l.name.clone())),
                }
            }
            Sel::Remote(i) => {
                let r = self.remotes.get(i).ok_or("No remote branch here")?;
                match &r.blocked {
                    Some(why) => Err(format!("Can't track {}: {why}", r.name)),
                    None => Ok(Plan::Track {
                        remote: r.name.clone(),
                        local: r.local.clone(),
                    }),
                }
            }
            Sel::New => self.new_branch(repo, self.name.trim()),
        }
    }

    fn new_branch(&self, repo: &Repo, name: &str) -> Result<Plan, String> {
        if name.is_empty() {
            return Err("Type a name for the new branch".to_owned());
        }
        if let Some(c) = local_exists(repo, name) {
            return Err(format!(
                "A branch {name} exists already, at {}",
                hash(repo, c)
            ));
        }
        if !valid_name(name) {
            return Err(format!("{name} isn't a valid branch name"));
        }
        Ok(Plan::New(name.to_owned()))
    }

    /// B: an existing branch of the commit if the name is one, else a new branch.
    fn plan_typed(&self, repo: &Repo) -> Result<Plan, String> {
        let t = self.typed.trim();
        if let Some(l) = self.locals.iter().find(|l| l.name == t) {
            return match &l.blocked {
                Some(why) => Err(format!("{t} is {why}")),
                None => Ok(Plan::Local(t.to_owned())),
            };
        }
        if let Some(r) = self.remotes.iter().find(|r| r.name == t || r.local == t) {
            return match &r.blocked {
                Some(why) if r.name == t => Err(format!("Can't track {t}: {why}")),
                Some(_) => self.new_branch(repo, t),
                None => Ok(Plan::Track {
                    remote: r.name.clone(),
                    local: r.local.clone(),
                }),
            };
        }
        self.new_branch(repo, t)
    }

    /// The folder as the user has it, before `-1`, `-2`….
    fn folder(&self, repo: &Repo, branch: &str) -> PathBuf {
        if let Some(t) = &self.edited {
            return PathBuf::from(t.trim());
        }
        let leaf = self.leaf.clone().unwrap_or_else(|| slug(branch));
        self.parent
            .clone()
            .unwrap_or_else(|| default_parent(repo))
            .join(leaf)
    }

    /// The branch whose name the folder follows: the plan's, or what's typed so far.
    fn branch_for_folder(&self, plan: &Result<Plan, String>, v: usize) -> String {
        match plan {
            // A detached worktree's folder is named after the commit.
            Ok(Plan::Detached) => self.hash.clone(),
            Ok(p) => p.branch().to_owned(),
            Err(_) if v == 1 || v == 3 => self.typed.trim().to_owned(),
            Err(_) => self.name.trim().to_owned(),
        }
    }
}

fn quote(p: &Path) -> String {
    let s = shown(p);
    if s.contains(' ') {
        format!("\"{s}\"")
    } else {
        s
    }
}

fn command(plan: &Plan, folder: &Path, hash: &str) -> String {
    let f = quote(folder);
    match plan {
        Plan::Local(n) => format!("git worktree add {f} {n}"),
        Plan::New(n) => format!("git worktree add -b {n} {f} {hash}"),
        Plan::Track { remote, local } => {
            format!("git worktree add --track -b {local} {f} {remote}")
        }
        Plan::Detached => format!("git worktree add --detach {f} {hash}"),
    }
}

fn describe(plan: &Plan, hash: &str) -> String {
    match plan {
        Plan::Local(n) => format!("Checks out {n}"),
        Plan::New(n) => format!("Creates the branch {n} at {hash}"),
        Plan::Track { remote, local } => {
            format!("Creates the branch {local}, tracking {remote}")
        }
        Plan::Detached => format!("Detached HEAD at {hash}, on no branch"),
    }
}

// ---------------------------------------------------------------------------------------------
// Showing it.

pub fn show(ctx: &egui::Context, repo: &Repo) {
    panel(ctx, repo);
    let Some(mut d) = STATE.with(|s| s.borrow_mut().open.take()) else {
        return;
    };
    if let Some(p) = &d.browse
        && let Some(answer) = p.answer()
    {
        d.browse = None;
        if let Some(dir) = answer {
            // Browse picks where it goes; the name stays the branch's.
            if let Some(t) = d.edited.take() {
                d.leaf = Path::new(t.trim())
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
            }
            d.parent = Some(dir);
        }
    }
    let v = variant();
    let (style, frame) = crate::prototype_warnings::dialog_style(ctx);
    let mut close = false;
    let mut run = None;
    let modal = egui::Modal::new(egui::Id::new("prototype-add-worktree"))
        .frame(frame)
        .backdrop_color(Color32::from_black_alpha(if style.visuals.dark_mode {
            90
        } else {
            40
        }))
        .show(ctx, |ui| {
            ui.set_style(style.clone());
            ui.set_width(600.0);
            ui.spacing_mut().item_spacing.y = 10.0;
            // Rows as tall as the text fields, so labels line up with them.
            ui.spacing_mut().interact_size.y = 28.0;
            header(ui, &d, v);
            let mut create = false;
            let plan = match v {
                0 => variant_a(ui, repo, &mut d),
                1 => variant_b(ui, repo, &mut d),
                2 => variant_c(ui, repo, &mut d),
                _ => variant_d(ui, repo, &mut d),
            };
            let (wanted, folder, branch) = place(&d, repo, &plan, v);
            // D shows the folder above the branch.
            if v != 3 {
                match v {
                    0 => folder_a(ui, repo, &mut d, &folder),
                    1 => folder_b(ui, repo, &mut d, &folder),
                    _ => folder_c(ui, repo, &mut d, &folder, &branch),
                }
                folder_notes(ui, repo, &d, &wanted, &folder);
            }
            match &plan {
                Ok(p) => {
                    crate::prototype_warnings::command_box(ui, &[command(p, &folder, &d.hash)])
                }
                // Not before anything's typed.
                Err(why) if !why.starts_with("Type a name") => {
                    ui.label(RichText::new(why).color(Color32::from_rgb(0xc0, 0x1c, 0x28)));
                }
                Err(_) => {}
            }
            ui.add_space(4.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let ok = ui
                    .add_enabled_ui(plan.is_ok(), |ui| {
                        crate::widgets::primary_button(ui, "Create", 80.0)
                    })
                    .inner;
                let ok = match &plan {
                    Err(why) => ok.on_disabled_hover_text(why),
                    Ok(_) => ok,
                };
                create = ok.clicked();
                if crate::widgets::text_button(ui, "Cancel").clicked() {
                    close = true;
                }
                crate::prototype_warnings::focus_ring(ui, &ok);
                if d.first_frame && !wants_name(&d, v) {
                    ok.request_focus();
                }
            });
            let enter = ui.input(|i| i.key_pressed(Key::Enter)) && !d.first_frame;
            if (create || enter)
                && let Ok(p) = &plan
            {
                run = Some(command(p, &folder, &d.hash));
            }
        });
    variant_bar(ctx);
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        close = true;
    }
    if let Some(cmd) = &run {
        crate::prototype_warnings::toast(format!("Would run: {cmd}, then reload the graph"));
    }
    d.first_frame = false;
    if !(close || run.is_some() || (modal.should_close() && d.browse.is_none())) {
        STATE.with(|s| s.borrow_mut().open = Some(d));
    }
}

/// Where the worktree goes: as the user has it, after `-1`, `-2`…, and the branch it follows.
fn place(
    d: &Dialog,
    repo: &Repo,
    plan: &Result<Plan, String>,
    v: usize,
) -> (PathBuf, PathBuf, String) {
    let branch = d.branch_for_folder(plan, v);
    let wanted = d.folder(repo, &branch);
    // Nothing to make free before there's a name.
    let named = d.edited.is_some() || !d.leaf.clone().unwrap_or_else(|| slug(&branch)).is_empty();
    let folder = if named {
        free(repo, &wanted)
    } else {
        wanted.clone()
    };
    (wanted, folder, branch)
}

/// The folder isn't the one asked for, or is inside the repository.
fn folder_notes(ui: &mut Ui, repo: &Repo, d: &Dialog, wanted: &Path, folder: &Path) {
    if folder != wanted && (d.edited.is_some() || d.parent.is_some() || d.leaf.is_some()) {
        ui.label(
            RichText::new(format!(
                "{} isn't empty, so the worktree goes in {}.",
                shown(wanted),
                shown(folder)
            ))
            .weak(),
        );
    }
    if let Some((wt, name, pattern)) = inside(repo, folder) {
        note_inside(ui, &wt, &name, &pattern);
    }
}

/// Whether the name field takes the focus when the dialog opens.
fn wants_name(d: &Dialog, v: usize) -> bool {
    v == 1 || v == 3 || d.sel == Sel::New
}

fn header(ui: &mut Ui, d: &Dialog, v: usize) {
    let t = crate::widgets::tones(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let (badge, _) = ui.allocate_exact_size(egui::Vec2::splat(32.0), egui::Sense::hover());
        ui.painter().circle_filled(badge.center(), 16.0, t.on_bg);
        let icon = egui::Rect::from_center_size(badge.center(), egui::Vec2::splat(18.0));
        crate::widgets::paint_glyph(ui.painter(), icon, parterre_core::glyphs::FOLDER, t.on_fg);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new("Add a worktree").size(16.0).strong());
            ui.label(
                RichText::new(format!("at {} {}", d.hash, d.subject))
                    .weak()
                    .size(12.0),
            );
        });
    });
    ui.label(
        RichText::new(format!("PROTOTYPE {}: nothing runs", VARIANTS[v].0))
            .small()
            .color(Color32::from_rgb(200, 120, 0)),
    );
}

fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().strong().weak());
}

/// A text field that takes the focus when the dialog opens, if `focus`.
fn field(ui: &mut Ui, text: &mut String, hint: &str, width: f32, focus: bool) -> egui::Response {
    let r = crate::widgets::text_field(ui, text, hint, width);
    if focus {
        r.request_focus();
    }
    r
}

// A: every choice in view. -------------------------------------------------------------------

fn variant_a(ui: &mut Ui, repo: &Repo, d: &mut Dialog) -> Result<Plan, String> {
    caption(ui, "CHECK OUT");
    ui.spacing_mut().item_spacing.y = 6.0;
    for i in 0..d.locals.len() {
        let l = &d.locals[i];
        ui.horizontal(|ui| {
            let r = ui.add_enabled(
                l.blocked.is_none(),
                egui::RadioButton::new(d.sel == Sel::Local(i), &l.name),
            );
            if r.clicked() {
                d.sel = Sel::Local(i);
            }
            let note = l
                .blocked
                .clone()
                .unwrap_or_else(|| "local branch".to_owned());
            ui.label(RichText::new(note).weak());
        });
    }
    let focus = d.first_frame && d.sel == Sel::New;
    ui.horizontal(|ui| {
        if ui.radio(d.sel == Sel::New, "New branch").clicked() {
            d.sel = Sel::New;
        }
        let r = field(ui, &mut d.name, "name", 240.0, focus);
        if r.gained_focus() || r.changed() {
            d.sel = Sel::New;
        }
        ui.label(RichText::new(format!("at {}", d.hash)).weak());
    });
    for i in 0..d.remotes.len() {
        let r = &d.remotes[i];
        ui.horizontal(|ui| {
            let b = ui.add_enabled(
                r.blocked.is_none(),
                egui::RadioButton::new(
                    d.sel == Sel::Remote(i),
                    format!("{} as a new branch {}", r.name, r.local),
                ),
            );
            if b.clicked() {
                d.sel = Sel::Remote(i);
            }
            if let Some(why) = &r.blocked {
                ui.label(RichText::new(why).weak());
            }
        });
    }
    ui.spacing_mut().item_spacing.y = 10.0;
    d.plan(repo, 0)
}

fn folder_a(ui: &mut Ui, repo: &Repo, d: &mut Dialog, folder: &Path) {
    caption(ui, "FOLDER");
    path_field(ui, repo, d, folder);
}

/// The whole path, and *Browse…*.
fn path_field(ui: &mut Ui, repo: &Repo, d: &mut Dialog, folder: &Path) {
    ui.horizontal(|ui| {
        let mut text = d.edited.clone().unwrap_or_else(|| shown(folder));
        let w = ui.available_width() - 90.0;
        let r = crate::widgets::text_field(ui, &mut text, "", w);
        if r.changed() {
            d.edited = Some(text);
        }
        browse(ui, repo, d, folder);
    });
    if d.edited.is_some() || d.parent.is_some() {
        ui.horizontal(|ui| {
            ui.label(RichText::new("No longer follows the branch name.").weak());
            if ui.link("Suggest again").clicked() {
                d.edited = None;
                d.parent = None;
                d.leaf = None;
            }
        });
    }
}

fn browse(ui: &mut Ui, _repo: &Repo, d: &mut Dialog, folder: &Path) {
    let r = crate::widgets::text_button(ui, "Browse…")
        .on_hover_text("Pick where the worktree goes. Its name stays the branch's.");
    if r.clicked() && d.browse.is_none() {
        let mut dialog = rfd::AsyncFileDialog::new().set_title("Where the worktree goes");
        if let Some(dir) = folder.parent().filter(|p| p.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        d.browse = Some(Pending::start((), dialog.pick_folder(), ui.ctx()));
    }
}

// B: one branch field. -----------------------------------------------------------------------

fn variant_b(ui: &mut Ui, repo: &Repo, d: &mut Dialog) -> Result<Plan, String> {
    caption(ui, "BRANCH");
    let w = ui.available_width();
    let r = field(ui, &mut d.typed, "a new branch's name", w, d.first_frame);
    if d.first_frame {
        select_all(ui, r.id, &d.typed);
    }
    let names: Vec<(String, Option<String>)> = d
        .locals
        .iter()
        .map(|l| (l.name.clone(), l.blocked.clone()))
        .chain(
            d.remotes
                .iter()
                .map(|r| (r.name.clone(), r.blocked.clone())),
        )
        .collect();
    if !names.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("On this commit:").weak());
            for (n, blocked) in names {
                let b = ui.add_enabled(blocked.is_none(), egui::Button::new(&n).small());
                let b = match &blocked {
                    Some(why) => b.on_disabled_hover_text(why),
                    None => b,
                };
                if b.clicked() {
                    d.typed = n;
                }
            }
        });
    }
    let plan = d.plan(repo, 1);
    if let Ok(p) = &plan {
        ui.label(RichText::new(describe(p, &d.hash)).strong());
    }
    plan
}

fn folder_b(ui: &mut Ui, repo: &Repo, d: &mut Dialog, folder: &Path) {
    if d.folder_open {
        caption(ui, "FOLDER");
        path_field(ui, repo, d, folder);
    } else {
        ui.horizontal(|ui| {
            ui.label(RichText::new("In").weak());
            ui.label(RichText::new(shown(folder)).monospace().size(12.5));
            if ui.link("Change…").clicked() {
                d.folder_open = true;
            }
        });
    }
}

/// Typing replaces the suggestion.
fn select_all(ui: &Ui, id: egui::Id, text: &str) {
    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
        let end = egui::text::CCursor::new(text.chars().count());
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                end,
            )));
        state.store(ui.ctx(), id);
    }
}

// D: C's folder on top, a branch dropdown to type in, the command. ---------------------------

fn variant_d(ui: &mut Ui, repo: &Repo, d: &mut Dialog) -> Result<Plan, String> {
    // The folder follows the branch as typed up to this frame; typing repaints.
    let before = d.plan(repo, 3);
    let (wanted, folder, branch) = place(d, repo, &before, 3);
    folder_c(ui, repo, d, &folder, &branch);
    folder_notes(ui, repo, d, &wanted, &folder);
    caption(ui, "BRANCH");
    branch_combo(ui, d);
    let plan = d.plan(repo, 3);
    if let Ok(p) = &plan {
        ui.label(RichText::new(describe(p, &d.hash)).strong());
    }
    plan
}

/// A field to type a branch in, and a list of the commit's branches a worktree can check out:
/// none that a worktree has.
fn branch_combo(ui: &mut Ui, d: &mut Dialog) {
    let usable: Vec<String> = d
        .locals
        .iter()
        .filter(|l| l.blocked.is_none())
        .map(|l| l.name.clone())
        .chain(
            d.remotes
                .iter()
                .filter(|r| r.blocked.is_none())
                .map(|r| r.name.clone()),
        )
        .collect();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let w = ui.available_width() - 24.0;
        let r = field(
            ui,
            &mut d.typed,
            "a new branch, or empty for a detached HEAD",
            w,
            d.first_frame,
        );
        if d.first_frame {
            select_all(ui, r.id, &d.typed);
        }
        if r.changed() {
            ui.ctx().request_repaint();
        }
        let id = ui.id().with("prototype-add-worktree-branches");
        let chevron = crate::widgets::popover_button(ui, id, None, false);
        egui::Popup::from_toggle_button_response(&chevron)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .align(egui::RectAlign::BOTTOM_END)
            .gap(4.0)
            .style(crate::menu::popover_style)
            .show(|ui| {
                ui.set_min_width(240.0);
                if usable.is_empty() {
                    ui.label(RichText::new("No free branch on this commit").weak());
                }
                for n in usable {
                    let on = d.typed.trim() == n;
                    if ui.add(egui::Button::selectable(on, n.as_str())).clicked() {
                        d.typed = n;
                        ui.ctx().request_repaint();
                        ui.close();
                    }
                }
            });
    });
}

// C: tabs, the folder in two parts. ----------------------------------------------------------

fn variant_c(ui: &mut Ui, repo: &Repo, d: &mut Dialog) -> Result<Plan, String> {
    let mut tabs = Vec::new();
    if !d.locals.is_empty() {
        tabs.push((Tab::Local, "Branch here"));
    }
    tabs.push((Tab::New, "New branch"));
    if !d.remotes.is_empty() {
        tabs.push((Tab::Remote, "Remote branch"));
    }
    if tabs.len() > 1 {
        crate::widgets::text_segmented(ui, &mut d.tab, &tabs);
    }
    let t = crate::widgets::tones(ui);
    egui::Frame::new()
        .fill(t.seg_bg.gamma_multiply(0.5))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            match d.tab {
                Tab::Local => {
                    let i = match d.sel {
                        Sel::Local(i) => i,
                        _ => 0,
                    };
                    pick(
                        ui,
                        "Branch",
                        i,
                        &d.locals,
                        |l| &l.name,
                        |l| &l.blocked,
                        |i| d.sel = Sel::Local(i),
                    );
                }
                Tab::New => {
                    ui.horizontal(|ui| {
                        ui.label("New branch");
                        field(ui, &mut d.name, "name", 300.0, d.first_frame);
                        ui.label(RichText::new(format!("at {}", d.hash)).weak());
                    });
                }
                Tab::Remote => {
                    let i = match d.sel {
                        Sel::Remote(i) => i,
                        _ => 0,
                    };
                    pick(
                        ui,
                        "Track",
                        i,
                        &d.remotes,
                        |r| &r.name,
                        |r| &r.blocked,
                        |i| d.sel = Sel::Remote(i),
                    );
                    if let Some(r) = d.remotes.get(i) {
                        ui.label(RichText::new(format!("as a new branch {}", r.local)).weak());
                    }
                }
            }
        });
    d.plan(repo, 2)
}

/// One of several branches, or the only one as text.
fn pick<T>(
    ui: &mut Ui,
    label: &str,
    i: usize,
    items: &[T],
    name: impl Fn(&T) -> &String,
    blocked: impl Fn(&T) -> &Option<String>,
    mut set: impl FnMut(usize),
) {
    ui.horizontal(|ui| {
        ui.label(label);
        if items.len() == 1 {
            ui.label(RichText::new(name(&items[0])).strong());
        } else {
            egui::ComboBox::from_id_salt(("prototype-add-worktree-pick", label))
                .selected_text(name(&items[i]).as_str())
                .show_ui(ui, |ui| {
                    for (k, it) in items.iter().enumerate() {
                        let b = ui.add_enabled(
                            blocked(it).is_none(),
                            egui::Button::selectable(k == i, name(it).as_str()),
                        );
                        if b.clicked() {
                            set(k);
                        }
                    }
                });
        }
        if let Some(why) = blocked(&items[i]) {
            ui.label(RichText::new(why).weak());
        }
    });
}

fn folder_c(ui: &mut Ui, repo: &Repo, d: &mut Dialog, folder: &Path, branch: &str) {
    caption(ui, "FOLDER");
    ui.horizontal(|ui| {
        ui.label("In");
        let parent = if branch.is_empty() && d.leaf.is_none() {
            shown(folder).trim_end_matches(['\\', '/']).to_owned()
        } else {
            folder.parent().map(shown).unwrap_or_default()
        };
        let mut text = d.parent.as_deref().map(shown).unwrap_or(parent);
        let w = ui.available_width() - 90.0;
        if crate::widgets::text_field(ui, &mut text, "", w).changed() {
            d.edited = None;
            d.parent = Some(PathBuf::from(text));
        }
        browse(ui, repo, d, folder);
    });
    ui.horizontal(|ui| {
        ui.label("Named");
        let auto = if branch.is_empty() {
            String::new()
        } else {
            folder
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let mut text = d.leaf.clone().unwrap_or(auto);
        if crate::widgets::text_field(ui, &mut text, "the branch's name", 240.0).changed() {
            d.edited = None;
            d.leaf = Some(text);
        }
        if d.leaf.is_some() {
            if ui.link("Follow the branch").clicked() {
                d.leaf = None;
            }
        } else if !branch.is_empty() {
            ui.label(RichText::new("follows the branch").weak());
        }
    });
}

// The rest. ----------------------------------------------------------------------------------

/// A note, not a warning: the folder would show in git status there.
fn note_inside(ui: &mut Ui, wt: &Path, name: &str, pattern: &str) {
    let t = crate::widgets::tones(ui);
    egui::Frame::new()
        .fill(t.on_bg)
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(format!(
                        "This folder is inside {name}, where git status would list it."
                    ))
                    .wrap(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let b = crate::widgets::text_button(ui, "Exclude it").on_hover_text(format!(
                        "Adds {pattern} to .git/info/exclude (only in this repository)"
                    ));
                    if b.clicked() {
                        STATE.with(|s| {
                            s.borrow_mut()
                                .excluded
                                .insert((wt.to_owned(), pattern.to_owned()))
                        });
                        crate::prototype_warnings::toast(format!(
                            "Would add {pattern} to .git/info/exclude"
                        ));
                    }
                });
            });
        });
}

/// The bar that switches between variants, bottom centre, above the dialog.
fn variant_bar(ctx: &egui::Context) {
    let n = VARIANTS.len();
    let v = variant();
    let mut to = None;
    if !ctx.egui_wants_keyboard_input() {
        if ctx.input(|i| i.key_pressed(Key::ArrowLeft)) {
            to = Some((v + n - 1) % n);
        }
        if ctx.input(|i| i.key_pressed(Key::ArrowRight)) {
            to = Some((v + 1) % n);
        }
    }
    egui::Area::new(egui::Id::new("prototype-add-worktree-bar"))
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -40.0))
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgb(30, 30, 30))
                .corner_radius(20)
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let text = |s: &str| RichText::new(s).color(Color32::WHITE);
                        if ui.add(egui::Button::new(text("◀")).frame(false)).clicked() {
                            to = Some((v + n - 1) % n);
                        }
                        let (key, name) = VARIANTS[v];
                        ui.label(text(&format!("Add worktree {key} — {name}")));
                        if ui.add(egui::Button::new(text("▶")).frame(false)).clicked() {
                            to = Some((v + 1) % n);
                        }
                    });
                });
        });
    if let Some(to) = to {
        STATE.with(|s| s.borrow_mut().variant = to);
    }
}

/// Every case of `prototype_add_worktree_demo.sh`'s repository, bottom left.
fn panel(ctx: &egui::Context, repo: &Repo) {
    if !STATE.with(|s| s.borrow().panel) {
        return;
    }
    egui::Window::new("PROTOTYPE: add worktree")
        .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(10.0, -40.0))
        .resizable(false)
        .collapsible(true)
        .default_open(true)
        .show(ctx, |ui| {
            ui.label(
                RichText::new("Opens the dialog on a commit. Nothing runs.")
                    .small()
                    .weak(),
            );
            ui.separator();
            let cases = [
                ("fix/typo", "a free local branch"),
                ("docs/readme", "two free local branches"),
                ("feature/shared", "local and its remote, level"),
                ("origin/feature/remote-only", "only a remote branch"),
                ("release", "checked out in worktree release"),
                ("main", "checked out here"),
                ("main~1", "no branch"),
            ];
            for (rev, what) in cases {
                ui.horizontal(|ui| {
                    if ui.small_button(rev).clicked()
                        && let Some(c) = resolve(repo, rev)
                    {
                        start(repo, c, None);
                    }
                    ui.label(RichText::new(what).small().weak());
                });
            }
            ui.separator();
            ui.label(
                RichText::new(
                    "fix-typo is taken, so fix/typo suggests fix-typo-1.\nOpen demo.worktrees/release to see a linked worktree's suggestions.",
                )
                .small()
                .weak(),
            );
        });
}
