//! The local-branch tool. Graph nodes and log rows share the same menu and controller.
//! Slow Git queries and all mutations run on workers; forms retain their selected commit.
//! Its dialogs are modeless windows, opened over the window they were asked from.

use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use eframe::egui::{self, Color32, Id, RichText, Ui, ViewportId, vec2};
use parterre_core::branches::{
    Action, Branches, Cancel, Catalog, Create, CreateDraft, Outcome, Report, Warning, command_text,
};
use parterre_core::{Oid, RefKind, Repo};

use crate::{dialogs, menu, widgets};

#[derive(Clone, Debug)]
pub enum Request {
    Create {
        start: Oid,
        track: Option<String>,
        switch: bool,
    },
    // PROTOTYPE (#163): the branch form in worktree mode.
    AddWorktree {
        start: Oid,
    },
    Run(Action),
}

/// One ref target gets a direct named item; several get the existing app's submenu treatment.
pub fn node_menu(
    ui: &mut Ui,
    repo: &Repo,
    commit: Oid,
    catalog: Option<&Catalog>,
    busy: bool,
    worktrees: bool,
) -> Option<Request> {
    let mut request = branch_menu(ui, repo, commit, catalog, busy);
    // PROTOTYPE (#163): the worktree section, only when worktrees are shown.
    if worktrees && catalog.is_some() {
        menu::separator(ui);
        if ui
            .add_enabled(!busy, egui::Button::new("Add worktree here…"))
            .clicked()
        {
            request = Some(Request::AddWorktree { start: commit });
            ui.close();
        }
        let others: Vec<String> = repo
            .worktrees
            .iter()
            .filter(|w| !w.open && w.head.is_some_and(|h| repo.commit(h).oid == commit))
            .map(|w| folder_name(&w.path))
            .collect();
        for verb in ["Go to worktree", "Delete worktree…"] {
            match others.as_slice() {
                [] => {}
                [one] => {
                    ui.add_enabled(false, egui::Button::new(format!("{verb} {one}")))
                        .on_disabled_hover_text("Prototype: not part of this question");
                }
                many => menu::plain_submenu(ui, verb, |ui| {
                    for name in many {
                        ui.add_enabled(false, egui::Button::new(name));
                    }
                }),
            }
        }
    }
    request
}

fn folder_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn branch_menu(
    ui: &mut Ui,
    repo: &Repo,
    commit: Oid,
    catalog: Option<&Catalog>,
    busy: bool,
) -> Option<Request> {
    let mut request = None;
    menu::separator(ui);
    if ui
        .add_enabled(
            !busy && catalog.is_some(),
            egui::Button::new("Create branch here…"),
        )
        .on_disabled_hover_text(if busy {
            "A Git operation is running"
        } else {
            "Loading branch information"
        })
        .clicked()
    {
        request = Some(Request::Create {
            start: commit,
            track: None,
            switch: false,
        });
        ui.close();
    }
    let Some(catalog) = catalog else {
        return request;
    };
    let refs: Vec<_> = repo
        .refs
        .iter()
        .filter(|r| repo.commit(r.target).oid == commit)
        .filter(|r| match r.kind {
            RefKind::LocalBranch => catalog
                .locals
                .iter()
                .any(|b| b.name == r.name && b.tip == commit),
            RefKind::RemoteBranch => catalog
                .remotes
                .iter()
                .any(|b| b.name == r.name && b.tip == commit),
            _ => false,
        })
        .collect();
    let mut switches: Vec<(String, Request)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if catalog.has_working_tree {
        for r in &refs {
            let candidates: Vec<&str> = match r.kind {
                RefKind::LocalBranch => vec![r.name.as_str()],
                RefKind::RemoteBranch => catalog.trackers(&r.name),
                _ => continue,
            };
            if r.kind == RefKind::RemoteBranch
                && candidates.is_empty()
                && !r.name.ends_with("/HEAD")
            {
                switches.push((
                    r.name.clone(),
                    Request::Create {
                        start: commit,
                        track: Some(r.name.clone()),
                        switch: true,
                    },
                ));
            }
            for name in candidates {
                if catalog.current.as_deref() != Some(name)
                    && !catalog.occupied.contains_key(name)
                    && seen.insert(name.to_owned())
                {
                    switches.push((
                        name.to_owned(),
                        Request::Run(Action::Switch(name.to_owned())),
                    ));
                }
            }
        }
    }
    let local_targets: std::collections::HashSet<_> = switches
        .iter()
        .filter(|(_, request)| matches!(request, Request::Run(Action::Switch(_))))
        .map(|(name, _)| name.clone())
        .collect();
    for (name, request) in &mut switches {
        if matches!(request, Request::Create { .. }) && local_targets.contains(name) {
            name.push_str(" (remote)");
        }
    }
    switches.sort_by(|a, b| a.0.cmp(&b.0));
    target_menu(ui, "Switch to", &switches, busy, &mut request);
    let deletions: Vec<_> = refs
        .iter()
        .filter(|r| {
            r.kind == RefKind::LocalBranch
                && catalog.current.as_ref() != Some(&r.name)
                && !catalog.occupied.contains_key(&r.name)
        })
        .map(|r| {
            (
                r.name.clone(),
                Request::Run(Action::Delete {
                    name: r.name.clone(),
                    tip: commit,
                }),
            )
        })
        .collect();
    target_menu(ui, "Delete branch", &deletions, busy, &mut request);
    request
}

fn target_menu(
    ui: &mut Ui,
    verb: &str,
    targets: &[(String, Request)],
    busy: bool,
    request: &mut Option<Request>,
) {
    let mut item = |ui: &mut Ui, label: String, value: &Request| {
        if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
            *request = Some(value.clone());
            ui.close();
        }
    };
    match targets {
        [] => {}
        [(name, target)] => item(ui, format!("{verb} {name}"), target),
        many => menu::plain_submenu(ui, verb, |ui| {
            for (name, target) in many {
                item(ui, name.clone(), target);
            }
        }),
    }
}

#[derive(Debug)]
struct Form {
    repo: Arc<Repo>,
    catalog: Arc<Catalog>,
    start: Oid,
    draft: CreateDraft,
    switch: bool,
    fresh: bool,
    /// The window it was asked from.
    opener: ViewportId,
    /// PROTOTYPE (#163): `Some` when this form adds a worktree.
    wt: Option<Wt>,
}

// ---------------------------------------------------------------------------------------------
// PROTOTYPE (#163) — throwaway. One form for Create branch and Add worktree: the branch controls
// on top, the worktree section below them only when adding a worktree, and the one checkbox that
// applies (Switch to new branch / Go to new worktree) at the bottom. Nothing runs in worktree
// mode: Add shows what would run.
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
struct Wt {
    root: String,
    name: String,
    name_edited: bool,
    go_to: bool,
}

/// What the branch field in worktree mode amounts to.
enum Checkout<'a> {
    Detached,
    Existing(&'a str),
    New,
}

fn slug(name: &str) -> String {
    name.chars()
        .map(|c| if c == '/' || c == '\\' { '-' } else { c })
        .collect()
}

fn subject_slug(subject: &str) -> String {
    let mut out = String::new();
    for word in subject.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).take(3) {
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(&word.to_lowercase());
    }
    out
}

fn with_separator(mut path: String) -> String {
    if !path.ends_with(std::path::MAIN_SEPARATOR) {
        path.push(std::path::MAIN_SEPARATOR);
    }
    path
}

fn taken(path: &std::path::Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|mut d| d.next().is_some()) || path.is_file()
}

impl Wt {
    fn new(repo: &Repo) -> Self {
        let open = repo.worktrees.iter().find(|w| w.open).map(|w| w.path.clone());
        let main = repo.worktrees.first().map(|w| w.path.clone());
        let base = open.clone().unwrap_or_else(|| repo.path.clone());
        let root = match (&open, &main) {
            (Some(o), Some(m)) if o != m => base.parent().map(PathBuf::from).unwrap_or(base),
            _ => {
                let parent = base.parent().map(PathBuf::from).unwrap_or_default();
                parent.join(format!("{}.worktrees", folder_name(&base)))
            }
        };
        Self {
            root: with_separator(root.display().to_string()),
            name: String::new(),
            name_edited: false,
            go_to: false,
        }
    }

    fn path(&self) -> PathBuf {
        PathBuf::from(&self.root).join(self.name.trim())
    }

    /// Follows the branch: `/` → `-`, the short hash when detached, `-1`, `-2` when taken.
    fn follow(&mut self, branch: &str, short: &str) {
        if self.name_edited {
            return;
        }
        let base = if branch.is_empty() { short.to_owned() } else { slug(branch) };
        let mut name = base.clone();
        let mut i = 1;
        while taken(&PathBuf::from(&self.root).join(&name)) {
            name = format!("{base}-{i}");
            i += 1;
        }
        self.name = name;
    }

    fn name_error(&self) -> Option<String> {
        let name = self.name.trim();
        if name.is_empty() || name == "." || name == ".." {
            return Some("Enter a folder name.".into());
        }
        if self.name_edited && taken(&self.path()) {
            return Some(format!("The folder '{name}' already exists."));
        }
        None
    }
}

impl Form {
    fn new_worktree(repo: Arc<Repo>, catalog: Arc<Catalog>, start: Oid, opener: ViewportId) -> Self {
        let mut form = Self::new(repo.clone(), catalog.clone(), start, None, false, opener);
        // The commit's first free local branch, else a remote one (the draft's own default),
        // else a name made from the subject.
        let free = form.free_locals();
        if let Some(local) = free.first() {
            form.draft.set_name(local.clone());
        } else if form.draft.name().is_empty()
            && let Some(ix) = repo.lookup(&start)
        {
            form.draft.set_name(subject_slug(&repo.commit(ix).subject));
        }
        form.wt = Some(Wt::new(&repo));
        form.follow();
        form
    }

    fn short(&self) -> String {
        let hex = self.start.to_hex();
        hex[..self.repo.abbrev_len.min(hex.len())].to_owned()
    }

    fn free_locals(&self) -> Vec<String> {
        self.catalog
            .locals
            .iter()
            .filter(|b| {
                b.tip == self.start
                    && self.catalog.current.as_deref() != Some(&b.name)
                    && !self.catalog.occupied.contains_key(&b.name)
            })
            .map(|b| b.name.clone())
            .collect()
    }

    fn free_remotes(&self) -> Vec<String> {
        self.catalog
            .remotes
            .iter()
            .filter(|r| r.tip == self.start && !r.name.ends_with("/HEAD"))
            .filter(|r| {
                let local = self.catalog.suggested_name(&r.name);
                !self.catalog.locals.iter().any(|b| b.name == local)
            })
            .map(|r| r.name.clone())
            .collect()
    }

    fn checkout(&self) -> Checkout<'_> {
        let name = self.draft.name().trim();
        if name.is_empty() {
            Checkout::Detached
        } else if self.catalog.locals.iter().any(|b| b.name == name) {
            Checkout::Existing(name)
        } else {
            Checkout::New
        }
    }

    fn follow(&mut self) {
        let short = self.short();
        let branch = self.draft.name().trim().to_owned();
        if let Some(wt) = &mut self.wt {
            wt.follow(&branch, &short);
        }
    }

    fn wt_commands(&self) -> Result<Vec<Vec<String>>, String> {
        let wt = self.wt.as_ref().expect("worktree mode");
        if let Some(e) = wt.name_error() {
            return Err(e);
        }
        let path = wt.path().display().to_string();
        let start = self.start.to_hex();
        let s = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        match self.checkout() {
            Checkout::Detached => Ok(vec![s(&["worktree", "add", "--detach", "--", &path, &start])]),
            Checkout::Existing(name) => {
                let b = self.catalog.locals.iter().find(|b| b.name == name).expect("listed");
                if let Some(at) = self.catalog.occupied.get(name) {
                    Err(format!("{name} is checked out in {}.", at.display()))
                } else if self.catalog.current.as_deref() == Some(name) {
                    Err(format!("{name} is checked out here."))
                } else if b.tip != self.start {
                    Err(format!("A branch {name} already exists at another commit."))
                } else {
                    Ok(vec![s(&["worktree", "add", "--", &path, name])])
                }
            }
            Checkout::New => {
                let mut commands =
                    Branches::commands(&self.catalog, &self.action()).map_err(|e| e.to_string())?;
                commands[0] = s(&["worktree", "add", "--no-track", "-b", self.draft.name().trim(), "--", &path, &start]);
                Ok(commands)
            }
        }
    }

    /// The branch field in worktree mode: a field with a dropdown of the commit's usable branches.
    fn wt_branch_field(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Local branch name").strong());
        let mut choices = self.free_locals();
        choices.extend(self.free_remotes());
        let mut text = self.draft.name().to_owned();
        let response = dialogs::editable_choice(ui, "wt-branch", &mut text, "(detached)", &choices);
        if self.fresh {
            response.request_focus();
        }
        if response.changed() {
            if self.catalog.remotes.iter().any(|r| r.name == text.trim()) {
                // A remote branch becomes a local branch that tracks it, as Switch does.
                self.draft = CreateDraft::new(&self.catalog, self.start, Some(text.trim()));
            } else {
                self.draft.set_name(text);
            }
            self.follow();
        }
        let weak = ui.visuals().weak_text_color();
        match self.checkout() {
            Checkout::Detached => {
                ui.colored_label(weak, format!("Detached HEAD at {}", self.short()));
            }
            Checkout::Existing(name) => {
                let upstream = self
                    .catalog
                    .locals
                    .iter()
                    .find(|b| b.name == name)
                    .and_then(|b| b.upstream.clone());
                let text = match upstream {
                    Some(u) => format!("Existing branch, tracking {u}"),
                    None => "Existing branch".to_owned(),
                };
                ui.colored_label(weak, text);
            }
            Checkout::New => {
                if let Some(error) = self.catalog.name_error(self.draft.name()) {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            }
        }
    }

    fn go_to_box(&mut self, ui: &mut Ui) {
        let wt = self.wt.as_mut().expect("worktree mode");
        ui.checkbox(&mut wt.go_to, "Go to new worktree");
    }

    fn folder_fields(&mut self, ui: &mut Ui) {
        let short = self.short();
        let branch = self.draft.name().trim().to_owned();
        let wt = self.wt.as_mut().expect("worktree mode");
        ui.label(RichText::new("Worktree root").strong());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let width = ui.available_width() - 80.0;
            if widgets::text_field(ui, &mut wt.root, "Folder", width).changed() {
                wt.root = with_separator(wt.root.clone());
                wt.follow(&branch, &short);
            }
            ui.add_enabled(false, egui::Button::new("Browse…"))
                .on_disabled_hover_text("Prototype");
        });
        ui.add_space(6.0);
        ui.label(RichText::new("Worktree name").strong());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let width = ui.available_width() - widgets::BUTTON - 4.0;
            if widgets::text_field(ui, &mut wt.name, "Folder name", width).changed() {
                wt.name_edited = true;
            }
            if ui
                .add_enabled_ui(wt.name_edited, |ui| {
                    widgets::icon_button(ui, parterre_core::glyphs::RESET, false)
                })
                .inner
                .on_hover_text("Follow the branch name")
                .clicked()
            {
                wt.name_edited = false;
                wt.follow(&branch, &short);
            }
        });
        if let Some(e) = wt.name_error() {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
    }
}

impl Form {
    fn new(
        repo: Arc<Repo>,
        catalog: Arc<Catalog>,
        start: Oid,
        prefer: Option<String>,
        switch: bool,
        opener: ViewportId,
    ) -> Self {
        let draft = CreateDraft::new(&catalog, start, prefer.as_deref());
        Self {
            repo,
            catalog,
            start,
            draft,
            switch,
            fresh: true,
            opener,
            wt: None,
        }
    }
    fn action(&self) -> Action {
        Action::Create(Create {
            start: self.start,
            name: self.draft.name().to_owned(),
            track: self.draft.upstream(),
            switch: self.switch,
        })
    }

    /// `busy` while another Git operation runs: it can't start until that one is done.
    fn show(&mut self, ctx: &egui::Context, busy: bool) -> (dialogs::Answer, bool) {
        let mut log = false;
        let worktree = self.wt.is_some();
        let (id, title) = if worktree {
            ("add-worktree", "Add a worktree")
        } else {
            ("create-branch", "Create branch")
        };
        let shown = dialogs::Dialog::new(id, title)
            .opener(self.opener)
            .raise(self.fresh)
            .show(ctx, |ui| {
                let commands = dialogs::fields(ui, |ui| {
                    if let Some(ix) = self.repo.lookup(&self.start) {
                        log = dialogs::commit_line(ui, self.repo.commit(ix), self.repo.abbrev_len);
                    }
                    // The branch controls.
                    if worktree {
                        self.wt_branch_field(ui);
                    } else {
                        self.name_field(ui);
                    }
                    if !worktree || matches!(self.checkout(), Checkout::New) {
                        ui.add_space(8.0);
                        self.track_field(ui);
                    }
                    // The worktree controls.
                    if worktree {
                        ui.separator();
                        self.folder_fields(ui);
                    }
                    // The one checkbox that applies.
                    ui.add_space(8.0);
                    if worktree {
                        self.go_to_box(ui);
                    } else {
                        ui.add_enabled(
                            self.catalog.has_working_tree,
                            egui::Checkbox::new(&mut self.switch, "Switch to new branch"),
                        );
                    }
                    if worktree {
                        self.wt_commands()
                    } else {
                        Branches::commands(&self.catalog, &self.action()).map_err(|e| e.to_string())
                    }
                });
                let shown = commands
                    .as_ref()
                    .map(|cmds| cmds.iter().map(|a| command_text(a)).collect::<Vec<_>>())
                    .unwrap_or_default();
                dialogs::command_box(ui, &shown);
                let label = match (worktree, self.wt.as_ref().is_some_and(|w| w.go_to), self.switch) {
                    (true, true, _) => "Add and go to",
                    (true, false, _) => "Add",
                    (false, _, true) => "Create and switch",
                    (false, _, false) => "Create",
                };
                dialogs::actions(ui, label, commands.is_ok() && !busy, false, false)
            });
        self.fresh = false;
        let mut answer = shown.inner;
        if shown.should_close() && answer == dialogs::Answer::Open {
            answer = dialogs::Answer::Cancel;
        }
        (answer, log)
    }

    fn name_field(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Local branch name").strong());
        let suggestion = self.draft.suggested_name(&self.catalog);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let width = ui.available_width() - widgets::BUTTON - 4.0;
            let mut name = self.draft.name().to_owned();
            let response = widgets::text_field(ui, &mut name, "Enter a branch name", width);
            if self.fresh {
                response.request_focus();
            }
            if response.changed() {
                self.draft.set_name(name);
            }
            if ui
                .add_enabled_ui(
                    !suggestion.is_empty() && self.draft.name() != suggestion,
                    |ui| widgets::icon_button(ui, parterre_core::glyphs::RESET, false),
                )
                .inner
                .on_hover_text(format!("Use suggested name: {suggestion}"))
                .clicked()
            {
                self.draft.restore_suggested_name(&self.catalog);
            }
        });
        if !self.draft.name().is_empty()
            && let Some(error) = self.catalog.name_error(self.draft.name())
        {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    fn track_field(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Track branch").strong());
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), widgets::BUTTON),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let mut selected = self.draft.remote().map(str::to_owned);
                let mut remotes = self.catalog.remote_names.clone();
                remotes.sort();
                let chosen = ui
                    .allocate_ui_with_layout(
                        vec2(110.0, widgets::BUTTON),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| dialogs::choice(ui, "track-remote", &mut selected, "None", &remotes),
                    )
                    .inner;
                if chosen {
                    self.draft.set_remote(selected);
                }
                let width = ui.available_width() - widgets::BUTTON - 4.0;
                ui.allocate_ui_with_layout(
                    vec2(width, widgets::BUTTON),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        let choices = self.draft.remote_branches(&self.catalog);
                        let mut branch = self.draft.track_name().to_owned();
                        let response = ui
                            .add_enabled_ui(self.draft.remote().is_some(), |ui| {
                                dialogs::editable_choice(
                                    ui,
                                    "track-branch",
                                    &mut branch,
                                    "Branch name",
                                    &choices,
                                )
                            })
                            .inner;
                        if response.changed() {
                            self.draft.set_track_name(&self.catalog, branch);
                        }
                    },
                );
                if ui
                    .add_enabled_ui(self.draft.can_restore_track_name(), |ui| {
                        widgets::icon_button(ui, parterre_core::glyphs::RESET, false)
                    })
                    .inner
                    .on_hover_text(format!("Use local branch name: {}", self.draft.name()))
                    .clicked()
                {
                    self.draft.restore_track_name();
                }
            },
        );
        let upstream = self.draft.upstream();
        let track_error =
            if self.draft.remote().is_some() && self.draft.track_name().trim().is_empty() {
                (!self.draft.name().is_empty()).then_some("Enter a branch name to track.")
            } else {
                upstream
                    .as_deref()
                    .and_then(|s| self.catalog.track_error(s))
            };
        if let Some(error) = track_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        let trackers = upstream
            .as_deref()
            .map(|s| self.catalog.trackers(s))
            .unwrap_or_default();
        if !trackers.is_empty() {
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
                    ui.colored_label(
                        color,
                        format!(
                            "Already tracked by {}. This creates another local branch tracking {}.",
                            trackers.join(", "),
                            upstream.as_deref().unwrap_or_default()
                        ),
                    );
                });
        }
    }
}

#[derive(Debug)]
struct Job {
    path: PathBuf,
    label: String,
    opener: ViewportId,
    cancel: Cancel,
    rx: mpsc::Receiver<Outcome>,
}

#[derive(Debug)]
struct Notice {
    id: u64,
    title: String,
    path: PathBuf,
    report: Report,
    error: Option<String>,
    at: f64,
}

/// A warning before losing work, waiting for an answer.
#[derive(Debug)]
struct Loss {
    path: PathBuf,
    warning: Warning,
    fresh: bool,
    opener: ViewportId,
}

#[derive(Debug, Default)]
pub struct Tool {
    repo: Option<Arc<Repo>>,
    pub catalog: Option<Arc<Catalog>>,
    loading: Option<mpsc::Receiver<Result<Catalog, String>>>,
    form: Option<Form>,
    warning: Option<Loss>,
    job: Option<Job>,
    notices: Vec<Notice>,
    next_notice: u64,
    details: Option<u64>,
    pub log_request: Option<(Arc<Repo>, Vec<Oid>, bool)>,
    pub reload: Option<PathBuf>,
}

impl Tool {
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn update(&mut self, ctx: &egui::Context, repo: Option<&Arc<Repo>>) {
        let changed = match (&self.repo, repo) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        if changed {
            let different_path = self.repo.as_ref().map(|r| &r.path) != repo.map(|r| &r.path);
            if different_path {
                self.form = None;
                self.warning = None;
                self.catalog = None;
            }
            self.repo = repo.cloned();
            self.loading = repo.map(|r| {
                let path = r.path.clone();
                let ctx = ctx.clone();
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let result = Catalog::load(&path).map_err(|e| e.to_string());
                    let _ = tx.send(result);
                    ctx.request_repaint();
                });
                rx
            });
        }
        let loaded = self.loading.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("Branch information worker stopped unexpectedly.".into()))
            }
        });
        if let Some(result) = loaded {
            self.loading = None;
            match result {
                Ok(catalog) => {
                    let catalog = Arc::new(catalog);
                    if let Some(form) = &mut self.form {
                        form.catalog = catalog.clone();
                    }
                    self.catalog = Some(catalog);
                    self.prototype_open(ctx);
                }
                Err(e) => {
                    self.catalog = None;
                    if let Some(repo) = &self.repo {
                        self.notice(
                            ctx,
                            repo.path.clone(),
                            "Could not load branch information".into(),
                            Report::default(),
                            Some(e),
                        );
                    }
                }
            }
        }
        let completed = self.job.as_ref().and_then(|job| match job.rx.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Outcome::Failed {
                error: parterre_core::branches::Error::Failed("Git operation worker stopped unexpectedly. Review the reloaded repository before retrying.".into()),
                report: Report::default(),
            }),
        });
        if let Some(result) = completed {
            let job = self.job.take().unwrap();
            self.reload = Some(job.path.clone());
            match result {
                Outcome::Warning(warning)
                    if self.repo.as_ref().is_some_and(|r| r.path == job.path) =>
                {
                    self.warning = Some(Loss {
                        path: job.path,
                        warning,
                        fresh: true,
                        opener: job.opener,
                    });
                }
                Outcome::Warning(_) => self.notice(
                    ctx,
                    job.path,
                    job.label,
                    Report::default(),
                    Some(
                        "Open this repository again to review the commits at risk before retrying."
                            .into(),
                    ),
                ),
                Outcome::Done(report) => self.notice(ctx, job.path, job.label, report, None),
                Outcome::Failed { error, report } => {
                    self.notice(ctx, job.path, job.label, report, Some(error.to_string()))
                }
            }
        }
    }

    /// `opener` is the window it was asked from, where its dialogs open.
    pub fn request(&mut self, ctx: &egui::Context, request: Request, opener: ViewportId) {
        if self.busy() {
            return;
        }
        let Some(repo) = self.repo.clone() else {
            return;
        };
        match request {
            Request::Create {
                start,
                track,
                switch,
            } => {
                if let Some(catalog) = self.catalog.clone() {
                    self.form = Some(Form::new(repo, catalog, start, track, switch, opener));
                }
            }
            Request::AddWorktree { start } => {
                if let Some(catalog) = self.catalog.clone() {
                    self.form = Some(Form::new_worktree(repo, catalog, start, opener));
                }
            }
            Request::Run(action) => self.run(ctx, repo.path.clone(), action, None, opener),
        }
    }

    /// PROTOTYPE (#163): PARTERRE_PROTOTYPE_DIALOG=worktree:<ref> or branch:<ref> opens the form
    /// once, for screenshots.
    fn prototype_open(&mut self, ctx: &egui::Context) {
        static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let Ok(spec) = std::env::var("PARTERRE_PROTOTYPE_DIALOG") else {
            return;
        };
        if DONE.swap(true, std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let Some(repo) = self.repo.clone() else { return };
        let (mode, name) = spec.split_once(':').unwrap_or(("worktree", spec.as_str()));
        let Some(r) = repo.refs.iter().find(|r| r.name == name) else {
            return;
        };
        let start = repo.commit(r.target).oid;
        let request = if mode == "branch" {
            Request::Create { start, track: None, switch: false }
        } else {
            Request::AddWorktree { start }
        };
        self.request(ctx, request, ViewportId::ROOT);
    }

    fn run(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        action: Action,
        approval: Option<Warning>,
        opener: ViewportId,
    ) {
        let (tx, rx) = mpsc::channel();
        let cancel = Cancel::default();
        let worker_cancel = cancel.clone();
        let worker_path = path.clone();
        let ctx = ctx.clone();
        let label = action.label();
        std::thread::spawn(move || {
            let outcome =
                Branches::new(worker_path).execute(action, approval.as_ref(), &worker_cancel);
            let _ = tx.send(outcome);
            ctx.request_repaint();
        });
        self.job = Some(Job {
            path,
            label,
            opener,
            cancel,
            rx,
        });
    }

    fn notice(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        title: String,
        report: Report,
        error: Option<String>,
    ) {
        self.next_notice += 1;
        self.notices.push(Notice {
            id: self.next_notice,
            title,
            path,
            report,
            error,
            at: ctx.input(|i| i.time),
        });
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if let Some(mut form) = self.form.take() {
            let (answer, log) = form.show(ctx, self.busy());
            if log {
                self.log_request = Some((form.repo.clone(), vec![form.start], false));
            }
            match answer {
                dialogs::Answer::Primary if form.wt.is_some() => {
                    let commands = form.wt_commands().unwrap_or_default();
                    let text = commands.iter().map(|a| command_text(a)).collect::<Vec<_>>().join("\n");
                    self.notice(ctx, form.repo.path.clone(), "Prototype: nothing ran".into(), Report::default(), Some(text));
                }
                dialogs::Answer::Primary => self.run(
                    ctx,
                    form.repo.path.clone(),
                    form.action(),
                    None,
                    form.opener,
                ),
                dialogs::Answer::Cancel => {}
                dialogs::Answer::Open => self.form = Some(form),
            }
        }
        if let Some(loss) = self.warning.take() {
            let warning = &loss.warning;
            let count = warning.commits.len();
            let lost = format!("{count} commit{}", if count == 1 { "" } else { "s" });
            let title = match &warning.action {
                Action::Delete { name, .. } => {
                    format!("Delete branch {name} and lose {lost}?")
                }
                _ => format!(
                    "Switch branches and lose {count} detached commit{}?",
                    if count == 1 { "" } else { "s" }
                ),
            };
            const TRIANGLE: parterre_core::glyphs::Glyph = &[parterre_core::glyphs::Part::Path(
                "M12 3 2 21h20ZM12 9v5m0 3v1",
            )];
            let mut show_log = false;
            let busy = self.busy();
            let shown = dialogs::Dialog::new("branch-loss", &title)
                .icon(TRIANGLE, true)
                .opener(loss.opener)
                .raise(loss.fresh)
                .show(ctx, |ui| {
                    ui.label("These commits are not reachable from any surviving branch, tag or worktree.");
                    egui::Frame::new()
                        .fill(widgets::tones(ui).seg_bg)
                        .corner_radius(8)
                        .inner_margin(egui::Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(&lost);
                                show_log = ui.link("Show in log").clicked();
                            });
                        });
                    let commands = warning.commands.iter().map(|a| command_text(a)).collect::<Vec<_>>();
                    dialogs::command_box(ui, &commands);
                    dialogs::actions(
                        ui,
                        if matches!(warning.action, Action::Delete { .. }) { "Delete anyway" } else { "Switch anyway" },
                        !busy, true, loss.fresh,
                    )
                });
            if show_log {
                self.log_request = Some((warning.repo.clone(), warning.commits.clone(), true));
            }
            match shown.inner {
                dialogs::Answer::Primary => {
                    let action = warning.action.clone();
                    self.run(ctx, loss.path, action, Some(loss.warning), loss.opener)
                }
                dialogs::Answer::Cancel => {}
                dialogs::Answer::Open if !shown.should_close() => {
                    self.warning = Some(Loss {
                        fresh: false,
                        ..loss
                    })
                }
                _ => {}
            }
        }
        self.notifications(ctx);
    }

    fn notifications(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        self.notices
            .retain(|n| n.error.is_some() || self.details == Some(n.id) || now - n.at < 5.0);
        if !self.notices.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        egui::Area::new(Id::new("branch-notifications"))
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-16.0, -40.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(360.0);
                if let Some(job) = &self.job {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(&job.label);
                            if ui.button("Cancel").clicked() {
                                job.cancel.cancel();
                            }
                        });
                    });
                }
                let mut remove = None;
                for n in &self.notices {
                    let color = if n.error.is_some() {
                        ui.visuals().error_fg_color
                    } else if ui.visuals().dark_mode {
                        Color32::from_rgb(75, 165, 105)
                    } else {
                        Color32::from_rgb(35, 120, 65)
                    };
                    egui::Frame::popup(ui.style())
                        .stroke(egui::Stroke::new(1.0, color))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if ui.link(RichText::new(&n.title).color(color)).clicked() {
                                    self.details = Some(n.id);
                                }
                                if ui.small_button("×").clicked() {
                                    remove = Some(n.id);
                                }
                            });
                            if let Some(error) = &n.error {
                                ui.add(egui::Label::new(error).wrap());
                            }
                        });
                }
                if let Some(id) = remove {
                    self.notices.retain(|n| n.id != id);
                }
            });
        if let Some(n) = self.notices.iter().find(|n| Some(n.id) == self.details) {
            let shown = dialogs::Dialog::new("git-operation-details", &n.title)
                .width(600.0)
                .show(ctx, |ui| {
                    dialogs::fields(ui, |ui| {
                        ui.weak(n.path.display().to_string());
                        if let Some(error) = &n.error {
                            ui.colored_label(ui.visuals().error_fg_color, error);
                        }
                        for step in &n.report.steps {
                            ui.label(RichText::new(command_text(&step.args)).monospace());
                            if !step.output.is_empty() {
                                ui.add(
                                    egui::Label::new(RichText::new(&step.output).monospace())
                                        .wrap(),
                                );
                            }
                        }
                    });
                    ui.separator();
                    dialogs::actions(ui, "", false, false, false)
                });
            if shown.inner == dialogs::Answer::Cancel || shown.should_close() {
                self.details = None;
            }
        }
    }
}

impl Drop for Tool {
    fn drop(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.cancel();
        }
    }
}
