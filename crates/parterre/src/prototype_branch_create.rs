//! THROWAWAY: three native creation-dialog layouts inside the real revision graph.
//! Question: do tracking defaults, editable names and duplicate-tracker warnings feel right?
//! Run scripts/prototype-branch-dialog.sh. Git commands are only previewed, never executed.

use std::cell::RefCell;

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
}

struct Remote {
    name: String,
    local_name: String,
    trackers: Vec<String>,
}

struct Dialog {
    commit: CommitIx,
    remotes: Vec<Remote>,
    locals: Vec<String>,
    track: Option<usize>,
    name: String,
    edited: bool,
    switch: bool,
    checked_name: String,
    error: Option<String>,
    focus_name: bool,
    command_open: bool,
    previewed: bool,
}

const VARIANTS: [&str; 3] = ["A · Name first", "B · Tracking first", "C · Side by side"];
const CASES: [(&str, &str, bool); 5] = [
    ("No branches", "demo-unlabelled", false),
    ("Untracked remotes", "origin/new-topic", false),
    ("Already tracked", "origin/shared-topic", false),
    ("Mixed remotes", "origin/a-already-tracked", false),
    ("Switch to remote", "origin/new-topic", true),
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
            .filter(|r| {
                r.kind == RefKind::RemoteBranch && r.target == commit && !r.name.ends_with("/HEAD")
            })
            .map(|r| {
                let mut trackers: Vec<String> = upstreams
                    .iter()
                    .filter(|(_, u)| *u == &r.full_name)
                    .map(|(b, _)| b.trim_start_matches("refs/heads/").to_owned())
                    .collect();
                trackers.sort();
                let local_name = remote_names
                    .iter()
                    .find_map(|remote| r.name.strip_prefix(&format!("{remote}/")))
                    .unwrap_or(&r.name)
                    .to_owned();
                Remote {
                    name: r.name.clone(),
                    local_name,
                    trackers,
                }
            })
            .collect();
        remotes.sort_by(|a, b| a.name.cmp(&b.name));
        let track = remotes.iter().position(|r| r.trackers.is_empty());
        let name = track
            .map(|i| free_name(&remotes[i].local_name, &locals))
            .unwrap_or_default();
        Self {
            commit,
            remotes,
            locals,
            track,
            name,
            edited: false,
            switch,
            checked_name: String::new(),
            error: None,
            focus_name: true,
            command_open: false,
            previewed: false,
        }
    }

    fn suggestion(&self) -> String {
        self.track
            .map(|i| free_name(&self.remotes[i].local_name, &self.locals))
            .unwrap_or_default()
    }

    fn validate(&mut self, repo: &Repo) {
        if self.name == self.checked_name {
            return;
        }
        self.checked_name = self.name.clone();
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
        let start = self
            .track
            .map(|i| self.remotes[i].name.clone())
            .unwrap_or_else(|| repo.commit(self.commit).oid.to_hex());
        let tracking = if self.track.is_some() {
            "--track"
        } else {
            "--no-track"
        };
        if self.switch {
            format!("git switch --create {name} {tracking} {}", quote(&start))
        } else {
            format!("git branch {tracking} {name} {}", quote(&start))
        }
    }

    fn name_field(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Local branch name").strong());
        let response = widgets::text_field(
            ui,
            &mut self.name,
            "Enter a branch name",
            ui.available_width(),
        );
        if self.focus_name {
            response.request_focus();
            self.focus_name = false;
        }
        if response.changed() {
            self.edited = true;
            self.previewed = false;
        }
        let suggestion = self.suggestion();
        if self.edited && !suggestion.is_empty() && self.name != suggestion {
            if widgets::text_button(ui, &format!("Use suggested name: {suggestion}")).clicked() {
                self.name = suggestion;
                self.edited = false;
                self.previewed = false;
            }
        } else {
            ui.label(
                RichText::new("You can choose any unused local name.")
                    .small()
                    .weak(),
            );
        }
        if let Some(error) = &self.error {
            ui.label(RichText::new(error).color(ui.visuals().error_fg_color));
        }
    }

    fn track_field(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Track").strong());
        let before = self.track;
        let selected = self
            .track
            .map(|i| self.remotes[i].name.as_str())
            .unwrap_or("None");
        egui::ComboBox::from_id_salt("prototype-track")
            .selected_text(selected)
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.track, None, "None");
                for (i, remote) in self.remotes.iter().enumerate() {
                    ui.selectable_value(&mut self.track, Some(i), &remote.name);
                }
            });
        if before != self.track {
            if self.track.is_some() && !self.edited {
                self.name = self.suggestion();
            }
            self.previewed = false;
        }
        if let Some(i) = self.track {
            let remote = &self.remotes[i];
            if remote.trackers.is_empty() {
                ui.label(
                    RichText::new("No local branch tracks this remote branch.")
                        .small()
                        .weak(),
                );
            } else {
                let text = format!(
                    "Already tracked by {}. This creates another local branch tracking {}.",
                    remote.trackers.join(", "),
                    remote.name
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
        } else {
            ui.label(
                RichText::new(if self.remotes.is_empty() {
                    "No remote branches point at this commit."
                } else {
                    "The new branch will have no upstream."
                })
                .small()
                .weak(),
            );
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
        state.dialog = Some(Dialog::new(repo, commit, switch));
    }
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
            let commit = repo.commit(d.commit);
            egui::Frame::new()
                .fill(widgets::tones(ui).seg_bg)
                .corner_radius(6)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(commit.oid.short(repo.abbrev_len)).monospace());
                        ui.add(egui::Label::new(&commit.subject).truncate());
                    });
                    ui.label(
                        RichText::new(format!("{} · {}", commit.author_name, commit.author_date))
                            .small()
                            .weak(),
                    );
                });
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
                    .add_enabled_ui(!d.name.is_empty() && d.error.is_none(), |ui| {
                        widgets::primary_button(ui, label, 90.0)
                    })
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
                .min(4);
            open_case(&mut s, repo, case);
            if let Some(d) = &mut s.dialog {
                if let Ok(track) = std::env::var("PARTERRE_BRANCH_TRACK") {
                    d.track = d.remotes.iter().position(|r| r.name == track);
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
        if let Some(d) = &mut s.dialog
            && dialog(ctx, repo, d, variant)
        {
            s.dialog = None;
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
                                    d.track
                                        .map(|i| d.remotes[i].name.as_str())
                                        .unwrap_or("none"),
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
