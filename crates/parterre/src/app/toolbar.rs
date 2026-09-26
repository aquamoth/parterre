//! The toolbar, its popovers and the ☰ menu. The toolbar keeps what is used every day; the
//! menu offers all of it again, in the toolbar's order, and the rest besides. Chosen on
//! 2026-09-26; the prototype is on the branch `prototype/menus`.

use std::path::{Path, PathBuf};

use eframe::egui::{
    self, Align, Id, Key, Layout, Margin, Popup, PopupCloseBehavior, RectAlign, Response, RichText,
    Sense, Stroke, Ui, Vec2, vec2,
};
use parterre_core::glyphs::{self, Glyph};
use parterre_core::layout::Direction;
use parterre_core::physics::DragModel;
use parterre_core::recent::same_path;
use parterre_core::revgraph::Simplification;

use super::{ParterreApp, SettingsPage};
use crate::export::Format;
use crate::menu::{self, Mark};
use crate::widgets::{self, tip, tip_explained};

const SHOW: [(Simplification, Glyph, &str); 3] = [
    (
        Simplification::Decorated,
        glyphs::LABELLED,
        "Only commits with a branch or tag, and the merges joining them (TortoiseGit's default)",
    ),
    (
        Simplification::BranchesAndMerges,
        glyphs::BRANCHINGS,
        "Also every fork point and merge",
    ),
    (
        Simplification::AllCommits,
        glyphs::ALL_COMMITS,
        "Every commit",
    ),
];

const DRAG: [(DragModel, Glyph, &str); 3] = [
    (DragModel::Adapt, glyphs::ADAPT, "1"),
    (DragModel::Free, glyphs::FREE, "2"),
    (DragModel::Subtree, glyphs::SUBTREE, "3"),
];

const MENU_ID: &str = "main-menu";
const FILTER_ID: &str = "filter-popover";
const ZOOM_ID: &str = "zoom-popover";
const DRAG_ID: &str = "drag-popover";

/// The popup of the button with `id`, for opening it from elsewhere (the screenshot
/// automation).
pub fn popup_id(name: &str) -> Id {
    let id = match name {
        "menu" => MENU_ID,
        "filter" => FILTER_ID,
        "zoom" => ZOOM_ID,
        _ => DRAG_ID,
    };
    Id::new(id).with("popup")
}

impl ParterreApp {
    pub(super) fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let menu_button =
                widgets::popover_button(ui, Id::new(MENU_ID), Some(glyphs::MENU), false);
            let menu_button = tip(menu_button, "Menu", "");
            Popup::menu(&menu_button).style(menu::style).show(|ui| {
                menu::fit_window(ui, |ui| {
                    ui.set_min_width(menu::MIN_WIDTH);
                    self.main_menu(ui);
                });
            });
            gap(ui);

            let g = &mut self.settings.graph;
            let items = SHOW.map(|(s, glyph, _)| (s, glyph));
            let show = widgets::segmented(ui, g.simplification, &items, |s, r| {
                tip_explained(r, s.label(), "", show_description(s))
            });
            if let Some(s) = show {
                g.simplification = s;
            }
            gap(ui);

            for (on, glyph, name) in [
                (&mut g.show_local_branches, glyphs::LOCAL, "local branches"),
                (
                    &mut g.show_remote_branches,
                    glyphs::REMOTE,
                    "remote branches",
                ),
                (&mut g.show_tags, glyphs::TAGS, "tags"),
            ] {
                let response = widgets::icon_button(ui, glyph, *on);
                let verb = if *on { "Hide" } else { "Show" };
                if tip(response, &format!("{verb} {name}"), "").clicked() {
                    *on = !*on;
                }
            }
            self.pull_requests_button(ui);
            let response = widgets::popover_button(ui, Id::new(FILTER_ID), None, false);
            let response = tip(response, "Filter branches", "");
            popover(&response, RectAlign::BOTTOM_START)
                .show(|ui| menu::fit_window(ui, |ui| self.filter_popover(ui)));

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Right to left from here.
                let response = widgets::popover_button(ui, Id::new(DRAG_ID), None, false);
                let response = tip(response, "Dragging options", "");
                popover(&response, RectAlign::BOTTOM_END)
                    .show(|ui| menu::fit_window(ui, |ui| self.drag_popover(ui)));
                let items = DRAG.map(|(m, glyph, _)| (m, glyph));
                let drag = widgets::segmented(ui, self.settings.net.model, &items, |m, r| {
                    let key = DRAG.iter().find(|d| d.0 == m).map_or("", |d| d.2);
                    tip_explained(r, m.label(), key, m.description())
                });
                if let Some(m) = drag {
                    self.set_drag_model(m);
                }
                gap(ui);

                let overview = self.settings.show_overview;
                let verb = if overview { "Hide" } else { "Show" };
                let response = widgets::icon_button(ui, glyphs::OVERVIEW, overview);
                if tip(response, &format!("{verb} the overview map"), "").clicked() {
                    self.settings.show_overview = !overview;
                }
                let response = widgets::icon_button(ui, glyphs::HEAD, false);
                if tip(response, "Go to HEAD", "Home").clicked() {
                    self.go_to_head();
                }
                let response =
                    widgets::popover_button(ui, Id::new(ZOOM_ID), Some(glyphs::ZOOM), false);
                let response = tip(response, "Zoom", "");
                popover(&response, RectAlign::BOTTOM_END)
                    .show(|ui| menu::fit_window(ui, |ui| self.zoom_popover(ui)));

                // Find, in the middle of what is left.
                let room = ui.available_width();
                ui.allocate_ui_with_layout(
                    vec2(room, widgets::BUTTON),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        // Narrow windows squeeze the field rather than the tools.
                        let width = (room - 16.0).clamp(60.0, 380.0);
                        ui.add_space(((room - width) / 2.0).max(0.0));
                        self.find_field(ui, width);
                    },
                );
            });
        });
    }

    /// Shows or hides open pull requests; greyed out unless `origin` is on GitHub.
    fn pull_requests_button(&mut self, ui: &mut Ui) {
        let available = self.pull_requests.origin().is_some();
        let on = self.pull_requests_active();
        let response = ui
            .add_enabled_ui(available, |ui| {
                widgets::icon_button(ui, glyphs::PULL_REQUEST, on)
            })
            .inner;
        let verb = if on { "Hide" } else { "Show" };
        let explained = match self.pull_requests.error() {
            Some(error) => format!("{PULL_REQUESTS_TIP}\n\nLast try: {error}."),
            None => PULL_REQUESTS_TIP.to_owned(),
        };
        let response = tip_explained(response, &format!("{verb} pull requests"), "", &explained)
            .on_disabled_hover_text(NO_PULL_REQUESTS_TIP);
        if response.clicked() {
            self.toggle_pull_requests();
        }
    }

    fn find_field(&mut self, ui: &mut Ui, width: f32) {
        let id = Id::new("search");
        let focused = ui.memory(|m| m.has_focus(id));
        let t = widgets::tones(ui);
        let stroke = if focused {
            Stroke::new(1.5, t.accent)
        } else {
            Stroke::new(1.0, t.field_line)
        };
        egui::Frame::new()
            .fill(t.field)
            .stroke(stroke)
            .corner_radius(8)
            .inner_margin(Margin {
                left: 8,
                right: 4,
                top: 0,
                bottom: 0,
            })
            .show(ui, |ui| {
                ui.set_width(width - 12.0);
                ui.set_height(28.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                let weak = ui.visuals().weak_text_color();
                let (icon, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                widgets::paint_glyph(ui.painter(), icon, glyphs::SEARCH, weak);
                let searching = !self.search.query.is_empty();
                let hint = width > 220.0;
                let tail = if searching {
                    128.0
                } else if hint {
                    52.0
                } else {
                    0.0
                };
                let edit = egui::TextEdit::singleline(&mut self.search.query)
                    .id(id)
                    .frame(egui::Frame::NONE)
                    .hint_text("Find commits, branches, tags")
                    .desired_width((ui.available_width() - tail).max(40.0));
                let response = ui.add(edit);
                if self.search.request_focus {
                    response.request_focus();
                    self.search.request_focus = false;
                }
                if response.changed() {
                    self.update_search();
                    if !self.search.hits.is_empty() {
                        self.goto_search_hit(true);
                    }
                }
                if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    let back = ui.input(|i| i.modifiers.shift);
                    self.goto_search_hit(!back);
                    response.request_focus();
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if !searching {
                        if !hint {
                            return;
                        }
                        egui::Frame::new()
                            .stroke(Stroke::new(1.0, t.field_line))
                            .corner_radius(4)
                            .inner_margin(Margin::symmetric(4, 0))
                            .show(ui, |ui| ui.label(RichText::new("Ctrl+F").small().weak()));
                        return;
                    }
                    if tip(widgets::mini_button(ui, glyphs::CLOSE), "Clear", "Esc").clicked() {
                        self.search.query.clear();
                        self.update_search();
                    }
                    let next = widgets::mini_button(ui, glyphs::CHEVRON_DOWN);
                    if tip(next, "Next", "Enter").clicked() {
                        self.goto_search_hit(true);
                    }
                    let previous = widgets::mini_button(ui, glyphs::CHEVRON_UP);
                    if tip(previous, "Previous", "Shift+Enter").clicked() {
                        self.goto_search_hit(false);
                    }
                    let n = self.search.hits.len();
                    let text = match self.search.current {
                        Some(c) => format!("{} / {n}", c + 1),
                        None if n == 0 => "None".to_owned(),
                        None => format!("{n} found"),
                    };
                    ui.label(RichText::new(text).small().weak());
                });
            });
    }

    fn filter_popover(&mut self, ui: &mut Ui) {
        ui.set_width(270.0);
        ui.weak("Filter");
        let g = &mut self.settings.graph;
        popover_row(ui, "Current branch only", "Only HEAD's history.", |ui| {
            widgets::switch(ui, &mut g.current_branch_only);
        });
        popover_row(
            ui,
            "First parent only",
            "Follow only first parents: merged side branches without refs disappear.",
            |ui| {
                widgets::switch(ui, &mut g.first_parent_only);
            },
        );
        let width = ui.available_width();
        ui.label("Only branches containing")
            .on_hover_text(REF_FILTER_TIP);
        widgets::text_field(ui, &mut g.ref_filter, "e.g. main, release", width);
        ui.label("Hide branches").on_hover_text(HIDE_TIP);
        widgets::text_field(
            ui,
            &mut g.hide_branches,
            "e.g. pipeline/*, release/*",
            width,
        );
    }

    fn zoom_popover(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            self.zoom_field(ui);
            if tip(
                widgets::icon_button(ui, glyphs::MINUS, false),
                "Zoom out",
                "−",
            )
            .clicked()
            {
                self.zoom_by(0.8);
            }
            if tip(
                widgets::icon_button(ui, glyphs::PLUS, false),
                "Zoom in",
                "+",
            )
            .clicked()
            {
                self.zoom_by(1.0 / 0.8);
            }
            if tip(widgets::text_button(ui, "Fit"), "Fit the whole graph", "F").clicked() {
                self.fit();
            }
            if tip(widgets::text_button(ui, "Reset"), "Zoom to 100%", "0").clicked() {
                self.zoom_by(1.0 / self.view.zoom);
            }
        });
    }

    fn drag_popover(&mut self, ui: &mut Ui) {
        ui.set_width(250.0);
        ui.weak("Dragging");
        let mut remember = self.settings.remember_moves;
        popover_row(ui, "Remember moved nodes", REMEMBER_TIP, |ui| {
            widgets::switch(ui, &mut remember);
        });
        self.set_remember_moves(remember);
        ui.separator();
        let displaced = self.scene.as_ref().is_some_and(|s| s.net.any_displaced());
        let response = ui.add_enabled_ui(displaced, |ui| {
            widgets::text_button(ui, "Return all nodes to layout")
        });
        if tip(response.inner, "Return all nodes to layout", "R").clicked() {
            self.reset_positions();
        }
    }

    fn main_menu(&mut self, ui: &mut Ui) {
        if menu::item(ui, "Open folder…", "Ctrl+O", Mark::None).clicked() {
            self.pick_folder = true;
        }
        // The repository shown is left out: it is open already.
        let open = self.repo.as_ref().map(|r| r.path.clone());
        let recent: Vec<PathBuf> = self
            .recent
            .iter()
            .filter(|p| open.as_deref().is_none_or(|o| !same_path(p, o)))
            .map(Path::to_path_buf)
            .collect();
        let mut picked = None;
        let mut clear = false;
        ui.add_enabled_ui(!recent.is_empty(), |ui| {
            menu::submenu(ui, "Recent folders", |ui| {
                for path in &recent {
                    // The folder it is in, where the shortcut would go, tells same names apart.
                    let (name, place) = super::name_and_place(path);
                    if menu::item(ui, &name, &place, Mark::None).clicked() {
                        picked = Some(path.clone());
                    }
                }
                menu::separator(ui);
                if menu::item(ui, "Clear recent folders", "", Mark::None).clicked() {
                    clear = true;
                }
            });
        });
        if let Some(path) = picked {
            self.open_folder(&path);
        }
        if clear {
            self.recent.clear();
            // The open one comes back, so that it is listed once another is opened.
            if let Some(open) = &open {
                self.recent.add(open);
            }
        }
        let close = ui.add_enabled_ui(self.repo.is_some(), |ui| {
            menu::item(ui, "Close folder", "Ctrl+W", Mark::None)
        });
        if close.inner.clicked() {
            self.close_folder();
        }
        menu::separator(ui);

        let (can_undo, can_redo) = self
            .scene
            .as_ref()
            .map_or((false, false), |s| (s.net.can_undo(), s.net.can_redo()));
        let undo = ui.add_enabled_ui(can_undo, |ui| {
            menu::item(ui, "Undo move", "Ctrl+Z", Mark::None)
        });
        if undo.inner.clicked() {
            self.undo();
        }
        let redo = ui.add_enabled_ui(can_redo, |ui| {
            menu::item(ui, "Redo move", "Ctrl+Shift+Z", Mark::None)
        });
        if redo.inner.clicked() {
            self.redo();
        }
        menu::separator(ui);

        let has_repo = open.is_some();
        let reload = ui.add_enabled_ui(has_repo, |ui| menu::item(ui, "Reload", "F5", Mark::None));
        if reload.inner.clicked() {
            self.reload();
        }
        let auto = self.settings.auto_reload;
        if menu::item(ui, "Reload automatically", "", Mark::Check(auto)).clicked() {
            self.settings.auto_reload = !auto;
        }
        // One item per format rather than a file-type list in the save dialog: rfd doesn't say
        // which type was picked, and macOS shows no list at all.
        ui.add_enabled_ui(has_repo, |ui| {
            menu::submenu(ui, "Export", |ui| {
                for format in Format::ALL {
                    let label = format!("{}…", format.name());
                    if menu::item(ui, &label, "", Mark::None).clicked() {
                        self.export = Some(format);
                    }
                }
            });
        });
        menu::separator(ui);

        // In the toolbar's order.
        menu::submenu(ui, "Show", |ui| {
            let g = &mut self.settings.graph;
            for s in Simplification::ALL {
                if menu::item(ui, s.label(), "", Mark::Radio(g.simplification == s)).clicked() {
                    g.simplification = s;
                }
            }
            menu::separator(ui);
            for (on, label) in [
                (&mut g.show_local_branches, "Local branches"),
                (&mut g.show_remote_branches, "Remote branches"),
                (&mut g.show_tags, "Tags"),
                (&mut g.show_stash, "Stash"),
                (&mut g.show_other_refs, "Other refs"),
            ] {
                if menu::item(ui, label, "", Mark::Check(*on)).clicked() {
                    *on = !*on;
                }
            }
            let available = self.pull_requests.origin().is_some();
            let on = self.pull_requests_active();
            let item = ui.add_enabled_ui(available, |ui| {
                menu::item(ui, "Pull requests", "", Mark::Check(on))
            });
            let item = item.inner.on_disabled_hover_text(NO_PULL_REQUESTS_TIP);
            if item.clicked() {
                self.toggle_pull_requests();
            }
        });
        menu::submenu(ui, "Filter", |ui| {
            let g = &mut self.settings.graph;
            for (on, label) in [
                (&mut g.current_branch_only, "Current branch only"),
                (&mut g.first_parent_only, "First parent only"),
            ] {
                if menu::item(ui, label, "", Mark::Check(*on)).clicked() {
                    *on = !*on;
                }
            }
            menu::separator(ui);
            let more = "Branch filter and hidden branches…";
            if menu::item(ui, more, "", Mark::None).clicked() {
                self.open_settings(SettingsPage::Filters);
            }
        });
        if menu::item(ui, "Find", "Ctrl+F", Mark::None).clicked() {
            self.search.request_focus = true;
        }
        menu::submenu(ui, "Zoom", |ui| {
            if menu::item(ui, "Zoom in", "+", Mark::None).clicked() {
                self.zoom_by(1.0 / 0.8);
            }
            if menu::item(ui, "Zoom out", "−", Mark::None).clicked() {
                self.zoom_by(0.8);
            }
            if menu::item(ui, "Zoom to 100%", "0", Mark::None).clicked() {
                self.zoom_by(1.0 / self.view.zoom);
            }
            if menu::item(ui, "Fit the whole graph", "F", Mark::None).clicked() {
                self.fit();
            }
        });
        if menu::item(ui, "Go to HEAD", "Home", Mark::None).clicked() {
            self.go_to_head();
        }
        let overview = &mut self.settings.show_overview;
        if menu::item(ui, "Overview map", "", Mark::Check(*overview)).clicked() {
            *overview = !*overview;
        }
        menu::submenu(ui, "Drag", |ui| {
            for (m, _, key) in DRAG {
                let mark = Mark::Radio(self.settings.net.model == m);
                if menu::item(ui, m.label(), key, mark).clicked() {
                    self.set_drag_model(m);
                }
            }
            menu::separator(ui);
            let remember = self.settings.remember_moves;
            let clicked = menu::item(ui, "Remember moved nodes", "", Mark::Check(remember));
            if clicked.clicked() {
                self.set_remember_moves(!remember);
            }
            let displaced = self.scene.as_ref().is_some_and(|s| s.net.any_displaced());
            let response = ui.add_enabled_ui(displaced, |ui| {
                menu::item(ui, "Return all nodes to layout", "R", Mark::None)
            });
            if response.inner.clicked() {
                self.reset_positions();
            }
        });
        menu::separator(ui);

        menu::submenu(ui, "Newest commits", |ui| {
            let direction = &mut self.settings.layout.direction;
            for d in Direction::ALL {
                if menu::item(ui, d.label(), "", Mark::Radio(*direction == d)).clicked() {
                    *direction = d;
                }
            }
        });
        let status = &mut self.settings.show_status_bar;
        if menu::item(ui, "Status bar", "", Mark::Check(*status)).clicked() {
            *status = !*status;
        }
        menu::separator(ui);

        if menu::item(ui, "Settings…", "Ctrl+,", Mark::None).clicked() {
            self.open_settings(self.settings_page);
        }
        if menu::item(ui, "Keyboard and mouse", "", Mark::None).clicked() {
            self.show_shortcuts = true;
        }
        if menu::item(ui, "Legend", "", Mark::None).clicked() {
            self.show_legend = true;
        }
        if menu::item(ui, "About parterre", "", Mark::None).clicked() {
            self.show_about = true;
        }
        // For users who start parterre from a file manager or Start menu (TODO question 12).
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(36.0);
            ui.label(
                RichText::new(format!("parterre {}", crate::VERSION))
                    .small()
                    .weak(),
            );
        });
        ui.add_space(2.0);
    }

    /// The zoom level, which can be typed over: "70" or "70%" zooms to 70 %, on Enter or when
    /// the field loses the focus (Esc leaves the zoom as it was).
    fn zoom_field(&mut self, ui: &mut Ui) {
        let id = Id::new("zoom-level");
        let focused = ui.memory(|m| m.has_focus(id));
        let mut shown = format!("{:.0}%", self.view.zoom * 100.0);
        let t = widgets::tones(ui);
        let response = egui::Frame::new()
            .fill(t.field)
            .stroke(Stroke::new(1.0, t.field_line))
            .corner_radius(7)
            .inner_margin(Margin::symmetric(6, 5))
            .show(ui, |ui| {
                let text = if focused {
                    &mut self.zoom_text
                } else {
                    &mut shown
                };
                ui.add(
                    egui::TextEdit::singleline(text)
                        .id(id)
                        .frame(egui::Frame::NONE)
                        .font(egui::FontId::proportional(14.0))
                        .horizontal_align(Align::Center)
                        .desired_width(46.0),
                )
            })
            .inner;
        let response = tip(response, "Type a zoom level", "");
        if response.gained_focus() {
            // Start from the number alone, all of it selected, ready to be typed over.
            self.zoom_text = format!("{:.0}", self.view.zoom * 100.0);
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                let all = egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(self.zoom_text.chars().count()),
                );
                state.cursor.set_char_range(Some(all));
                state.store(ui.ctx(), id);
            }
        }
        if response.lost_focus()
            && !ui.input(|i| i.key_pressed(Key::Escape))
            && let Some(percent) = parse_percent(&self.zoom_text)
        {
            self.zoom_by(percent / 100.0 / self.view.zoom);
        }
    }

    pub(super) fn zoom_by(&mut self, factor: f32) {
        self.view
            .zoom_around(self.canvas, self.canvas.center(), factor);
    }

    pub(super) fn set_remember_moves(&mut self, remember: bool) {
        if remember && !self.settings.remember_moves {
            self.settings.remember_moves = true;
            self.record_moves();
        }
        self.settings.remember_moves = remember;
    }
}

pub(super) const REF_FILTER_TIP: &str = "Only branches and tags whose names contain one of these \
    comma-separated words start history.";
pub(super) const HIDE_TIP: &str = "Leave out branches matching these comma-separated wildcards \
    (* is any text, ? one character; origin/release/1 matches release/*), with the history only \
    they lead to. Branches that a shown branch's history contains stay, and so does the current \
    branch.";
pub(super) const PULL_REQUESTS_TIP: &str = "Open pull requests of origin on GitHub, and a \
    fork's into its parent, as labels on the commits they propose, where those have been \
    fetched. Click one to open it. Asks GitHub only when gh is signed in (gh auth login).";
pub(super) const NO_PULL_REQUESTS_TIP: &str =
    "Pull requests: only for repositories whose origin is on GitHub, for now.";
pub(super) const REMEMBER_TIP: &str =
    "Keep nodes where you moved them, per repository, across runs and relayouts.";

/// "70", "70%" or "70.5 %" as a number of percent.
fn parse_percent(text: &str) -> Option<f32> {
    let number: f32 = text.trim().trim_end_matches('%').trim().parse().ok()?;
    (number.is_finite() && number > 0.0).then_some(number)
}

fn show_description(s: Simplification) -> &'static str {
    SHOW.iter().find(|x| x.0 == s).map_or("", |x| x.2)
}

/// A popover under `button`, open until a click outside it.
fn popover(button: &Response, align: RectAlign) -> Popup<'_> {
    Popup::from_toggle_button_response(button)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .align(align)
        .gap(4.0)
        .style(menu::popover_style)
}

/// A label on the left (explained by `tip`, if any) and a control on the right.
fn popover_row(ui: &mut Ui, label: &str, tip: &str, control: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        let label = ui.label(label);
        if !tip.is_empty() {
            label.on_hover_text(tip);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), control);
    });
}

/// Space between groups of tools.
fn gap(ui: &mut Ui) {
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, 20.0), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range(),
        ui.visuals().widgets.noninteractive.bg_stroke,
    );
    ui.add_space(4.0);
}

#[cfg(test)]
mod tests {
    use super::parse_percent;

    #[test]
    fn zoom_levels_as_typed() {
        assert_eq!(parse_percent("70"), Some(70.0));
        assert_eq!(parse_percent(" 70 % "), Some(70.0));
        assert_eq!(parse_percent("12.5%"), Some(12.5));
        assert_eq!(parse_percent("0"), None);
        assert_eq!(parse_percent("-5"), None);
        assert_eq!(parse_percent("big"), None);
    }
}
