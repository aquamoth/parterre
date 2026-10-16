//! The settings window neither dims nor blocks the app, and every change applies at once, so
//! the graph behind shows what a setting does.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Align, Layout, RichText, ScrollArea, Stroke, Ui, vec2};
use parterre_core::glyphs;
use parterre_core::layout::{Direction, LayoutOptions, Ranking, Trunk};
use rfd::AsyncFileDialog;

use super::log_window::layout_picker;
use super::toolbar::{
    CURRENT_BRANCH_ONLY_TIP, FIRST_PARENT_ONLY_TIP, HIDE_BRANCHES_TIP, PULL_REQUESTS_TIP,
    REF_FILTER_TIP, REMEMBER_MOVES_TIP, WORKTREES_TIP,
};
use super::viewport;
use super::{ParterreApp, PickFor, privacy};
use crate::dialogs;
use crate::keys;
use crate::settings::{Arrows, EdgeStyle, Look, RepoSettings, Settings};
use crate::settings_file::{self, Imported};
use crate::text_size;
use crate::theme::{BranchColor, ThemeChoice};
use crate::usage::Screen;
use crate::widgets::{self, text_segmented};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsPage {
    #[default]
    Appearance,
    BranchColours,
    Graph,
    Filters,
    Dragging,
    Privacy,
    Advanced,
    Manage,
}

impl SettingsPage {
    const ALL: [SettingsPage; 8] = [
        SettingsPage::Appearance,
        SettingsPage::BranchColours,
        SettingsPage::Graph,
        SettingsPage::Filters,
        SettingsPage::Dragging,
        SettingsPage::Privacy,
        SettingsPage::Advanced,
        SettingsPage::Manage,
    ];

    /// By its label without spaces, in any case.
    pub fn named(name: &str) -> Option<SettingsPage> {
        SettingsPage::ALL
            .into_iter()
            .find(|p| p.label().replace(' ', "").eq_ignore_ascii_case(name))
    }

    fn label(self) -> &'static str {
        match self {
            SettingsPage::Appearance => "Appearance",
            SettingsPage::BranchColours => "Branch colours",
            SettingsPage::Graph => "Graph",
            SettingsPage::Filters => "Filters",
            SettingsPage::Dragging => "Dragging",
            SettingsPage::Privacy => "Privacy",
            SettingsPage::Advanced => "Advanced",
            SettingsPage::Manage => "Manage",
        }
    }
}

const SIDEBAR_WIDTH: f32 = 150.0;
const PAGE_MIN_WIDTH: f32 = 440.0;

const THEME_TIP: &str = "Follow system switches along with the desktop's light or dark mode.";
const TEXT_SIZE_TIP: &str = "The size of the text, buttons and menus in every window. The graph \
    has its own zoom. Also Ctrl+wheel anywhere but over the graph, and Ctrl+plus, minus and 0 \
    in the log, diff and settings windows.";
const ARROWS_TIP: &str = "Which way the arrowheads on edges point.";
const HIGHLIGHT_TIP: &str =
    "Draw the edges of the hovered and selected nodes in the selection colour.";
const OVERVIEW_TIP: &str = "A small map of the whole graph in the bottom-right corner; click or \
    drag in it to move the view.";
const STATUS_TIP: &str = "The bar at the bottom: the selected commit or edge, and how many \
    nodes and commits are shown.";
const DIRECTION_TIP: &str = "The side of the graph the newest commits are on.";
const STASH_TIP: &str = "Show the stash, as a label on the commit it was made on.";
const LAYER_GAP_TIP: &str = "Space between rows of commits (default 30).";
const NODE_GAP_TIP: &str = "Space between neighbouring commits in a row (default 25).";
const EDGE_GAP_TIP: &str = "Room for each edge that passes between the commits of a row.";
const LOG_LAYOUT_TIP: &str = "How the log window arranges its commits, details and changed \
    files. Also in the log window's header.";
const PER_REPOSITORY: &str = "each repository keeps its own filters.";
const EXPORT_TIP: &str = "All settings and the filters of the repository shown, to import on \
    another computer or share with a team. Not window sizes.";
const IMPORT_TIP: &str = "A file exported by parterre. Choose what of it to import.";
const IMPORT_SETTINGS_TIP: &str = "Every setting but the filters and window sizes. Settings the \
    file lacks go back to their defaults.";
const IMPORT_FILTERS_TIP: &str = "The filters in the file, for the repository shown.";
const RESET_TIP: &str = "Every setting back to its default, and every repository's filters.";
const UPDATES_TIP: &str = "Ask GitHub once a day whether a newer release is out. Sends nothing \
    of parterre's own.";
const NO_UPDATES_TIP: &str = "Not in this build: its package manager updates parterre.";
const NO_POSTHOG_TIP: &str = "Not in this build: it sends nothing.";
const INSTALL_ID_TIP: &str = "Sent with the usage statistics, never with a crash report. Quote \
    it to have its data deleted.";

fn settings_viewport() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("settings")
}

impl ParterreApp {
    pub(super) fn open_settings(&mut self, page: SettingsPage) {
        self.settings_page = page;
        self.show_settings = true;
    }

    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            self.settings_viewport.closed();
            self.settings_window_text_size = None;
            self.settings_window_size = None;
            return;
        }
        let least = vec2(SIDEBAR_WIDTH + PAGE_MIN_WIDTH + 56.0, 320.0);
        let size = *self.settings_window_size.get_or_insert_with(|| {
            let [w, h] = self.settings.settings_window.size;
            let screen = ctx.input(|i| i.viewport().monitor_size);
            let size = vec2(w, h).max(least);
            screen.map_or(size, |s| size.min(s * 0.9))
        });
        let builder = viewport::builder("Settings – parterre", size, least, &self.window_icon)
            // winit 0.30 drops these buttons on Windows and macOS only. Maximized on Linux, the
            // page just widens.
            .with_minimize_button(false)
            .with_maximize_button(false);
        let id = settings_viewport();
        self.settings_viewport
            .before_show(ctx, id, Screen::Settings);
        ctx.show_viewport_immediate(id, builder, |ui, class| {
            let input = self.settings_viewport.begin(ui, class, self.window_theme);
            // As in a dialog.
            crate::focus::take_on_click(ui.ctx());
            if let Some(input) = input {
                // Its size is kept in points: it grows and shrinks with the text size.
                let text_size = ui.ctx().zoom_factor();
                if self
                    .settings_window_text_size
                    .replace(text_size)
                    .is_some_and(|shown| shown != text_size)
                {
                    let [w, h] = self.settings.settings_window.size;
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(w, h)));
                } else {
                    input.keep_size(&mut self.settings.settings_window.size);
                }
                if input.close_requested || ui.input_mut(|i| keys::close_window().consume(i)) {
                    self.show_settings = false;
                }
                if ui.input_mut(|i| keys::fetch().consume(i)) {
                    self.fetch(ui.ctx(), id);
                }
                // Embedded, the main window reads the text size input.
                text_size::read_input(ui, &mut self.settings.text_size, true);
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::central_panel(&ui.ctx().global_style()).inner_margin(12))
                .show(ui, |ui| self.settings_contents(ui));
        });
    }

    fn settings_contents(&mut self, ui: &mut Ui) {
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(SIDEBAR_WIDTH);
                let t = widgets::tones(ui);
                let text = ui.visuals().text_color();
                let selection = &mut ui.visuals_mut().selection;
                selection.bg_fill = t.press;
                selection.stroke.color = text;
                for page in SettingsPage::ALL {
                    if page == SettingsPage::Advanced {
                        ui.separator();
                    }
                    let selected = self.settings_page == page;
                    let text = RichText::new(page.label());
                    let item = egui::Button::selectable(selected, text)
                        .min_size(vec2(SIDEBAR_WIDTH, 30.0));
                    if ui.add(item).clicked() {
                        self.settings_page = page;
                    }
                }
            });
            ui.separator();
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                // Else the page would take the row's horizontal layout.
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width().max(PAGE_MIN_WIDTH));
                    self.settings_page_ui(ui);
                });
            });
        });
    }

    fn settings_page_ui(&mut self, ui: &mut Ui) {
        let page = self.settings_page;
        if !matches!(page, SettingsPage::Advanced | SettingsPage::Manage) {
            title(ui, page.label());
        }
        let s = &mut self.settings;
        match page {
            SettingsPage::Appearance => {
                group(ui, |rows| {
                    rows.row("Theme", THEME_TIP, |ui| {
                        text_segmented(ui, &mut s.theme, &ThemeChoice::ALL.map(|t| (t, t.label())));
                    });
                    rows.row("Text size", TEXT_SIZE_TIP, |ui| {
                        let percent = |size: f32| format!("{:.0}%", size * 100.0);
                        egui::ComboBox::from_id_salt("text-size")
                            .selected_text(percent(s.text_size))
                            .show_ui(ui, |ui| {
                                for size in parterre_core::text_size::STEPS {
                                    ui.selectable_value(&mut s.text_size, size, percent(size));
                                }
                            });
                    });
                    rows.row(
                        "Style",
                        "Modern: curved edges bundled into trunks, and wide rows split so siblings \
                     stack. Classic: straight, separate edges, and rows as wide as they need to be.",
                        |ui| {
                            let current = Look::of(s);
                            egui::ComboBox::from_id_salt("look")
                                .selected_text(current.map_or("Custom", Look::label))
                                .show_ui(ui, |ui| {
                                    for look in Look::ALL {
                                        let chosen = current == Some(look);
                                        if ui.selectable_label(chosen, look.label()).clicked() {
                                            look.apply(s);
                                        }
                                    }
                                });
                        },
                    );
                    rows.row("Edges", "", |ui| {
                        text_segmented(
                            ui,
                            &mut s.edge_style,
                            &EdgeStyle::ALL.map(|e| (e, e.label())),
                        );
                    });
                    rows.row("Arrows", ARROWS_TIP, |ui| {
                        combo(ui, "arrows", &mut s.arrows, &Arrows::ALL, Arrows::label);
                    });
                    rows.switch(
                        "Highlight edges of the selection",
                        HIGHLIGHT_TIP,
                        &mut s.highlight_edges,
                    );
                    rows.switch(
                        "Count collapsed commits on edges",
                        "Label each edge with the number of commits hidden in it.",
                        &mut s.show_hidden_counts,
                    );
                    rows.switch("Overview map", OVERVIEW_TIP, &mut s.show_overview);
                    rows.switch("Status bar", STATUS_TIP, &mut s.show_status_bar);
                    rows.switch(
                        "Syntax colour",
                        "Colour code by its language in the diff and blame windows. Their \
                         toolbars toggle it too.",
                        &mut s.syntax_colour,
                    );
                });
                ui.add_space(14.0);
                title(ui, "Log window");
                let log = &mut s.log_window;
                group(ui, |rows| {
                    rows.row("Layout", LOG_LAYOUT_TIP, |ui| {
                        if let Some(layout) = layout_picker(ui, log.layout) {
                            log.layout = layout;
                        }
                        ui.add_space(4.0);
                        ui.weak(log.layout.label());
                    });
                });
            }
            SettingsPage::BranchColours => branch_colours(ui, &mut s.branch_colors),
            SettingsPage::Graph => group(ui, |rows| {
                rows.row("Newest commits", DIRECTION_TIP, |ui| {
                    combo(
                        ui,
                        "direction",
                        &mut s.layout.direction,
                        &Direction::ALL,
                        Direction::label,
                    );
                });
                rows.switch(
                    "Bundle edges into trunks",
                    "Edges running into the same commit share one line where they run in \
                     parallel.",
                    &mut s.layout.concentrate_edges,
                );
                rows.switch(
                    "Reload automatically",
                    "Reload when a commit, checkout or fetch outside parterre changes the \
                     branches, tags or HEAD. F5 reloads by hand.",
                    &mut s.auto_reload,
                );
                rows.switch("Stash", STASH_TIP, &mut s.graph.show_stash);
                rows.switch(
                    "Other refs",
                    "Refs outside heads, remotes and tags, e.g. refs/pull/* or tool checkpoints.",
                    &mut s.graph.show_other_refs,
                );
                rows.switch(
                    "Pull requests",
                    PULL_REQUESTS_TIP,
                    &mut s.graph.show_pull_requests,
                );
                rows.switch("Worktrees", WORKTREES_TIP, &mut s.graph.show_worktrees);
                let tags = s.graph.show_tags;
                rows.row(
                    "Tags make nodes",
                    "When off, a tag alone does not make a commit a node.",
                    |ui| {
                        ui.add_enabled_ui(tags, |ui| {
                            widgets::switch(ui, &mut s.graph.tags_make_nodes)
                        });
                    },
                );
            }),
            SettingsPage::Filters => {
                let repo = self.repo.as_deref().map(RepoSettings::key);
                ui.weak(match repo {
                    Some(key) => format!("For {}: {PER_REPOSITORY}", RepoSettings::name(&key)),
                    None => format!("For the repository shown: {PER_REPOSITORY}"),
                });
                ui.add_space(8.0);
                filters(ui, s);
            }
            SettingsPage::Dragging => {
                let mut remember = s.remember_moves;
                group(ui, |rows| {
                    rows.switch("Remember moved nodes", REMEMBER_MOVES_TIP, &mut remember);
                    rows.switch(
                        "Keep nodes from overlapping",
                        "In Adapt mode, push overlapping nodes apart.",
                        &mut s.net.avoid_overlap,
                    );
                });
                self.set_remember_moves(remember);
            }
            SettingsPage::Privacy => {
                group(ui, |rows| {
                    let available = parterre_telemetry::has_update_check();
                    let tip = if available {
                        UPDATES_TIP
                    } else {
                        NO_UPDATES_TIP
                    };
                    rows.row("Check for updates", tip, |ui| {
                        switch_or_greyed_off(ui, &mut s.check_for_updates, available);
                    });
                });
                ui.add_space(14.0);
                if let Some(error) = privacy_page(ui, &mut self.telemetry) {
                    self.status = Some(super::Note::Error(error));
                }
            }
            SettingsPage::Manage => self.manage_page(ui),
            SettingsPage::Advanced => {
                title(ui, "Upstreams");
                group(ui, |rows| {
                    rows.switch(
                        "Ahead and behind",
                        "Colour the commits between a branch and its upstream, link a rebased \
                         branch to its upstream, and count ahead|behind in the status bar and \
                         the log.",
                        &mut s.graph.show_upstreams,
                    );
                });
                ui.add_space(14.0);
                title(ui, "Spacing");
                let l = &mut s.layout;
                group(ui, |rows| {
                    rows.row(
                        "Vertical placement",
                        "How commits are spread over rows.",
                        |ui| {
                            combo(ui, "ranking", &mut l.ranking, &Ranking::ALL, Ranking::label);
                        },
                    );
                    rows.row(
                        "Default branch",
                        "Where the first-parent line of origin/HEAD goes: one straight line \
                         with each branch on the side that stays narrowest, the same with \
                         branches taking turns, or leftmost and bending like the others.",
                        |ui| {
                            combo(ui, "trunk", &mut l.trunk, &Trunk::ALL, Trunk::label);
                        },
                    );
                    rows.slider(
                        "Between layers",
                        LAYER_GAP_TIP,
                        &mut l.layer_gap,
                        10.0..=120.0,
                    );
                    rows.slider(
                        "Extra for slanted edges",
                        "Widen gaps that long sideways edges cross, so edges stay steep (up to \
                         300).",
                        &mut l.gap_per_span,
                        0.0..=0.5,
                    );
                    rows.slider("Between nodes", NODE_GAP_TIP, &mut l.node_gap, 5.0..=100.0);
                    rows.slider("Between edges", EDGE_GAP_TIP, &mut l.edge_gap, 2.0..=40.0);
                    rows.slider(
                        "Maximum row width",
                        "Rows wider than this are split so siblings stack up. 0 = never.",
                        &mut l.max_layer_width,
                        0.0..=10000.0,
                    );
                });
                ui.add_space(8.0);
                if widgets::text_button(ui, "Default spacing").clicked() {
                    *l = LayoutOptions {
                        direction: l.direction,
                        ranking: l.ranking,
                        trunk: l.trunk,
                        ..LayoutOptions::default()
                    };
                }
                ui.add_space(14.0);
                title(ui, "Dragging physics");
                let n = &mut s.net;
                group(ui, |rows| {
                    rows.slider(
                        "Pull",
                        "How far neighbours are pulled along their edges (Adapt).",
                        &mut n.pull,
                        0.0..=1.0,
                    );
                    rows.slider(
                        "Push",
                        "How strongly, and from how far, nodes push each other away (Adapt).",
                        &mut n.push,
                        0.0..=1.0,
                    );
                    rows.slider(
                        "Wobble",
                        "How much nodes overshoot before they settle (Adapt).",
                        &mut n.wobble,
                        0.0..=1.0,
                    );
                });
            }
        }
    }
}

impl ParterreApp {
    fn manage_page(&mut self, ui: &mut Ui) {
        title(ui, "Export and import");
        group(ui, |rows| {
            rows.row("Settings", EXPORT_TIP, |ui| {
                if widgets::text_button(ui, "Export…").clicked() {
                    self.settings_file_requested = Some(PickFor::ExportSettings);
                }
            });
            rows.row("Settings file", IMPORT_TIP, |ui| {
                if widgets::text_button(ui, "Import…").clicked() {
                    self.settings_file_requested = Some(PickFor::ImportSettings);
                }
            });
        });
        if let Some(note) = &self.settings_note {
            ui.add_space(6.0);
            let color = if note.is_error() {
                ui.visuals().error_fg_color
            } else {
                ui.visuals().weak_text_color()
            };
            ui.label(RichText::new(note.text()).color(color));
        }
        ui.add_space(14.0);
        title(ui, "Reset");
        group(ui, |rows| {
            rows.row("All settings, in every repository", RESET_TIP, |ui| {
                if widgets::text_button(ui, "Reset…").clicked() {
                    self.confirm_reset_settings = true;
                }
            });
        });
    }

    pub(super) fn keep_repo_settings(&mut self) {
        if let Some(repo) = &self.repo {
            let key = RepoSettings::key(repo);
            self.stored
                .keep(key, RepoSettings::of(&self.settings.graph));
        }
    }

    fn repo_settings_name(&self) -> Option<String> {
        let repo = self.repo.as_deref()?;
        Some(RepoSettings::name(&RepoSettings::key(repo)))
    }

    pub(super) fn settings_dialog(&self, what: PickFor, frame: &eframe::Frame) -> AsyncFileDialog {
        let (title, name) = match what {
            PickFor::ImportSettings => ("Import settings", String::new()),
            _ => (
                "Export settings",
                match self.repo_settings_name() {
                    Some(repo) => format!("parterre-{repo}.json"),
                    None => "parterre-settings.json".into(),
                },
            ),
        };
        let mut dialog = AsyncFileDialog::new()
            .set_title(title)
            .set_parent(frame)
            .add_filter("parterre settings", &["json"]);
        if !name.is_empty() {
            dialog = dialog.set_file_name(name);
        }
        if let Some(dir) = &self.settings_dir {
            dialog = dialog.set_directory(dir);
        }
        dialog
    }

    pub(super) fn export_settings(&mut self, mut path: PathBuf) {
        crate::usage::action(crate::usage::Action::ExportSettings);
        if path.extension().is_none() {
            path.set_extension("json");
        }
        self.settings_dir = path.parent().map(Path::to_owned);
        let text = settings_file::export(&self.settings, self.repo.is_some());
        self.settings_note = Some(match std::fs::write(&path, text) {
            Ok(()) => super::Note::Info(format!("Exported to {}", path.display())),
            Err(e) => super::Note::Error(format!("Could not write {}: {e}", path.display())),
        });
    }

    pub(super) fn import_settings(&mut self, path: &Path) {
        self.settings_dir = path.parent().map(Path::to_owned);
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let imported = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|text| settings_file::import(&text, &self.settings));
        match imported {
            Ok(imported) => {
                self.settings_note = None;
                self.settings_import = Some(Import {
                    include_settings: imported.settings.is_some(),
                    include_filters: imported.repository.is_some() && self.repo.is_some(),
                    file: name.into_owned(),
                    imported,
                });
            }
            Err(e) => {
                let note = super::Note::Error(format!("Could not import {name}: {e}"));
                self.settings_note = Some(note);
            }
        }
    }

    pub(super) fn import_settings_dialog(&mut self, ctx: &egui::Context) {
        let repo = self.repo_settings_name();
        let Some(import) = &mut self.settings_import else {
            return;
        };
        let shown = dialogs::Dialog::new("import-settings", "Import settings")
            .screen(crate::usage::Screen::ImportSettings)
            .width(380.0)
            .modal()
            .opener(settings_viewport())
            .show(ctx, |ui| {
                ui.weak(format!("From {}", import.file));
                ui.add_space(8.0);
                if import.imported.settings.is_some() {
                    ui.checkbox(&mut import.include_settings, "Settings")
                        .on_hover_text(IMPORT_SETTINGS_TIP);
                }
                if import.imported.repository.is_some() {
                    let label = match &repo {
                        Some(name) => format!("Filters of {name}"),
                        None => "Filters".to_owned(),
                    };
                    ui.add_enabled_ui(repo.is_some(), |ui| {
                        ui.checkbox(&mut import.include_filters, label)
                            .on_hover_text(IMPORT_FILTERS_TIP)
                            .on_disabled_hover_text("Open a repository to import its filters.");
                    });
                }
                let skipped = import.skipped();
                if !skipped.is_empty() {
                    ui.add_space(8.0);
                    let newer = if import.imported.newer {
                        " (from a newer parterre)"
                    } else {
                        ""
                    };
                    ui.weak(format!("Left out{newer}: {}", skipped.join(", ")));
                }
                ui.add_space(12.0);
                ui.separator();
                let chosen = import.include_settings || import.include_filters;
                dialogs::actions(ui, dialogs::Primary::new("Import").enabled(chosen))
            });
        match shown.inner {
            dialogs::Answer::Primary => {
                if let Some(import) = self.settings_import.take() {
                    self.apply_import(import);
                }
            }
            dialogs::Answer::Cancel => self.settings_import = None,
            dialogs::Answer::Open if shown.should_close() => self.settings_import = None,
            dialogs::Answer::Open => {}
        }
    }

    fn apply_import(&mut self, import: Import) {
        crate::usage::action(crate::usage::Action::ImportSettings);
        let mut parts = Vec::new();
        if import.include_settings
            && let Some(mut settings) = import.imported.settings
        {
            settings.sanitize();
            let remember =
                std::mem::replace(&mut settings.remember_moves, self.settings.remember_moves);
            self.settings = settings;
            self.set_remember_moves(remember);
            parts.push("settings".to_owned());
        }
        if import.include_filters
            && let (Some(repo_settings), Some(name)) =
                (import.imported.repository, self.repo_settings_name())
        {
            repo_settings.apply(&mut self.settings.graph);
            parts.push(format!("the filters of {name}"));
        }
        let note = format!("Imported {} from {}", parts.join(" and "), import.file);
        self.settings_note = Some(super::Note::Info(note));
    }

    pub(super) fn reset_settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.confirm_reset_settings {
            return;
        }
        let shown = dialogs::Dialog::new("reset-settings", "Reset all settings?")
            .screen(crate::usage::Screen::ResetSettings)
            .icon(glyphs::RESET, true)
            .width(380.0)
            .modal()
            .opener(settings_viewport())
            .show(ctx, |ui| {
                ui.label(
                    "Every setting goes back to its default, in every repository. Recent \
                     repositories, remembered moves and what is sent to PostHog are kept.",
                );
                ui.add_space(12.0);
                ui.separator();
                let primary = dialogs::Primary::new("Reset")
                    .destructive(true)
                    .focus_cancel(true);
                dialogs::actions(ui, primary)
            });
        match shown.inner {
            dialogs::Answer::Primary => {
                self.confirm_reset_settings = false;
                crate::usage::action(crate::usage::Action::ResetSettings);
                self.reset_settings();
            }
            dialogs::Answer::Cancel => self.confirm_reset_settings = false,
            dialogs::Answer::Open if shown.should_close() => self.confirm_reset_settings = false,
            dialogs::Answer::Open => {}
        }
    }

    fn reset_settings(&mut self) {
        self.settings = Settings::default();
        self.stored.reset();
        self.settings_note = None;
    }
}

#[derive(Debug)]
pub(super) struct Import {
    file: String,
    imported: Imported,
    include_settings: bool,
    include_filters: bool,
}

impl Import {
    /// What the file has that is left out of the parts chosen.
    fn skipped(&self) -> Vec<&str> {
        let chosen = |name: &&String| {
            (self.include_settings && name.starts_with("settings."))
                || (self.include_filters && name.starts_with("repository."))
        };
        self.imported
            .skipped
            .iter()
            .filter(chosen)
            .map(String::as_str)
            .collect()
    }
}

fn filters(ui: &mut Ui, s: &mut Settings) {
    let g = &mut s.graph;
    group(ui, |rows| {
        rows.switch(
            "Current branch only",
            CURRENT_BRANCH_ONLY_TIP,
            &mut g.current_branch_only,
        );
        rows.switch(
            "First parent only",
            FIRST_PARENT_ONLY_TIP,
            &mut g.first_parent_only,
        );
        rows.row("Branch filter", REF_FILTER_TIP, |ui| {
            text_field(ui, &mut g.ref_filter, "e.g. main, release");
        });
        rows.row("Hide branches", HIDE_BRANCHES_TIP, |ui| {
            text_field(ui, &mut g.hide_branches, "e.g. pipeline/*, release/*");
        });
    });
}

fn switch_or_greyed_off(ui: &mut Ui, on: &mut bool, available: bool) {
    let mut off = false;
    let on = if available { on } else { &mut off };
    ui.add_enabled_ui(available, |ui| widgets::switch(ui, on));
}

/// Returns why the "What parterre sends" page didn't open.
fn privacy_page(ui: &mut Ui, telemetry: &mut privacy::Telemetry) -> Option<String> {
    title(ui, "Sent to PostHog");
    let in_build = parterre_telemetry::has_usage_statistics();
    let dnt = telemetry.do_not_track;
    let at_start = telemetry.crash_reports_at_start;
    let tip = |text| if in_build { text } else { NO_POSTHOG_TIP };
    group(ui, |rows| {
        let answer = telemetry.privacy.answer.as_mut();
        // Shown off without an answer: DO_NOT_TRACK is set, or the build can't send.
        let (mut usage, mut crashes) = (false, false);
        let (usage, crashes, id, available) = match answer {
            Some(a) if in_build && !dnt => (
                &mut a.usage_statistics,
                &mut a.crash_reports,
                Some(a.install_id.clone()),
                true,
            ),
            a => (
                &mut usage,
                &mut crashes,
                a.map(|a| a.install_id.clone()),
                false,
            ),
        };
        rows.row(privacy::USAGE_LABEL, tip(privacy::USAGE_TEXT), |ui| {
            switch_or_greyed_off(ui, usage, available);
        });
        let changed = available && *crashes != at_start;
        rows.row(privacy::CRASHES_LABEL, tip(privacy::CRASHES_TEXT), |ui| {
            switch_or_greyed_off(ui, crashes, available);
            if changed {
                ui.label(RichText::new("from the next start").weak());
            }
        });
        if let Some(id) = id.filter(|_| in_build) {
            rows.row("Install ID", INSTALL_ID_TIP, |ui| {
                let copied_id = egui::Id::new("copied-install-id");
                widgets::copy_on_click(
                    ui,
                    copied_id,
                    || id.clone(),
                    |ui, copied| widgets::text_button(ui, if copied { "Copied" } else { "Copy" }),
                );
                ui.label(RichText::new(id).monospace().weak());
            });
        }
    });
    ui.add_space(8.0);
    if in_build && dnt {
        ui.label(RichText::new("Off: DO_NOT_TRACK is set.").weak());
    }
    let link =
        egui::Button::new(RichText::new("What parterre sends").color(ui.visuals().hyperlink_color))
            .frame(false);
    let link = ui
        .add(link)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(crate::about::PRIVACY);
    if link.clicked() {
        return crate::browser::open(crate::about::PRIVACY).err();
    }
    None
}

fn title(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).strong().size(15.0));
    ui.add_space(6.0);
}

struct Rows<'a> {
    ui: &'a mut Ui,
    row_count: usize,
}

impl Rows<'_> {
    fn row(&mut self, label: &str, tip: &str, control: impl FnOnce(&mut Ui)) {
        if self.row_count > 0 {
            let rect = self.ui.available_rect_before_wrap();
            let stroke = self.ui.visuals().widgets.noninteractive.bg_stroke;
            self.ui
                .painter()
                .hline(rect.x_range(), rect.top(), Stroke::new(1.0, stroke.color));
        }
        self.row_count += 1;
        self.ui.horizontal(|ui| {
            ui.set_min_height(42.0);
            ui.add_space(12.0);
            let label = ui.label(label);
            if !tip.is_empty() {
                label.on_hover_text(tip);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(12.0);
                control(ui);
            });
        });
    }

    fn switch(&mut self, label: &str, tip: &str, on: &mut bool) {
        self.row(label, tip, |ui| {
            widgets::switch(ui, on);
        });
    }

    fn slider(
        &mut self,
        label: &str,
        tip: &str,
        value: &mut f32,
        range: std::ops::RangeInclusive<f32>,
    ) {
        self.row(label, tip, |ui| {
            ui.add(egui::Slider::new(value, range));
        });
    }
}

fn group(ui: &mut Ui, add_rows: impl FnOnce(&mut Rows)) {
    let t = widgets::tones(ui);
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    egui::Frame::new()
        .fill(t.group)
        .stroke(Stroke::new(1.0, stroke.color))
        .corner_radius(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add_rows(&mut Rows { ui, row_count: 0 });
        });
}

fn combo<T: PartialEq + Copy>(
    ui: &mut Ui,
    id: &str,
    value: &mut T,
    all: &[T],
    label: fn(T) -> &'static str,
) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(label(*value))
        .width(170.0)
        .show_ui(ui, |ui| {
            for &v in all {
                ui.selectable_value(value, v, label(v));
            }
        });
}

fn text_field(ui: &mut Ui, text: &mut String, hint: &str) {
    widgets::text_field(ui, text, hint, 210.0);
}

fn branch_colours(ui: &mut Ui, rules: &mut Vec<BranchColor>) {
    ui.weak("The first matching rule wins; the current branch stays red.")
        .on_hover_text(
            "* is any text, ? one character; commas separate wildcards. origin/feature/x \
             matches feature/*.",
        );
    ui.add_space(8.0);
    let (mut swap, mut remove) = (None, None);
    let count = rules.len();
    if count > 0 {
        group(ui, |rows| {
            for (i, rule) in rules.iter_mut().enumerate() {
                rows.row("", "", |ui| {
                    // Left to right, so that Tab visits the row in the order it reads.
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        egui::color_picker::color_edit_button_srgba(
                            ui,
                            &mut rule.color,
                            egui::color_picker::Alpha::Opaque,
                        );
                        let spacing = ui.spacing().item_spacing.x;
                        let width = ui.available_width()
                            - 3.0 * (widgets::MINI_BUTTON_SIZE + spacing)
                            - spacing;
                        widgets::text_field(ui, &mut rule.patterns, "e.g. feature/*", width);
                        let up = ui.add_enabled_ui(i > 0, |ui| {
                            widgets::mini_button(ui, parterre_core::glyphs::CHEVRON_UP)
                        });
                        if up.inner.on_hover_text("Move up").clicked() {
                            swap = Some(i - 1);
                        }
                        let down = ui.add_enabled_ui(i + 1 < count, |ui| {
                            widgets::mini_button(ui, parterre_core::glyphs::CHEVRON_DOWN)
                        });
                        if down.inner.on_hover_text("Move down").clicked() {
                            swap = Some(i);
                        }
                        if widgets::mini_button(ui, parterre_core::glyphs::CLOSE)
                            .on_hover_text("Remove")
                            .clicked()
                        {
                            remove = Some(i);
                        }
                    });
                });
            }
        });
        ui.add_space(8.0);
    }
    if let Some(i) = swap {
        rules.swap(i, i + 1);
    }
    if let Some(i) = remove {
        rules.remove(i);
    }
    if widgets::text_button(ui, "Add rule").clicked() {
        let suggested = BranchColor::SUGGESTED;
        rules.push(BranchColor {
            patterns: String::new(),
            color: suggested[rules.len() % suggested.len()],
        });
    }
}
