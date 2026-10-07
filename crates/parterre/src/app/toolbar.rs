//! The toolbar and its popovers: what is used every day. The menu bar (`menu_bar`) offers all
//! of it again, and the rest besides. Chosen on 2026-09-26; the prototype is on the branch
//! `prototype/menus`.

use eframe::egui::{
    self, Align, Id, Key, Layout, Margin, Popup, PopupCloseBehavior, RectAlign, Response, Sense,
    Stroke, Ui, vec2,
};
use parterre_core::glyphs::{self, Glyph};
use parterre_core::physics::DragModel;
use parterre_core::revgraph::Simplification;

use super::ParterreApp;
use crate::keys;
use crate::menu;
use crate::usage::{self, Menu};
use crate::widgets::{self, tip, tip_explained};

const SHOW: [(Simplification, Glyph, &str); 3] = [
    (
        Simplification::Decorated,
        glyphs::LABELLED,
        "Only commits with a branch or tag, and the merges joining them (TortoiseGit's default)",
    ),
    // "Branchings and merges" is in the menu only.
    (
        Simplification::Forks,
        glyphs::BRANCHINGS,
        "Also the commits where their histories fork apart",
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

const FILTER_ID: &str = "filter-popover";
const ZOOM_ID: &str = "zoom-popover";
const DRAG_ID: &str = "drag-popover";

/// The popup of the button with `id`, for opening it from elsewhere (the screenshot
/// automation).
pub fn popup_id(name: &str) -> Id {
    toolbar_button_id(name).with("popup")
}

/// The toolbar button that opens `filter`, `zoom` or `drag`, for a script to click.
pub fn toolbar_button_id(name: &str) -> Id {
    let id = match name {
        "filter" => FILTER_ID,
        "zoom" => ZOOM_ID,
        _ => DRAG_ID,
    };
    Id::new(id)
}

impl ParterreApp {
    pub(super) fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            self.fetch_button(ui);
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
            self.worktrees_button(ui);
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
                if tip(response, "Go to HEAD", &keys::GO_TO_HEAD.label()).clicked() {
                    usage::action(usage::Action::GoToHead);
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

    /// Fetches every remote; greyed out without one, or while git runs.
    fn fetch_button(&mut self, ui: &mut Ui) {
        let blocked = self.fetch_blocked();
        let response = ui
            .add_enabled_ui(blocked.is_none(), |ui| {
                widgets::icon_button(ui, glyphs::FETCH, false)
            })
            .inner;
        let response = tip_explained(response, "Fetch", &keys::fetch().label(), FETCH_TIP)
            .on_disabled_hover_text(blocked.unwrap_or_default());
        if response.clicked() {
            self.fetch(ui.ctx(), egui::ViewportId::ROOT);
        }
    }

    /// Shows or hides the worktrees.
    fn worktrees_button(&mut self, ui: &mut Ui) {
        let on = self.settings.graph.show_worktrees;
        let response = widgets::icon_button(ui, glyphs::FOLDER, on);
        let verb = if on { "Hide" } else { "Show" };
        let response = tip_explained(response, &format!("{verb} worktrees"), "", WORKTREES_TIP);
        if response.clicked() {
            self.settings.graph.show_worktrees = !on;
        }
    }

    fn find_field(&mut self, ui: &mut Ui, width: f32) {
        let n = self.search.hits.len();
        let count = match self.search.current {
            Some(c) => format!("{} / {n}", c + 1),
            None if n == 0 => "None".to_owned(),
            None => format!("{n} found"),
        };
        let find = widgets::Find {
            id: Id::new("search"),
            width,
            hint: "Find commits, branches, tags",
            count: &count,
            keys: &crate::keys::find_field_keys(),
            focus: std::mem::take(&mut self.search.request_focus),
            select: false,
        };
        let searching = !self.search.query.trim().is_empty();
        let found = widgets::find_field(ui, &find, &mut self.search.query);
        if found.changed {
            // Counted once per search: as its first character is typed.
            if !searching && !self.search.query.trim().is_empty() {
                usage::action(usage::Action::Find);
            }
            self.update_search();
            if !self.search.hits.is_empty() {
                self.goto_search_hit(true);
            }
        }
        if found.cleared {
            self.search.query.clear();
            self.update_search();
        }
        if found.next {
            self.goto_search_hit(true);
        }
        if found.previous {
            self.goto_search_hit(false);
        }
    }

    fn filter_popover(&mut self, ui: &mut Ui) {
        usage::menu(ui.ctx(), Menu::Filter);
        ui.set_width(270.0);
        ui.weak("Filter")
            .on_hover_text("Each repository keeps its own filters.");
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
        usage::menu(ui.ctx(), Menu::Zoom);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            self.zoom_field(ui);
            if tip(
                widgets::icon_button(ui, glyphs::MINUS, false),
                "Zoom out",
                &keys::ZOOM_OUT.label(),
            )
            .clicked()
            {
                self.zoom_by(0.8);
            }
            if tip(
                widgets::icon_button(ui, glyphs::PLUS, false),
                "Zoom in",
                &keys::ZOOM_IN.label(),
            )
            .clicked()
            {
                self.zoom_by(1.0 / 0.8);
            }
            let fit = widgets::text_button(ui, "Fit");
            if tip(fit, "Fit the whole graph", &keys::ZOOM_TO_FIT.label()).clicked() {
                usage::action(usage::Action::Fit);
                self.fit();
            }
            let reset = widgets::text_button(ui, "Reset");
            if tip(reset, "Zoom to 100%", &keys::ACTUAL_SIZE.label()).clicked() {
                self.zoom_by(1.0 / self.view.zoom);
            }
        });
    }

    fn drag_popover(&mut self, ui: &mut Ui) {
        usage::menu(ui.ctx(), Menu::Drag);
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
        if tip(response.inner, "Return all nodes to layout", "").clicked() {
            self.reset_positions();
        }
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
pub(super) const WORKTREES_TIP: &str = "The repository's worktrees, marked with a folder: \
    the branches they have checked out, even where hidden, and other worktrees' detached \
    HEADs in a colour of their own. Right-click one to open it.";
const FETCH_TIP: &str = "Fetch every remote, pruning the branches deleted there (git fetch \
    --all --prune). Only remote-tracking branches move.";
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
