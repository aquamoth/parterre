//! The settings window: pages in a sidebar, rows of a label and a control. It neither dims
//! nor blocks the app, and every change applies at once, so the graph behind shows what a
//! setting does.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Align, Layout, RichText, ScrollArea, Stroke, Ui, vec2};
use parterre_core::glyphs;
use parterre_core::layout::{Direction, LayoutOptions, Ranking, Trunk};
use rfd::AsyncFileDialog;

use super::log_window::layout_picker;
use super::toolbar::{HIDE_TIP, REF_FILTER_TIP, REMEMBER_TIP};
use super::{ParterreApp, Picked, privacy};
use crate::dialogs;
use crate::settings::{Arrows, EdgeStyle, Look, RepoSettings, Settings};
use crate::settings_file::{self, Imported};
use crate::text_size;
use crate::theme::{BranchColor, ThemeChoice};
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

    /// The page named `name` (its label, in any case, without spaces), for the screenshot
    /// automation.
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

const SIDEBAR: f32 = 150.0;

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
const LAYER_GAP_TIP: &str = "Space between rows of commits (TortoiseGit: 30).";
const NODE_GAP_TIP: &str = "Space between neighbouring commits in a row (TortoiseGit: 25).";
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
/// The "What parterre sends" page (#260).
const WHAT_PARTERRE_SENDS: &str = "https://github.com/aquamoth/parterre/blob/main/docs/privacy.md";
const PAGE: f32 = 440.0;

impl ParterreApp {
    pub(super) fn open_settings(&mut self, page: SettingsPage) {
        self.settings_page = page;
        self.show_settings = true;
    }

    /// A window of its own, which can be moved anywhere, beside parterre's window too.
    /// (Screenshot runs embed it in the main window instead.)
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            self.settings_window_theme = None;
            self.settings_window_text_size = None;
            self.settings_window_size = None;
            return;
        }
        // As the user left it last time, no larger than the screen: the page scrolls.
        let least = vec2(SIDEBAR + PAGE + 56.0, 320.0);
        let size = *self.settings_window_size.get_or_insert_with(|| {
            let [w, h] = self.settings.settings_window.size;
            let screen = ctx.input(|i| i.viewport().monitor_size);
            let size = vec2(w, h).max(least);
            screen.map_or(size, |s| size.min(s * 0.9))
        });
        let builder = egui::ViewportBuilder::default()
            .with_title("Settings – parterre")
            .with_app_id(crate::settings::APP_ID)
            .with_icon(self.window_icon.clone())
            .with_inner_size(size)
            .with_min_inner_size(least)
            // A dialog: nothing to minimize or maximize. winit 0.30 does this on Windows and
            // macOS only; on Linux (X11 and Wayland) it ignores the buttons, and only a window
            // fixed in size has no maximize. The page takes the window's width, so maximized it
            // is just wide.
            .with_minimize_button(false)
            .with_maximize_button(false);
        let id = egui::ViewportId::from_hash_of("settings");
        crate::usage::screen(ctx, id.0, crate::usage::Screen::Settings);
        // A new window is created after this frame and painted in the next; don't wait for
        // input to bring that about.
        if self.settings_window_theme.is_none() && !ctx.embed_viewports() {
            ctx.request_repaint();
        }
        ctx.show_viewport_immediate(id, builder, |ui, class| {
            super::commands::window_begin(ui);
            if class != egui::ViewportClass::EmbeddedWindow {
                // Its own title bar, too, in parterre's theme.
                if self.settings_window_theme != self.window_theme {
                    self.settings_window_theme = self.window_theme;
                    if let Some(theme) = self.window_theme {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::SetTheme(theme));
                    }
                }
                // Its size in points: it grows and shrinks with the text size.
                let text_size = ui.ctx().zoom_factor();
                if self
                    .settings_window_text_size
                    .replace(text_size)
                    .is_some_and(|shown| shown != text_size)
                {
                    let [w, h] = self.settings.settings_window.size;
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(w, h)));
                } else if let Some(size) = ui.input(|i| i.viewport().inner_rect.map(|r| r.size()))
                    && size.x > 0.0
                    && size.y > 0.0
                {
                    self.settings.settings_window.size = [size.x, size.y];
                }
                let closing = ui.input_mut(|i| {
                    i.viewport().close_requested() || crate::keys::close_window().consume(i)
                });
                if closing {
                    self.show_settings = false;
                }
                if ui.input_mut(|i| crate::keys::fetch().consume(i)) {
                    self.fetch(ui.ctx(), id);
                }
                // Embedded, it is in the main window, which reads the text size input.
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
                ui.set_width(SIDEBAR);
                // The chosen page in grey, as in menus, rather than the selection blue.
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
                    let item =
                        egui::Button::selectable(selected, text).min_size(vec2(SIDEBAR, 30.0));
                    if ui.add(item).clicked() {
                        self.settings_page = page;
                    }
                }
            });
            ui.separator();
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                // (The scroll area would take the row's horizontal layout.)
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width().max(PAGE));
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
                     stack. Classic: as TortoiseGit draws it.",
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
                    rows.row("Edges", "Straight is how TortoiseGit draws them.", |ui| {
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
                    super::toolbar::PULL_REQUESTS_TIP,
                    &mut s.graph.show_pull_requests,
                );
                rows.switch(
                    "Worktrees",
                    super::toolbar::WORKTREES_TIP,
                    &mut s.graph.show_worktrees,
                );
                let tags = s.graph.show_tags;
                rows.row(
                    "Tags make nodes",
                    "When off, a tag alone does not make a commit a node (TortoiseGit's \
                     \"Show all tags\").",
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
                    rows.switch("Remember moved nodes", REMEMBER_TIP, &mut remember);
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
                    // Without `send`, and on Snap and Flatpak, there is nothing to turn on.
                    let available = parterre_telemetry::has_update_check();
                    let tip = if available {
                        UPDATES_TIP
                    } else {
                        NO_UPDATES_TIP
                    };
                    rows.row("Check for updates", tip, |ui| {
                        locked_switch(ui, &mut s.check_for_updates, available);
                    });
                });
                ui.add_space(14.0);
                if let Some(error) = privacy_page(ui, &mut self.telemetry) {
                    self.status = Some((error, true));
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
                        "Widen gaps that long sideways edges cross, so edges stay steep \
                         (TortoiseGit does this, up to 300).",
                        &mut l.gap_per_span,
                        0.0..=0.5,
                    );
                    rows.slider("Between nodes", NODE_GAP_TIP, &mut l.node_gap, 5.0..=100.0);
                    rows.slider("Between edges", EDGE_GAP_TIP, &mut l.edge_gap, 2.0..=40.0);
                    rows.slider(
                        "Maximum row width",
                        "Rows wider than this are split so siblings stack up. 0 = never \
                         (TortoiseGit).",
                        &mut l.max_layer_width,
                        0.0..=10000.0,
                    );
                });
                ui.add_space(8.0);
                if widgets::text_button(ui, "TortoiseGit spacing").clicked() {
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
                    self.settings_file = Some(Picked::ExportSettings);
                }
            });
            rows.row("Settings file", IMPORT_TIP, |ui| {
                if widgets::text_button(ui, "Import…").clicked() {
                    self.settings_file = Some(Picked::ImportSettings);
                }
            });
        });
        if let Some((note, failed)) = &self.settings_note {
            ui.add_space(6.0);
            let color = if *failed {
                ui.visuals().error_fg_color
            } else {
                ui.visuals().weak_text_color()
            };
            ui.label(RichText::new(note).color(color));
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

    /// Keeps the settings of the repository shown with the others'.
    pub(super) fn keep_repo_settings(&mut self) {
        if let Some(repo) = &self.repo {
            let key = RepoSettings::key(repo);
            self.stored
                .keep(key, RepoSettings::of(&self.settings.graph));
        }
    }

    /// The file dialog for exporting or importing settings.
    pub(super) fn settings_dialog(&self, what: Picked, frame: &eframe::Frame) -> AsyncFileDialog {
        let repo = self
            .repo
            .as_deref()
            .map(|r| RepoSettings::name(&RepoSettings::key(r)));
        let (title, name) = match what {
            Picked::ImportSettings => ("Import settings", String::new()),
            _ => (
                "Export settings",
                match repo {
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
        // A name typed without the extension gets it added.
        if path.extension().is_none() {
            path.set_extension("json");
        }
        self.settings_dir = path.parent().map(Path::to_owned);
        let text = settings_file::export(&self.settings, self.repo.is_some());
        self.settings_note = Some(match std::fs::write(&path, text) {
            Ok(()) => (format!("Exported to {}", path.display()), false),
            Err(e) => (format!("Could not write {}: {e}", path.display()), true),
        });
    }

    /// Reads a settings file, and asks what of it to import.
    pub(super) fn import_settings(&mut self, path: &Path) {
        self.settings_dir = path.parent().map(Path::to_owned);
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let imported = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|text| settings_file::import(&text, &self.settings));
        match imported {
            Ok(imported) => {
                self.settings_note = None;
                self.import = Some(Import {
                    settings: imported.settings.is_some(),
                    filters: imported.repository.is_some() && self.repo.is_some(),
                    file: name.into_owned(),
                    imported,
                });
            }
            Err(e) => self.settings_note = Some((format!("Could not import {name}: {e}"), true)),
        }
    }

    /// Asks what of a settings file to import, over the settings window.
    pub(super) fn import_settings_dialog(&mut self, ctx: &egui::Context) {
        let repo = self
            .repo
            .as_deref()
            .map(|r| RepoSettings::name(&RepoSettings::key(r)));
        let Some(import) = &mut self.import else {
            return;
        };
        let shown = dialogs::Dialog::new("import-settings", "Import settings")
            .screen(crate::usage::Screen::ImportSettings)
            .width(380.0)
            .modal()
            .opener(egui::ViewportId::from_hash_of("settings"))
            .show(ctx, |ui| {
                ui.weak(format!("From {}", import.file));
                ui.add_space(8.0);
                if import.imported.settings.is_some() {
                    ui.checkbox(&mut import.settings, "Settings")
                        .on_hover_text(IMPORT_SETTINGS_TIP);
                }
                if import.imported.repository.is_some() {
                    let label = match &repo {
                        Some(name) => format!("Filters of {name}"),
                        None => "Filters".to_owned(),
                    };
                    ui.add_enabled_ui(repo.is_some(), |ui| {
                        ui.checkbox(&mut import.filters, label)
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
                let chosen = import.settings || import.filters;
                dialogs::actions(ui, "Import", chosen, false, false)
            });
        match shown.inner {
            dialogs::Answer::Primary => {
                if let Some(import) = self.import.take() {
                    self.apply_import(import);
                }
            }
            dialogs::Answer::Cancel => self.import = None,
            dialogs::Answer::Open if shown.should_close() => self.import = None,
            dialogs::Answer::Open => {}
        }
    }

    fn apply_import(&mut self, import: Import) {
        crate::usage::action(crate::usage::Action::ImportSettings);
        let mut parts = Vec::new();
        if import.settings
            && let Some(mut settings) = import.imported.settings
        {
            settings.sanitize();
            let remember =
                std::mem::replace(&mut settings.remember_moves, self.settings.remember_moves);
            self.settings = settings;
            self.set_remember_moves(remember);
            parts.push("settings".to_owned());
        }
        if import.filters
            && let (Some(repo_settings), Some(repo)) = (import.imported.repository, &self.repo)
        {
            repo_settings.apply(&mut self.settings.graph);
            let name = RepoSettings::name(&RepoSettings::key(repo));
            parts.push(format!("the filters of {name}"));
        }
        let note = format!("Imported {} from {}", parts.join(" and "), import.file);
        self.settings_note = Some((note, false));
    }

    /// Asks before setting every setting back to its default, over the settings window.
    pub(super) fn reset_settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.confirm_reset_settings {
            return;
        }
        let shown = dialogs::Dialog::new("reset-settings", "Reset all settings?")
            .screen(crate::usage::Screen::ResetSettings)
            .icon(glyphs::RESET, true)
            .width(380.0)
            .modal()
            .opener(egui::ViewportId::from_hash_of("settings"))
            .show(ctx, |ui| {
                ui.label(
                    "Every setting goes back to its default, in every repository. Recent \
                     repositories, remembered moves and what is sent to PostHog are kept.",
                );
                ui.add_space(12.0);
                ui.separator();
                dialogs::actions(ui, "Reset", true, true, true)
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

/// A settings file read, and what of it to import.
#[derive(Debug)]
pub(super) struct Import {
    file: String,
    imported: Imported,
    settings: bool,
    filters: bool,
}

impl Import {
    /// What is left out of the parts chosen.
    fn skipped(&self) -> Vec<&str> {
        let chosen = |name: &&String| {
            (self.settings && name.starts_with("settings."))
                || (self.filters && name.starts_with("repository."))
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
            "Only HEAD's history (TortoiseGit's \"Current branch\").",
            &mut g.current_branch_only,
        );
        rows.switch(
            "First parent only",
            "Follow only first parents: merged side branches without refs disappear.",
            &mut g.first_parent_only,
        );
        rows.row("Branch filter", REF_FILTER_TIP, |ui| {
            text_field(ui, &mut g.ref_filter, "e.g. main, release");
        });
        rows.row("Hide branches", HIDE_TIP, |ui| {
            text_field(ui, &mut g.hide_branches, "e.g. pipeline/*, release/*");
        });
    });
}

/// A switch for `on`, shown off and greyed out unless `available`.
fn locked_switch(ui: &mut Ui, on: &mut bool, available: bool) {
    let mut off = false;
    let on = if available { on } else { &mut off };
    ui.add_enabled_ui(available, |ui| widgets::switch(ui, on));
}

/// Settings › Privacy's *Sent to PostHog* (#227): the two switches, the install ID with
/// *Copy*, and the "What parterre sends" page. Returns why that page didn't open.
fn privacy_page(ui: &mut Ui, telemetry: &mut privacy::Telemetry) -> Option<String> {
    title(ui, "Sent to PostHog");
    let in_build = parterre_telemetry::has_usage_statistics();
    let dnt = telemetry.do_not_track;
    let at_start = telemetry.crash_reports_at_start;
    let tip = |text| if in_build { text } else { NO_POSTHOG_TIP };
    group(ui, |rows| {
        let answer = telemetry.privacy.answer.as_mut();
        // Without an answer (DO_NOT_TRACK has given it, or a build without `send`), off.
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
        rows.row(privacy::USAGE, tip(privacy::USAGE_TEXT), |ui| {
            locked_switch(ui, usage, available);
        });
        let changed = available && *crashes != at_start;
        rows.row(privacy::CRASHES, tip(privacy::CRASHES_TEXT), |ui| {
            locked_switch(ui, crashes, available);
            if changed {
                ui.label(RichText::new("from the next start").weak());
            }
        });
        if let Some(id) = id.filter(|_| in_build) {
            rows.row("Install ID", INSTALL_ID_TIP, |ui| {
                // "Copied" for a moment after a click.
                let copied_id = egui::Id::new("copied-install-id");
                let now = ui.input(|i| i.time);
                let at: Option<f64> = ui.data(|d| d.get_temp(copied_id));
                let copied = at.is_some_and(|at| now - at < 1.5);
                if copied {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(300));
                }
                let label = if copied { "Copied" } else { "Copy" };
                if widgets::text_button(ui, label).clicked() {
                    ui.ctx().copy_text(id.clone());
                    ui.data_mut(|d| d.insert_temp(copied_id, now));
                }
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
        .on_hover_text(WHAT_PARTERRE_SENDS);
    if link.clicked() {
        return crate::browser::open(WHAT_PARTERRE_SENDS).err();
    }
    None
}

fn title(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).strong().size(15.0));
    ui.add_space(6.0);
}

/// Rows of a [`group`].
struct Rows<'a> {
    ui: &'a mut Ui,
    count: usize,
}

impl Rows<'_> {
    /// `label` on the left, explained by `tip` on hover if it isn't empty, and `control` on
    /// the right.
    fn row(&mut self, label: &str, tip: &str, control: impl FnOnce(&mut Ui)) {
        if self.count > 0 {
            let rect = self.ui.available_rect_before_wrap();
            let stroke = self.ui.visuals().widgets.noninteractive.bg_stroke;
            self.ui
                .painter()
                .hline(rect.x_range(), rect.top(), Stroke::new(1.0, stroke.color));
        }
        self.count += 1;
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

/// Rows on a rounded background.
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
            add_rows(&mut Rows { ui, count: 0 });
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
                    // Right to left.
                    if widgets::mini_button(ui, parterre_core::glyphs::CLOSE)
                        .on_hover_text("Remove")
                        .clicked()
                    {
                        remove = Some(i);
                    }
                    let down = ui.add_enabled_ui(i + 1 < count, |ui| {
                        widgets::mini_button(ui, parterre_core::glyphs::CHEVRON_DOWN)
                    });
                    if down.inner.on_hover_text("Move down").clicked() {
                        swap = Some(i);
                    }
                    let up = ui.add_enabled_ui(i > 0, |ui| {
                        widgets::mini_button(ui, parterre_core::glyphs::CHEVRON_UP)
                    });
                    if up.inner.on_hover_text("Move up").clicked() {
                        swap = Some(i - 1);
                    }
                    let width = ui.available_width() - 44.0;
                    widgets::text_field(ui, &mut rule.patterns, "e.g. feature/*", width);
                    egui::color_picker::color_edit_button_srgba(
                        ui,
                        &mut rule.color,
                        egui::color_picker::Alpha::Opaque,
                    );
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
