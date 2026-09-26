//! The settings window: pages in a sidebar, rows of a label and a control. It neither dims
//! nor blocks the app, and every change applies at once, so the graph behind shows what a
//! setting does.

use eframe::egui::{self, Align, Layout, RichText, ScrollArea, Stroke, Ui, vec2};
use parterre_core::layout::{Direction, LayoutOptions, Ranking};

use super::ParterreApp;
use super::log_window::layout_picker;
use super::toolbar::{HIDE_TIP, REF_FILTER_TIP, REMEMBER_TIP};
use crate::settings::{Arrows, EdgeStyle, Look};
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
    Advanced,
}

impl SettingsPage {
    const ALL: [SettingsPage; 6] = [
        SettingsPage::Appearance,
        SettingsPage::BranchColours,
        SettingsPage::Graph,
        SettingsPage::Filters,
        SettingsPage::Dragging,
        SettingsPage::Advanced,
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
            SettingsPage::Advanced => "Advanced",
        }
    }
}

const SIDEBAR: f32 = 150.0;

const THEME_TIP: &str = "Follow system switches along with the desktop's light or dark mode.";
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
            return;
        }
        let builder = egui::ViewportBuilder::default()
            .with_title("Settings – parterre")
            .with_app_id(crate::settings::APP_ID)
            .with_icon(self.window_icon.clone())
            .with_inner_size([SIDEBAR + PAGE + 56.0, 480.0])
            .with_resizable(false)
            // A dialog: nothing to minimize or maximize (maximizing broke its layout). winit
            // 0.30 does this on Windows and macOS only; on Linux (X11 and Wayland) it ignores
            // the buttons, and only the window being fixed in size takes away maximize (winit's
            // own Wayland title bar leaves it out, X11 gets a hint). Minimize stays there.
            .with_minimize_button(false)
            .with_maximize_button(false);
        let id = egui::ViewportId::from_hash_of("settings");
        // A new window is created after this frame and painted in the next; don't wait for
        // input to bring that about.
        if self.settings_window_theme.is_none() && !ctx.embed_viewports() {
            ctx.request_repaint();
        }
        ctx.show_viewport_immediate(id, builder, |ui, class| {
            if class != egui::ViewportClass::EmbeddedWindow {
                // Its own title bar, too, in parterre's theme.
                if self.settings_window_theme != self.window_theme {
                    self.settings_window_theme = self.window_theme;
                    if let Some(theme) = self.window_theme {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::SetTheme(theme));
                    }
                }
                let closing = ui
                    .input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape));
                if closing {
                    self.show_settings = false;
                }
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
                    ui.set_width(PAGE);
                    self.settings_page_ui(ui);
                });
            });
        });
    }

    fn settings_page_ui(&mut self, ui: &mut Ui) {
        let page = self.settings_page;
        if page != SettingsPage::Advanced {
            title(ui, page.label());
        }
        let s = &mut self.settings;
        match page {
            SettingsPage::Appearance => {
                group(ui, |rows| {
                    rows.row("Theme", THEME_TIP, |ui| {
                        text_segmented(ui, &mut s.theme, &ThemeChoice::ALL.map(|t| (t, t.label())));
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
            SettingsPage::Filters => group(ui, |rows| {
                let g = &mut s.graph;
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
            }),
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
            SettingsPage::Advanced => {
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
