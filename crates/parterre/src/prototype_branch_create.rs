//! THROWAWAY: three native creation-dialog layouts inside the real revision graph.
//! Question: do tracking defaults, editable names and duplicate-tracker warnings feel right?
//! Run scripts/prototype-branch-dialog.sh. Git commands are only previewed, never executed.

use std::cell::RefCell;
use std::collections::HashMap;

use eframe::egui::{self, Align, Color32, Id, Layout, RichText, Ui, vec2};
use parterre_core::git::Git;
use parterre_core::{CommitIx, RefKind, Repo};

use crate::{menu, widgets};

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

#[derive(Default)]
struct State {
    initialized: bool,
    variant: usize,
    case: usize,
    dialog: Option<Dialog>,
    to_log: Option<CommitIx>,
}

struct Remote {
    name: String,
    at_commit: bool,
}

struct Dialog {
    commit: CommitIx,
    remotes: Vec<Remote>,
    remote_names: Vec<String>,
    trackers: HashMap<String, Vec<String>>,
    locals: Vec<String>,
    track: String,
    name: String,
    edited: bool,
    switch: bool,
    checked_name: String,
    checked_track: String,
    error: Option<String>,
    track_error: Option<String>,
    focus_name: bool,
    command_open: bool,
    previewed: bool,
    to_log: bool,
}

const VARIANTS: [&str; 3] = [
    "A · Name first (chosen)",
    "B · Tracking first",
    "C · Side by side",
];
const CASES: [(&str, &str, bool); 6] = [
    ("No branches", "demo-unlabelled", false),
    ("Untracked remotes", "origin/new-topic", false),
    ("Already tracked", "origin/shared-topic", false),
    ("Mixed remotes", "origin/a-already-tracked", false),
    ("Switch to remote", "origin/new-topic", true),
    ("Future upstream", "demo-unlabelled", false),
];

fn enabled() -> bool {
    std::env::var_os("PARTERRE_BRANCH_PROTO").is_some()
}

// Read-only probes for the throwaway form; do not widen the production Git interface.
fn probe(repo: &Repo, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .current_dir(&repo.path)
        .env("LC_ALL", "C")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn free_name(base: &str, locals: &[String]) -> String {
    let occupied = |s: &str| {
        locals
            .iter()
            .any(|b| b == s || b.starts_with(&format!("{s}/")) || s.starts_with(&format!("{b}/")))
    };
    let base = if locals.iter().any(|b| base.starts_with(&format!("{b}/"))) {
        base.replace('/', "-")
    } else {
        base.to_owned()
    };
    if !occupied(&base) {
        return base;
    }
    for n in 2.. {
        let candidate = format!("{base}-{n}");
        if !occupied(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

impl Dialog {
    fn new(repo: &Repo, commit: CommitIx, switch: bool) -> Self {
        let git = Git::new(&repo.path);
        let upstreams = parterre_core::forge::upstreams(&git).unwrap_or_default();
        let mut trackers: HashMap<String, Vec<String>> = HashMap::new();
        for (branch, upstream) in upstreams {
            trackers
                .entry(upstream.trim_start_matches("refs/remotes/").to_owned())
                .or_default()
                .push(branch.trim_start_matches("refs/heads/").to_owned());
        }
        for names in trackers.values_mut() {
            names.sort();
        }
        let mut remote_names: Vec<String> = probe(repo, &["remote"])
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect();
        remote_names.sort_by_key(|s| std::cmp::Reverse(s.len()));
        let locals: Vec<String> = repo
            .refs
            .iter()
            .filter(|r| r.kind == RefKind::LocalBranch)
            .map(|r| r.name.clone())
            .collect();
        let mut remotes: Vec<Remote> = repo
            .refs
            .iter()
            .filter(|r| r.kind == RefKind::RemoteBranch && !r.name.ends_with("/HEAD"))
            .map(|r| Remote {
                name: r.name.clone(),
                at_commit: r.target == commit,
            })
            .collect();
        remotes.sort_by(|a, b| a.name.cmp(&b.name));
        let track = remotes
            .iter()
            .find(|r| r.at_commit && !trackers.contains_key(&r.name))
            .map(|r| r.name.clone())
            .unwrap_or_default();
        let mut d = Self {
            commit,
            remotes,
            remote_names,
            trackers,
            locals,
            track,
            name: String::new(),
            edited: false,
            switch,
            checked_name: String::new(),
            checked_track: String::new(),
            error: None,
            track_error: None,
            focus_name: true,
            command_open: false,
            previewed: false,
            to_log: false,
        };
        d.name = d.suggestion();
        d
    }

    fn tracking_parts(&self) -> Option<(&str, &str)> {
        let typed = self.track.trim();
        self.remote_names.iter().find_map(|remote| {
            typed
                .strip_prefix(&format!("{remote}/"))
                .filter(|branch| !branch.is_empty())
                .map(|branch| (remote.as_str(), branch))
        })
    }

    fn suggestion(&self) -> String {
        self.tracking_parts()
            .map(|(_, branch)| free_name(branch, &self.locals))
            .unwrap_or_default()
    }

    fn validate(&mut self, repo: &Repo) {
        if self.name == self.checked_name && self.track == self.checked_track {
            return;
        }
        self.checked_name = self.name.clone();
        self.checked_track = self.track.clone();
        self.track_error = if self.track.trim().is_empty() {
            None
        } else if let Some((_, branch)) = self.tracking_parts() {
            if branch == "HEAD"
                || probe(repo, &["check-ref-format", &format!("refs/heads/{branch}")]).is_none()
            {
                Some("Enter a valid remote branch name.".to_owned())
            } else {
                None
            }
        } else {
            Some(
                "Use a configured remote followed by a branch name, such as origin/topic."
                    .to_owned(),
            )
        };
        self.error = if self.name.is_empty() {
            None
        } else if self.locals.iter().any(|b| b == &self.name) {
            Some("A local branch with this name already exists.".to_owned())
        } else if self.locals.iter().any(|b| {
            b.starts_with(&format!("{}/", self.name)) || self.name.starts_with(&format!("{b}/"))
        }) {
            Some("This name conflicts with an existing branch path.".to_owned())
        } else if probe(repo, &["check-ref-format", "--branch", &self.name]).is_none() {
            Some("Enter a valid Git branch name.".to_owned())
        } else {
            None
        };
    }

    fn command(&self, repo: &Repo) -> String {
        let name = quote(if self.name.is_empty() {
            "<branch-name>"
        } else {
            &self.name
        });
        let track = self.track.trim();
        let at_commit = self.remotes.iter().any(|r| r.name == track && r.at_commit);
        let start = if at_commit {
            track.to_owned()
        } else {
            repo.commit(self.commit).oid.to_hex()
        };
        let tracking = if at_commit { "--track" } else { "--no-track" };
        let create = if self.switch {
            format!("git switch --create {name} {tracking} {}", quote(&start))
        } else {
            format!("git branch {tracking} {name} {}", quote(&start))
        };
        if track.is_empty() || at_commit {
            return create;
        }
        if self.remotes.iter().any(|r| r.name == track) {
            format!(
                "{create}\ngit branch --set-upstream-to={} {name}",
                quote(&format!("refs/remotes/{track}"))
            )
        } else if let Some((remote, branch)) = self.tracking_parts() {
            format!(
                "{create}\ngit config --local {} {}\ngit config --local {} {}",
                quote(&format!("branch.{}.remote", self.name)),
                quote(remote),
                quote(&format!("branch.{}.merge", self.name)),
                quote(&format!("refs/heads/{branch}"))
            )
        } else {
            create
        }
    }

    fn name_field(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Local branch name").strong());
        let suggestion = self.suggestion();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let width = ui.available_width() - widgets::BUTTON - 4.0;
            let response = widgets::text_field(ui, &mut self.name, "Enter a branch name", width);
            if self.focus_name {
                response.request_focus();
                self.focus_name = false;
            }
            if response.changed() {
                self.edited = true;
                self.previewed = false;
            }
            let reset = ui
                .add_enabled_ui(!suggestion.is_empty() && self.name != suggestion, |ui| {
                    widgets::icon_button(ui, parterre_core::glyphs::RESET, false)
                })
                .inner
                .on_hover_text(format!("Use suggested name: {suggestion}"));
            if reset.clicked() {
                self.name = suggestion;
                self.edited = false;
                self.previewed = false;
            }
        });
        if let Some(error) = &self.error {
            ui.label(RichText::new(error).color(ui.visuals().error_fg_color));
        }
    }

    fn track_field(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Track").strong());
        let before = self.track.clone();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let width = ui.available_width() - 24.0;
            widgets::text_field(ui, &mut self.track, "None", width);
            let id = ui.id().with("prototype-track-branches");
            let chevron = widgets::popover_button(ui, id, None, false);
            egui::Popup::from_toggle_button_response(&chevron)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .align(egui::RectAlign::BOTTOM_END)
                .gap(4.0)
                .style(menu::popover_style)
                .show(|ui| {
                    ui.set_min_width(240.0);
                    if ui
                        .add(egui::Button::selectable(self.track.is_empty(), "None"))
                        .clicked()
                    {
                        self.track.clear();
                        ui.close();
                    }
                    for remote in &self.remotes {
                        if ui
                            .add(egui::Button::selectable(
                                self.track.trim() == remote.name,
                                &remote.name,
                            ))
                            .clicked()
                        {
                            self.track = remote.name.clone();
                            ui.close();
                        }
                    }
                });
        });
        if before != self.track {
            if !self.track.trim().is_empty() && !self.edited {
                self.name = self.suggestion();
            }
            self.previewed = false;
        }
        if let Some(error) = &self.track_error {
            ui.label(RichText::new(error).color(ui.visuals().error_fg_color));
        }
        if let Some(trackers) = self.trackers.get(self.track.trim()) {
            let text = format!(
                "Already tracked by {}. This creates another local branch tracking {}.",
                trackers.join(", "),
                self.track.trim()
            );
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
                    ui.label(RichText::new(text).color(color));
                });
        }
    }
}

fn quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_-.:".contains(&b))
    {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

pub fn node_menu(ui: &mut Ui, repo: &Repo, commit: CommitIx) {
    if enabled() && ui.button("Create branch here…").clicked() {
        STATE.with(|s| s.borrow_mut().dialog = Some(Dialog::new(repo, commit, false)));
        ui.close();
    }
}

fn open_case(state: &mut State, repo: &Repo, case: usize) {
    state.case = case;
    let (_, target, switch) = CASES[case];
    if let Some(commit) = repo.resolve(target) {
        let mut d = Dialog::new(repo, commit, switch);
        if case == 5 {
            d.track = "origin/new-feature".to_owned();
            d.name = d.suggestion();
        }
        state.dialog = Some(d);
    }
}

/// The subtitle from the earlier worktree dialog: one row like a log commit.
fn commit_line(ui: &mut Ui, repo: &Repo, d: &mut Dialog) {
    const SIZE: f32 = 12.0;
    let c = repo.commit(d.commit);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.spacing_mut().interact_size.y = 0.0;
        let weak = ui.visuals().weak_text_color();
        let bar = || {
            RichText::new("|")
                .size(SIZE)
                .color(weak.gamma_multiply(0.6))
        };
        ui.label(RichText::new("at").weak().size(SIZE));
        if ui
            .link(
                RichText::new(c.oid.short(repo.abbrev_len))
                    .monospace()
                    .size(SIZE),
            )
            .on_hover_text("Show in the log")
            .clicked()
        {
            d.to_log = true;
        }
        ui.label(bar());
        let font = egui::FontId::proportional(SIZE);
        let width = |s: &str| {
            ui.painter()
                .layout_no_wrap(s.to_owned(), font.clone(), weak)
                .size()
                .x
        };
        let spacing = ui.spacing().item_spacing.x;
        let right =
            width("|") * 2.0 + width(&c.author_name) + width(&c.author_date) + spacing * 5.0;
        let room = (ui.available_width() - right).max(40.0);
        let height = ui.fonts_mut(|f| f.row_height(&font));
        ui.allocate_ui_with_layout(
            vec2(room, height),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.add(egui::Label::new(RichText::new(&c.subject).size(SIZE)).truncate())
                    .on_hover_text(&c.subject);
            },
        );
        ui.label(bar());
        ui.label(RichText::new(&c.author_name).weak().size(SIZE));
        ui.label(bar());
        ui.label(RichText::new(&c.author_date).weak().size(SIZE));
    });
}

pub fn take_log() -> Option<CommitIx> {
    STATE.with(|s| s.borrow_mut().to_log.take())
}

fn dialog(ctx: &egui::Context, repo: &Repo, d: &mut Dialog, variant: usize) -> bool {
    let mut style = (*ctx.global_style()).clone();
    menu::popover_style(&mut style);
    let frame = egui::Frame::popup(&style)
        .inner_margin(egui::Margin::same(20))
        .corner_radius(12);
    let mut close = false;
    let modal = egui::Modal::new(Id::new("prototype-create-branch"))
        .frame(frame)
        .backdrop_color(Color32::from_black_alpha(if style.visuals.dark_mode {
            90
        } else {
            40
        }))
        .show(ctx, |ui| {
            ui.set_style(style.clone());
            ui.set_width(if variant == 2 { 620.0 } else { 470.0 });
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.heading("Create branch");
            commit_line(ui, repo, d);
            ui.add_space(2.0);
            match variant {
                0 => {
                    d.name_field(ui);
                    ui.add_space(8.0);
                    d.track_field(ui);
                }
                1 => {
                    d.track_field(ui);
                    ui.add_space(8.0);
                    d.name_field(ui);
                }
                _ => {
                    ui.columns(2, |cols| {
                        d.track_field(&mut cols[0]);
                        d.name_field(&mut cols[1]);
                    });
                }
            }
            d.validate(repo);
            ui.add_space(6.0);
            if ui.checkbox(&mut d.switch, "Switch to new branch").changed() {
                d.previewed = false;
            }
            let command = d.command(repo);
            let command_header = egui::CollapsingHeader::new("Git command")
                .id_salt("prototype-command")
                .open(Some(d.command_open))
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.add(
                            egui::Label::new(RichText::new(&command).monospace().small()).wrap(),
                        );
                        if widgets::copy_button(ui, false, widgets::tones(ui).accent).clicked() {
                            ui.ctx().copy_text(command.clone());
                        }
                    });
                });
            if command_header.header_response.clicked() {
                d.command_open = !d.command_open;
            }
            if d.previewed {
                ui.label(
                    RichText::new("Prototype: command previewed; no branch was created.")
                        .color(widgets::tones(ui).accent),
                );
                ui.add(egui::Label::new(RichText::new(&command).monospace().small()).wrap());
            }
            ui.separator();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if d.switch {
                    "Create and switch"
                } else {
                    "Create"
                };
                if ui
                    .add_enabled_ui(
                        !d.name.is_empty() && d.error.is_none() && d.track_error.is_none(),
                        |ui| widgets::primary_button(ui, label, 90.0),
                    )
                    .inner
                    .clicked()
                {
                    d.previewed = true;
                }
                if widgets::text_button(ui, "Cancel").clicked() {
                    close = true;
                }
            });
        });
    close || modal.should_close()
}

pub fn show(ctx: &egui::Context, repo: &Repo) {
    if !enabled() {
        return;
    }
    STATE.with(|cell| {
        let mut s = cell.borrow_mut();
        if !s.initialized {
            s.initialized = true;
            s.variant = match std::env::var("PARTERRE_BRANCH_VARIANT").as_deref() {
                Ok("B") => 1,
                Ok("C") => 2,
                _ => 0,
            };
            let case = std::env::var("PARTERRE_BRANCH_PROTO")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1)
                .min(CASES.len() - 1);
            open_case(&mut s, repo, case);
            if let Some(d) = &mut s.dialog {
                if let Ok(track) = std::env::var("PARTERRE_BRANCH_TRACK") {
                    d.track = track;
                    d.name = d.suggestion();
                }
                if let Ok(name) = std::env::var("PARTERRE_BRANCH_NAME") {
                    d.name = name;
                    d.edited = true;
                }
                d.command_open = std::env::var_os("PARTERRE_BRANCH_COMMAND").is_some();
            }
        }
        let variant = s.variant;
        if let Some(d) = &mut s.dialog {
            let close = dialog(ctx, repo, d, variant);
            let to_log = d.to_log.then_some(d.commit);
            d.to_log = false;
            s.to_log = to_log;
            if close {
                s.dialog = None;
            }
        }
        // Above the modal, visibly separate from the proposed product dialog.
        egui::Area::new(Id::new("branch-prototype-controls"))
            .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -40.0))
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(30, 30, 30))
                    .corner_radius(12)
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        *ui.visuals_mut() = egui::Visuals::dark();
                        ui.visuals_mut().override_text_color = Some(Color32::WHITE);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("PROTOTYPE").small().strong());
                            if ui.button("◀").clicked() {
                                s.variant = (s.variant + 2) % 3;
                            }
                            ui.label(VARIANTS[s.variant]);
                            if ui.button("▶").clicked() {
                                s.variant = (s.variant + 1) % 3;
                            }
                            ui.separator();
                            egui::ComboBox::from_id_salt("prototype-case")
                                .selected_text(CASES[s.case].0)
                                .show_ui(ui, |ui| {
                                    for (i, (label, _, _)) in CASES.iter().enumerate() {
                                        if ui.selectable_label(s.case == i, *label).clicked() {
                                            open_case(&mut s, repo, i);
                                        }
                                    }
                                });
                            if ui.button("Reset / reopen").clicked() {
                                let case = s.case;
                                open_case(&mut s, repo, case);
                            }
                        });
                        if let Some(d) = &s.dialog {
                            ui.label(
                                RichText::new(format!(
                                    "name={:?} · track={} · switch={} · name edited={}",
                                    d.name,
                                    if d.track.is_empty() { "none" } else { &d.track },
                                    d.switch,
                                    d.edited
                                ))
                                .small(),
                            );
                        }
                    });
            });
        if !ctx.egui_wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
                s.variant = (s.variant + 2) % 3;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
                s.variant = (s.variant + 1) % 3;
            }
        }
    });
}
