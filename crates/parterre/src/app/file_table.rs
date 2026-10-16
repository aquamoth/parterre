use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{
    self, Color32, FontId, Galley, Id, Key, Margin, Modifiers, Pos2, Rect, Response, RichText,
    ScrollArea, Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2,
};
use parterre_core::changed_files::{
    ChangedFile, FileColumn, FileOrder, FileStatus, filter_and_sort,
};
use parterre_core::columns::{ColumnWidths, Layout};
use parterre_core::compare::WorkingTree;
use parterre_core::conflicts::{Answer, Conflict};
use parterre_core::file_diff::GITLINK_MODE;
use parterre_core::git::Git;
use parterre_core::text::{capitalized, elide_start, thousands};

use super::column_borders::{self, Columns};
use super::table_style::{CELL_PAD, Colors, HEADING_HEIGHT, cell, colors, heading_background};
use crate::{focus, widgets};

const MAX_DIFFS_WITHOUT_ASKING: usize = 10;
const ROW_HEIGHT: f32 = 23.0;
const BAR_HEIGHT: f32 = 40.0;
const FILTER_WIDTH: f32 = 280.0;
const HEADING_FONT_SIZE: f32 = 12.0;
const SORT_ARROW_ROOM: f32 = 12.0;
const STATUS_BOX_SIZE: f32 = 16.0;
const PATH_MIN_WIDTH: f32 = 120.0;
const ROOMY_PATH_WIDTH: f32 = 260.0;
/// The columns after the path.
const FIXED_COLUMN_WIDTHS: [f32; 4] = [90.0, 100.0, 100.0, 110.0];
const COMPACT_FIXED_COLUMN_WIDTHS: [f32; 4] = [56.0, 78.0, 66.0, 82.0];

mod column {
    pub const PATH: usize = 0;
    pub const EXTENSION: usize = 1;
    pub const STATUS: usize = 2;
    pub const ADDED: usize = 3;
    pub const REMOVED: usize = 4;
}

pub type Listing = Result<Vec<ChangedFile>, String>;

#[derive(Debug, Default)]
pub struct FileTable {
    order: FileOrder,
    filter: String,
    selection: FileSelection,
    pub widths: ColumnWidths,
    /// *Conflicts only* unticked.
    all_files: bool,
    scroll_offset: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct StatusBadge {
    pub letter: char,
    pub filled: bool,
    pub color: Color32,
}

#[derive(Clone, Debug, Default)]
pub struct StatusBadges {
    pub badges: Vec<StatusBadge>,
    /// Shown on hover.
    pub words: String,
}

#[derive(Clone, Copy)]
enum StatusColumn<'a> {
    Names,
    Badges(&'a [StatusBadges]),
    WorkingTree(&'a WorkingTree),
}

impl<'a> StatusColumn<'a> {
    fn badges(self) -> Option<&'a [StatusBadges]> {
        match self {
            StatusColumn::Badges(badges) => Some(badges),
            _ => None,
        }
    }

    fn working_tree(self) -> Option<&'a WorkingTree> {
        match self {
            StatusColumn::WorkingTree(tree) => Some(tree),
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
struct FileSelection {
    list_id: Option<Id>,
    paths: HashSet<String>,
    anchor: Option<String>,
}

impl FileSelection {
    fn select_only(&mut self, path: &str) {
        self.paths = HashSet::from([path.to_owned()]);
        self.anchor = Some(path.to_owned());
    }

    fn click(&mut self, row: usize, modifiers: Modifiers, files: &[ChangedFile], shown: &[usize]) {
        let path = &files[shown[row]].path;
        if modifiers.command {
            if !self.paths.remove(path) {
                self.paths.insert(path.clone());
            }
            self.anchor = Some(path.clone());
            return;
        }
        let anchor = self
            .anchor
            .as_ref()
            .and_then(|a| shown.iter().position(|&i| &files[i].path == a));
        match anchor {
            Some(from) if modifiers.shift => {
                let (first, last) = (from.min(row), from.max(row));
                self.paths = shown[first..=last]
                    .iter()
                    .map(|&i| files[i].path.clone())
                    .collect();
            }
            _ => self.select_only(path),
        }
    }

    fn selected<'f>(
        &self,
        files: &'f [ChangedFile],
        shown: &[usize],
    ) -> impl Iterator<Item = &'f ChangedFile> {
        shown
            .iter()
            .map(move |&i| &files[i])
            .filter(|f| self.paths.contains(&f.path))
    }
}

/// By row of the list as shown.
enum FileClick {
    Select(usize, Modifiers),
    Open(usize),
    ContextMenu(usize),
}

/// By row of the list as shown.
enum MenuPick {
    Open(usize),
    Blame(usize),
    WorkingTree(usize, WorkingTreePick),
}

struct RowInput {
    click: Option<FileClick>,
    pick: Option<MenuPick>,
    /// While the list has the keyboard.
    enter: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkingTreePick {
    MergeTool,
    Answer(Answer),
    Terminal,
    FileSystem,
}

#[derive(Default)]
pub struct TableAction<'f> {
    pub open: Vec<&'f ChangedFile>,
    pub blame: Option<&'f ChangedFile>,
    pub working_tree: Option<(&'f ChangedFile, WorkingTreePick)>,
}

impl FileTable {
    pub fn clear_selection(&mut self) {
        self.selection = FileSelection::default();
    }

    /// `name` tells the window's widgets apart from another window's; the selection and the
    /// scroll position belong to the list `list_id`.
    pub fn show<'f>(
        &mut self,
        ui: &mut Ui,
        name: &str,
        list_id: Id,
        files: Option<&'f Listing>,
        add_to_bar: impl FnOnce(&mut Ui),
    ) -> TableAction<'f> {
        let files = files.map(|l| l.as_deref().map_err(String::as_str));
        self.table(ui, name, list_id, files, StatusColumn::Names, add_to_bar)
    }

    /// Conflicted files show what each side did as their status, and each row's menu has
    /// working-tree items.
    pub fn show_working_tree<'f>(
        &mut self,
        ui: &mut Ui,
        name: &str,
        list_id: Id,
        files: Option<&'f Listing>,
        tree: Option<&WorkingTree>,
        add_to_bar: impl FnOnce(&mut Ui),
    ) -> TableAction<'f> {
        let files = files.map(|l| l.as_deref().map_err(String::as_str));
        let status = tree.map_or(StatusColumn::Names, StatusColumn::WorkingTree);
        self.table(ui, name, list_id, files, status, add_to_bar)
    }

    pub fn show_badged<'f>(
        &mut self,
        ui: &mut Ui,
        name: &str,
        list_id: Id,
        files: &'f [ChangedFile],
        badges: &[StatusBadges],
        add_to_bar: impl FnOnce(&mut Ui),
    ) -> TableAction<'f> {
        debug_assert_eq!(files.len(), badges.len());
        let status = StatusColumn::Badges(badges);
        self.table(ui, name, list_id, Some(Ok(files)), status, add_to_bar)
    }

    fn table<'f>(
        &mut self,
        ui: &mut Ui,
        name: &str,
        list_id: Id,
        files: Option<Result<&'f [ChangedFile], &str>>,
        status: StatusColumn,
        add_to_bar: impl FnOnce(&mut Ui),
    ) -> TableAction<'f> {
        if self.selection.list_id != Some(list_id) {
            self.selection = FileSelection {
                list_id: Some(list_id),
                ..FileSelection::default()
            };
        }
        let c = colors(ui);
        let shown = self.bar(ui, &c, name, files, status.working_tree(), add_to_bar);
        let files = match files {
            None => {
                note(ui, |ui| {
                    ui.weak("Loading…");
                });
                return TableAction::default();
            }
            Some(Err(e)) => {
                note(ui, |ui| {
                    ui.colored_label(c.removed, format!("Could not list the changed files: {e}"));
                });
                return TableAction::default();
            }
            Some(Ok(files)) => files,
        };
        let shown = shown.unwrap_or_default();
        self.headings(ui, &c, name);
        if shown.is_empty() {
            note(ui, |ui| {
                ui.weak(if files.is_empty() {
                    "No changes."
                } else {
                    "No file matches the filter."
                });
            });
            return TableAction::default();
        }
        let input = self.rows(ui, &c, list_id, files, &shown, status);
        self.act(ui, files, &shown, input)
    }

    /// Returns the files listed, filtered and sorted, as indices into them.
    fn bar(
        &mut self,
        ui: &mut Ui,
        c: &Colors,
        name: &str,
        files: Option<Result<&[ChangedFile], &str>>,
        tree: Option<&WorkingTree>,
        add_to_bar: impl FnOnce(&mut Ui),
    ) -> Option<Vec<usize>> {
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), BAR_HEIGHT), Sense::hover());
        heading_background(ui, rect, c);
        let mut bar = ui.new_child(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(8.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        filter_field(&mut bar, &mut self.filter, Id::new((name, "filter")));
        let conflicts = tree.filter(|t| !t.conflicts.is_empty());
        if conflicts.is_some() {
            bar.add_space(8.0);
            let mut only = !self.all_files;
            widgets::tip_explained(
                bar.checkbox(&mut only, "Conflicts only"),
                "Conflicts only",
                "",
                "List only the files with conflicts, as git status shows them under \
                 Unmerged paths. Each drops out of the list once it is resolved.",
            );
            self.all_files = !only;
        } else if matches!(files, Some(Ok(_))) {
            // Ticked again once none is left, so that the next conflicts start filtered.
            self.all_files = false;
        }
        add_to_bar(&mut bar);
        let files = files?.ok()?;
        let mut shown = filter_and_sort(files, &self.filter, self.order);
        if let Some(tree) = conflicts.filter(|_| !self.all_files) {
            shown.retain(|&i| tree.conflict(&files[i].path).is_some());
        }
        let count = if shown.len() == files.len() {
            let n = files.len();
            format!("{} file{}", thousands(n), if n == 1 { "" } else { "s" })
        } else {
            format!(
                "{} of {} files",
                thousands(shown.len()),
                thousands(files.len())
            )
        };
        let weak = ui.visuals().weak_text_color();
        bar.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(count).size(12.0).color(weak));
        });
        Some(shown)
    }

    fn headings(&mut self, ui: &mut Ui, c: &Colors, name: &str) {
        let weak = ui.visuals().weak_text_color();
        let (head, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), HEADING_HEIGHT), Sense::hover());
        heading_background(ui, head, c);
        let compact = is_compact(head.width());
        let layout = file_columns(&self.widths, head.width(), head.left());
        let span = |layout: &Layout, i: usize| {
            Rect::from_x_y_ranges(layout.x[i]..=layout.x[i] + layout.w[i], head.y_range())
        };
        let sorts: Vec<Response> = FileColumn::ALL
            .into_iter()
            .enumerate()
            .map(|(i, column)| {
                let response =
                    ui.interact(span(&layout, i), Id::new((name, "sort", i)), Sense::click());
                if heading(column, compact) != column.title() {
                    response.on_hover_text(column.title())
                } else {
                    response
                }
            })
            .collect();
        // Laid out again after the drag, so that the columns follow the pointer in this frame.
        let border_id = Id::new((name, "columns"));
        let active = column_borders::drag(ui, border_id, head, &columns(&layout), &mut self.widths);
        let layout = file_columns(&self.widths, head.width(), head.left());
        let font = FontId::proportional(HEADING_FONT_SIZE);
        for ((i, column), response) in FileColumn::ALL.into_iter().enumerate().zip(sorts) {
            let rect = span(&layout, i);
            if response.clicked() {
                self.order.click(column);
            }
            let color = if response.hovered() {
                ui.visuals().text_color()
            } else {
                weak
            };
            let max_width = layout.w[i] - 2.0 * CELL_PAD - SORT_ARROW_ROOM;
            let g = cell(ui, heading(column, compact), font.clone(), color, max_width);
            let sorted = self.order.column == column;
            let arrow_room = if sorted { SORT_ARROW_ROOM } else { 0.0 };
            let left = if matches!(column, FileColumn::Added | FileColumn::Removed) {
                rect.right() - CELL_PAD - g.size().x - arrow_room
            } else {
                rect.left() + CELL_PAD
            };
            let text_width = g.size().x;
            ui.painter()
                .galley(pos2(left, rect.center().y - g.size().y / 2.0), g, color);
            if sorted {
                let at = pos2(left + text_width + 7.0, rect.center().y);
                sort_arrow(ui, at, self.order.descending, color);
            }
        }
        column_borders::paint(ui, head, &columns(&layout), active, c);
    }

    fn rows(
        &mut self,
        ui: &mut Ui,
        c: &Colors,
        list_id: Id,
        files: &[ChangedFile],
        shown: &[usize],
        status: StatusColumn,
    ) -> RowInput {
        ui.spacing_mut().item_spacing.y = 0.0;
        let body = egui::TextStyle::Body.resolve(ui.style());
        let text = ui.visuals().text_color();
        let weak = ui.visuals().weak_text_color();
        let tree = status.working_tree();
        let mut click = None;
        let mut pick = None;
        // The rows are one stop for Tab, which the keys move the selection in, from the row
        // clicked last.
        let stop = Id::new(("files", list_id, "stop"));
        let anchor_row = self
            .selection
            .anchor
            .as_ref()
            .and_then(|a| shown.iter().position(|&i| &files[i].path == a));
        let height = ui.available_height();
        let rows_per_page = ((height / ROW_HEIGHT).floor() as usize).saturating_sub(1);
        let keys = widgets::list_keys(ui, stop, anchor_row, shown.len(), rows_per_page);
        let mut area = ScrollArea::vertical()
            .id_salt(("files", list_id))
            .auto_shrink(false);
        if let Some(row) = keys.select {
            click = Some(FileClick::Select(row, Modifiers::NONE));
            // Scrolled only as far as brings it in view.
            let top = row as f32 * ROW_HEIGHT;
            let offset = self.scroll_offset.max(top + ROW_HEIGHT - height).min(top);
            area = area.vertical_scroll_offset(offset.max(0.0));
        }
        let keyed_row = keys.select.or(anchor_row);
        let cue = focus::shows_own_by_id(ui.ctx(), stop).then(|| keyed_row.unwrap_or(0));
        let menu_row = keyed_row.filter(|_| keys.menu);
        let mut cued = false;
        let stop = widgets::list_stop(ui, stop, ui.available_rect_before_wrap());
        let output = area.show_rows(ui, ROW_HEIGHT, shown.len(), |ui, range| {
            for row in range {
                let file = &files[shown[row]];
                // Not a stop of its own: the table is one.
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::CLICK);
                focus::click_row(&response, stop.id);
                if response.double_clicked() {
                    click = Some(FileClick::Open(row));
                } else if response.clicked() {
                    click = Some(FileClick::Select(row, ui.input(|i| i.modifiers)));
                } else if response.secondary_clicked() {
                    click = Some(FileClick::ContextMenu(row));
                }
                if self.selection.paths.contains(&file.path) {
                    ui.painter().rect_filled(rect, 0.0, c.selected_bg);
                }
                if response.hovered() {
                    ui.painter().rect_filled(rect, 0.0, c.hover);
                } else if row % 2 == 1 {
                    ui.painter().rect_filled(rect, 0.0, c.stripe);
                }
                if cue == Some(row) {
                    widgets::paint_list_cue(ui, rect);
                    cued = true;
                }
                let Layout { x, w } = file_columns(&self.widths, rect.width(), rect.left());
                let span = |i: usize| Rect::from_x_y_ranges(x[i]..=x[i] + w[i], rect.y_range());
                let y = rect.center().y;
                let put = |g: Arc<Galley>, left: f32| {
                    ui.painter()
                        .galley(pos2(left, y - g.size().y / 2.0), g, text);
                };
                let put_right = |g: Arc<Galley>, right: f32| {
                    ui.painter()
                        .galley(pos2(right - g.size().x, y - g.size().y / 2.0), g, text);
                };
                let pieces = path_pieces(file);
                let path_width = w[column::PATH] - 2.0 * CELL_PAD;
                let (path, elided) = path_galley(ui, &pieces, &body, path_width, text, weak);
                put(path, x[column::PATH] + CELL_PAD);
                let extension_width = w[column::EXTENSION] - 2.0 * CELL_PAD;
                put(
                    cell(ui, file.extension(), body.clone(), weak, extension_width),
                    x[column::EXTENSION] + CELL_PAD,
                );
                let status_left = pos2(x[column::STATUS] + CELL_PAD, y);
                let badged = status.badges().map(|b| &b[shown[row]]);
                let conflict = tree.and_then(|t| t.conflict(&file.path));
                match (badged, conflict) {
                    (_, Some(conflict)) => paint_conflict(ui, conflict, c, status_left),
                    (Some(b), None) => paint_status_badges(ui, &b.badges, status_left),
                    (None, None) => {
                        let color = match file.status {
                            FileStatus::Added => c.added,
                            FileStatus::Deleted => c.removed,
                            FileStatus::Renamed | FileStatus::Copied => c.renamed,
                            _ => text,
                        };
                        let width = w[column::STATUS] - 2.0 * CELL_PAD;
                        let g = cell(ui, file.status.name(), body.clone(), color, width);
                        put(g, status_left.x);
                    }
                }
                // `None` for a binary file.
                let count = |n: Option<u32>, color| {
                    let (s, color) = match n {
                        Some(n) => (thousands(n as usize), color),
                        None => ("–".to_owned(), weak),
                    };
                    ui.painter().layout_no_wrap(s, body.clone(), color)
                };
                let right = |i: usize| x[i] + w[i] - CELL_PAD;
                put_right(count(file.added, c.added), right(column::ADDED));
                put_right(count(file.removed, c.removed), right(column::REMOVED));
                let hover = response.hover_pos();
                let on_status = hover.is_some_and(|p| span(column::STATUS).contains(p));
                let on_path = hover.is_some_and(|p| span(column::PATH).contains(p));
                let response = match (badged, conflict) {
                    (_, Some(conflict)) if on_status => {
                        response.on_hover_text_at_pointer(capitalized(conflict.words()))
                    }
                    (Some(b), None) if on_status => response.on_hover_text_at_pointer(&b.words),
                    _ => response,
                };
                let response = if elided && on_path {
                    response.on_hover_ui(|ui| {
                        ui.label(path_job(&pieces, 0, &body, text, weak));
                    })
                } else {
                    response
                };
                let at = pos2(rect.left() + CELL_PAD, rect.bottom());
                crate::menu::context_menu(&response, menu_row == Some(row), at)
                    .style(crate::menu::style)
                    .show(|ui| {
                        crate::menu::fit_window(ui, |ui| {
                            crate::usage::menu(ui.ctx(), crate::usage::Menu::File);
                            ui.set_min_width(crate::menu::MIN_WIDTH);
                            let chosen = self.selection.paths.len();
                            let many = chosen > 1 && self.selection.paths.contains(&file.path);
                            if let Some(p) = row_menu(ui, row, file, many.then_some(chosen), tree) {
                                pick = Some(p);
                            }
                            if let Some(tree) = tree
                                && let Some(p) = working_tree_menu(ui, tree, file)
                            {
                                pick = Some(MenuPick::WorkingTree(row, p));
                            }
                            crate::menu::separator(ui);
                            if ui.button("Copy path").clicked() {
                                ui.ctx().copy_text(file.path.clone());
                                ui.close();
                            }
                        });
                    });
            }
        });
        self.scroll_offset = output.state.offset.y;
        widgets::list_stop_ring(ui, &stop, cued);
        RowInput {
            click,
            pick,
            enter: keys.enter,
        }
    }

    fn act<'f>(
        &mut self,
        ui: &mut Ui,
        files: &'f [ChangedFile],
        shown: &[usize],
        input: RowInput,
    ) -> TableAction<'f> {
        let RowInput { click, pick, enter } = input;
        let enter = enter
            || !ui.ctx().egui_wants_keyboard_input()
                && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
        let selection = &mut self.selection;
        let mut action = TableAction::default();
        match pick {
            // The selection, if the row is in it; the row became the selection otherwise.
            Some(MenuPick::Open(row)) => {
                let file = &files[shown[row]];
                if selection.paths.contains(&file.path) {
                    action.open.extend(selection.selected(files, shown));
                } else {
                    action.open.push(file);
                }
            }
            Some(MenuPick::Blame(row)) => action.blame = Some(&files[shown[row]]),
            Some(MenuPick::WorkingTree(row, p)) => {
                action.working_tree = Some((&files[shown[row]], p));
            }
            None => {}
        }
        match click {
            Some(FileClick::Open(row)) => {
                let file = &files[shown[row]];
                selection.select_only(&file.path);
                action.open.push(file);
            }
            Some(FileClick::Select(row, modifiers)) => {
                selection.click(row, modifiers, files, shown);
            }
            Some(FileClick::ContextMenu(row)) => {
                let path = &files[shown[row]].path;
                if !selection.paths.contains(path) {
                    selection.select_only(path);
                }
            }
            None => {}
        }
        if enter {
            action.open.extend(selection.selected(files, shown));
        }
        action
    }
}

fn note(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        add(ui);
    });
}

fn paint_status_badges(ui: &Ui, badges: &[StatusBadge], left_center: Pos2) {
    let mut x = left_center.x;
    for badge in badges {
        let (letter, fill) = if badge.filled {
            ((badge.letter, Color32::WHITE), Some(badge.color))
        } else {
            ((badge.letter, badge.color), None)
        };
        x += boxed(ui, &[letter], badge.color, fill, pos2(x, left_center.y)) + 4.0;
    }
}

/// Ours then theirs, each in its status's colour, boxed in red.
fn paint_conflict(ui: &Ui, conflict: &Conflict, c: &Colors, left_center: Pos2) {
    let weak = ui.visuals().weak_text_color();
    let letters = conflict.letters().map(|l| {
        let color = match l {
            'M' => c.modified,
            'A' => c.added,
            'D' => c.removed,
            _ => weak,
        };
        (l, color)
    });
    boxed(ui, &letters, c.removed, None, left_center);
}

/// Square for one letter, wider for more. Returns its width.
fn boxed(
    ui: &Ui,
    letters: &[(char, Color32)],
    border: Color32,
    fill: Option<Color32>,
    left_center: Pos2,
) -> f32 {
    let mut job = LayoutJob::default();
    for &(letter, color) in letters {
        job.append(
            letter.encode_utf8(&mut [0; 4]),
            0.0,
            TextFormat::simple(FontId::monospace(11.0), color),
        );
    }
    let g = ui.painter().layout_job(job);
    let width = (g.size().x + 6.0).max(STATUS_BOX_SIZE);
    let r = Rect::from_min_size(
        pos2(left_center.x, left_center.y - STATUS_BOX_SIZE / 2.0),
        vec2(width, STATUS_BOX_SIZE),
    );
    match fill {
        Some(fill) => {
            ui.painter().rect_filled(r, 3.0, fill);
        }
        None => {
            ui.painter().rect_stroke(
                r.shrink(0.5),
                3.0,
                Stroke::new(1.2, border),
                egui::StrokeKind::Inside,
            );
        }
    }
    ui.painter()
        .galley(r.center() - g.size() / 2.0, g, Color32::PLACEHOLDER);
    width
}

/// `many` is how many files are selected, when the row is one of them.
fn row_menu(
    ui: &mut Ui,
    row: usize,
    file: &ChangedFile,
    many: Option<usize>,
    tree: Option<&WorkingTree>,
) -> Option<MenuPick> {
    let mut pick = None;
    let text = match many {
        Some(n) => format!("Show changes ({n} files)"),
        None => "Show changes".to_owned(),
    };
    if ui
        .add(egui::Button::new(text).shortcut_text("Enter"))
        .clicked()
    {
        pick = Some(MenuPick::Open(row));
        ui.close();
    }
    let why = if file.status == FileStatus::Deleted {
        Some("The file is gone in this version")
    } else if file.modes.contains(&GITLINK_MODE) {
        Some("A submodule has no lines to blame")
    } else if tree
        .and_then(|t| t.conflict(&file.path))
        .map_or(file.is_binary(), |c| c.binary)
    {
        Some("A binary file has no lines to blame")
    } else {
        None
    };
    let blame = ui
        .add_enabled(why.is_none(), egui::Button::new("Blame"))
        .on_disabled_hover_text(why.unwrap_or_default());
    if blame.clicked() {
        pick = Some(MenuPick::Blame(row));
        ui.close();
    }
    pick
}

fn working_tree_menu(
    ui: &mut Ui,
    tree: &WorkingTree,
    file: &ChangedFile,
) -> Option<WorkingTreePick> {
    let mut pick = None;
    let mut item = |ui: &mut Ui, response: Response, p: WorkingTreePick| {
        if response.clicked() {
            pick = Some(p);
            ui.close();
        }
    };
    crate::menu::separator(ui);
    let conflict = tree.conflict(&file.path);
    let why = match conflict {
        Some(c) => c.merge_tool().err(),
        None => Some("Not conflicted"),
    };
    let tool = ui
        .add_enabled(why.is_none(), egui::Button::new("Open in merge tool"))
        .on_disabled_hover_text(why.unwrap_or_default());
    item(ui, tool, WorkingTreePick::MergeTool);
    if let Some(c) = conflict {
        for answer in tree.sides.mine_first(c.answers()) {
            let commands: Vec<String> = c
                .commands(answer)
                .iter()
                .map(|args| format!("git {}", args.join(" ")))
                .collect();
            let response = ui
                .button(c.item(&tree.sides, answer))
                .on_hover_text(commands.join("\n"));
            item(ui, response, WorkingTreePick::Answer(answer));
        }
    }
    crate::menu::separator(ui);
    let terminal = ui.button("Open terminal");
    item(ui, terminal, WorkingTreePick::Terminal);
    let files = ui.button("Open file system");
    item(ui, files, WorkingTreePick::FileSystem);
    pick
}

/// The nearest that still exists.
pub fn file_folder(root: &Path, path: &str) -> PathBuf {
    let mut dir = root.join(path);
    dir.pop();
    while !dir.is_dir() && dir.starts_with(root) && dir != root {
        dir.pop();
    }
    if dir.is_dir() { dir } else { root.to_owned() }
}

#[derive(Debug)]
pub struct DiffQueue<T> {
    ready: Vec<T>,
    unconfirmed: Option<Vec<T>>,
}

impl<T> Default for DiffQueue<T> {
    fn default() -> Self {
        DiffQueue {
            ready: Vec::new(),
            unconfirmed: None,
        }
    }
}

impl<T> DiffQueue<T> {
    pub fn push(&mut self, open: Vec<T>) {
        if open.len() > MAX_DIFFS_WITHOUT_ASKING {
            self.unconfirmed = Some(open);
        } else {
            self.ready.extend(open);
        }
    }

    pub fn take(&mut self) -> Vec<T> {
        std::mem::take(&mut self.ready)
    }

    pub fn cancel(&mut self) {
        self.unconfirmed = None;
    }

    /// A modal dialog over the window `ui` is in.
    pub fn confirm_many(&mut self, ui: &mut Ui, id: Id) {
        let Some(unconfirmed) = &self.unconfirmed else {
            return;
        };
        let n = unconfirmed.len();
        let shown = crate::dialogs::Dialog::new(id, &format!("Open {n} diff windows?"))
            .screen(crate::usage::Screen::OpenDiffs)
            .width(340.0)
            .modal()
            .opener(ui.ctx().viewport_id())
            .show(ui.ctx(), |ui| {
                ui.add_space(4.0);
                ui.label("A window opens for every selected file.");
                ui.add_space(12.0);
                ui.separator();
                crate::dialogs::actions(ui, crate::dialogs::Primary::new("Open all"))
            });
        match shown.inner {
            crate::dialogs::Answer::Primary => self
                .ready
                .extend(self.unconfirmed.take().unwrap_or_default()),
            crate::dialogs::Answer::Cancel => self.unconfirmed = None,
            crate::dialogs::Answer::Open if shown.should_close() => self.unconfirmed = None,
            crate::dialogs::Answer::Open => {}
        }
    }
}

/// Values asked of git on a worker thread when first needed, kept by key.
#[derive(Debug)]
pub struct Lister<K, V> {
    /// `None` while git works on it.
    pub(super) cache: HashMap<K, Option<V>>,
    /// `None` for a request dropped because a newer one came in.
    rx: Option<mpsc::Receiver<(K, Option<V>)>>,
    tx: Option<mpsc::Sender<K>>,
    repo_path: Option<PathBuf>,
}

impl<K, V> Default for Lister<K, V> {
    fn default() -> Self {
        Lister {
            cache: HashMap::new(),
            rx: None,
            tx: None,
            repo_path: None,
        }
    }
}

impl<K, V> Lister<K, V>
where
    K: Clone + Eq + Hash + Send + 'static,
    V: Send + 'static,
{
    /// Asked from another folder (another worktree gone to, perhaps since deleted), it starts
    /// afresh.
    pub fn get(
        &mut self,
        repo_path: &Path,
        key: K,
        ctx: &egui::Context,
        work: fn(&Git, &K) -> V,
    ) -> Option<&V> {
        if self.repo_path.as_deref() != Some(repo_path) {
            *self = Lister {
                repo_path: Some(repo_path.to_owned()),
                ..Lister::default()
            };
        }
        while let Some(Ok((key, value))) = self.rx.as_ref().map(|rx| rx.try_recv()) {
            match value {
                Some(value) => self.cache.insert(key, Some(value)),
                None => self.cache.remove(&key),
            };
        }
        if let Entry::Vacant(slot) = self.cache.entry(key.clone()) {
            slot.insert(None);
            let tx = self.tx.get_or_insert_with(|| {
                let (req_tx, req_rx) = mpsc::channel::<K>();
                let (res_tx, res_rx) = mpsc::channel();
                let git = Git::new(repo_path);
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    while let Ok(mut key) = req_rx.recv() {
                        // Holding an arrow key asks for one commit after another, and only the
                        // newest matters. The others are forgotten, so that they are asked for
                        // again if they show up once more.
                        while let Ok(newer) = req_rx.try_recv() {
                            if res_tx.send((key, None)).is_err() {
                                return;
                            }
                            key = newer;
                        }
                        let value = work(&git, &key);
                        if res_tx.send((key, Some(value))).is_err() {
                            return;
                        }
                        ctx.request_repaint();
                    }
                });
                self.rx = Some(res_rx);
                req_tx
            });
            let _ = tx.send(key.clone());
        }
        self.cache.get(&key).and_then(Option::as_ref)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathPiece {
    Folder,
    Name,
    Arrow,
}

/// `old → new` for renames and copies.
fn path_pieces(file: &ChangedFile) -> Vec<(String, PathPiece)> {
    let mut pieces = Vec::new();
    let push_path = |pieces: &mut Vec<(String, PathPiece)>, path: &str| match path.rfind('/') {
        Some(i) => {
            pieces.push((path[..=i].to_owned(), PathPiece::Folder));
            pieces.push((path[i + 1..].to_owned(), PathPiece::Name));
        }
        None => pieces.push((path.to_owned(), PathPiece::Name)),
    };
    if let Some(old) = &file.old_path {
        push_path(&mut pieces, old);
        pieces.push((" → ".to_owned(), PathPiece::Arrow));
    }
    push_path(&mut pieces, &file.path);
    pieces
}

/// The pieces from byte `from` of their text on, after an ellipsis if that isn't 0. The arrow
/// is monospaced: the proportional font has none.
fn path_job(
    pieces: &[(String, PathPiece)],
    from: usize,
    font: &FontId,
    text: Color32,
    weak: Color32,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    if from > 0 {
        job.append("…", 0.0, TextFormat::simple(font.clone(), weak));
    }
    let mut at = 0;
    for (piece, kind) in pieces {
        let (start, end) = (at, at + piece.len());
        at = end;
        if end <= from {
            continue;
        }
        let piece = &piece[start.max(from) - start..];
        let format = match kind {
            PathPiece::Folder => TextFormat::simple(font.clone(), weak),
            PathPiece::Name => TextFormat::simple(font.clone(), text),
            PathPiece::Arrow => TextFormat::simple(FontId::monospace(font.size), weak),
        };
        job.append(piece, 0.0, format);
    }
    job
}

/// Cut at the start, so that the file name stays in view; true if it was cut.
fn path_galley(
    ui: &Ui,
    pieces: &[(String, PathPiece)],
    font: &FontId,
    width: f32,
    text: Color32,
    weak: Color32,
) -> (Arc<Galley>, bool) {
    let full: String = pieces.iter().map(|(s, _)| s.as_str()).collect();
    let painter = ui.painter();
    // A candidate is `full`, or "…" and an end of `full`.
    let job = |shown: &str| {
        let from = if shown == full {
            0
        } else {
            full.len() + '…'.len_utf8() - shown.len()
        };
        path_job(pieces, from, font, text, weak)
    };
    let shown = elide_start(&full, |s| painter.layout_job(job(s)).size().x <= width);
    (painter.layout_job(job(&shown)), shown != full)
}

/// In a narrow pane, the columns after the path shrink, with shorter headings, so that the path
/// keeps room.
fn is_compact(width: f32) -> bool {
    width - FIXED_COLUMN_WIDTHS.iter().sum::<f32>() < ROOMY_PATH_WIDTH
}

fn heading(column: FileColumn, compact: bool) -> &'static str {
    match column {
        FileColumn::Extension if compact => "Ext.",
        FileColumn::Added if compact => "Added",
        FileColumn::Removed if compact => "Removed",
        _ => column.title(),
    }
}

fn file_columns(widths: &ColumnWidths, width: f32, left: f32) -> Layout {
    let fixed = if is_compact(width) {
        COMPACT_FIXED_COLUMN_WIDTHS
    } else {
        FIXED_COLUMN_WIDTHS
    };
    let defaults = [0.0, fixed[0], fixed[1], fixed[2], fixed[3]];
    widths.layout(&defaults, column::PATH, PATH_MIN_WIDTH, left, width)
}

fn columns(layout: &Layout) -> Columns<'_> {
    Columns {
        layout,
        flex: column::PATH,
        flex_min: PATH_MIN_WIDTH,
    }
}

fn sort_arrow(ui: &Ui, at: Pos2, descending: bool, color: Color32) {
    let (w, h) = (3.5, 3.0);
    let points = if descending {
        vec![
            at + vec2(-w, -h / 2.0),
            at + vec2(w, -h / 2.0),
            at + vec2(0.0, h),
        ]
    } else {
        vec![
            at + vec2(-w, h / 2.0),
            at + vec2(w, h / 2.0),
            at + vec2(0.0, -h),
        ]
    };
    ui.painter()
        .add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
}

fn filter_field(ui: &mut Ui, text: &mut String, id: Id) {
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
        .corner_radius(7)
        .inner_margin(Margin {
            left: 8,
            right: 6,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.set_width(FILTER_WIDTH - 14.0);
            ui.set_height(28.0);
            ui.spacing_mut().item_spacing.x = 6.0;
            let weak = ui.visuals().weak_text_color();
            let (icon, _) = ui.allocate_exact_size(Vec2::splat(15.0), Sense::hover());
            widgets::paint_glyph(ui.painter(), icon, parterre_core::glyphs::SEARCH, weak);
            ui.add(
                egui::TextEdit::singleline(text)
                    .id(id)
                    .frame(egui::Frame::NONE)
                    .hint_text("Filter paths")
                    .desired_width(ui.available_width()),
            );
        });
}

#[cfg(test)]
mod tests {
    use super::super::tool_harness::{
        assert_no_id_clashes, click_events, git, raw_input, run_frame, stuck_merge, text_at, write,
    };
    use super::*;

    #[test]
    fn a_lister_asked_from_another_folder_starts_afresh() {
        let ctx = egui::Context::default();
        let mut lister: Lister<u32, String> = Lister::default();
        let wait = |lister: &mut Lister<u32, String>, path: &str| loop {
            if let Some(v) = lister.get(Path::new(path), 1, &ctx, |git, _| {
                git.dir().display().to_string()
            }) {
                return v.clone();
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(wait(&mut lister, "/gone"), "/gone");
        assert_eq!(wait(&mut lister, "/open"), "/open");
    }

    fn renamed() -> ChangedFile {
        ChangedFile {
            path: "src/new.rs".into(),
            old_path: Some("lib/old.rs".into()),
            status: FileStatus::Renamed,
            modes: [0o100644; 2],
            added: Some(1),
            removed: Some(0),
        }
    }

    #[test]
    fn renames_show_old_and_new_path_with_folders_apart() {
        let pieces = path_pieces(&renamed());
        let kinds: Vec<_> = pieces.iter().map(|(s, k)| (s.as_str(), *k)).collect();
        assert_eq!(
            kinds,
            [
                ("lib/", PathPiece::Folder),
                ("old.rs", PathPiece::Name),
                (" → ", PathPiece::Arrow),
                ("src/", PathPiece::Folder),
                ("new.rs", PathPiece::Name),
            ]
        );
        let job = path_job(
            &pieces,
            0,
            &FontId::default(),
            Color32::WHITE,
            Color32::GRAY,
        );
        assert_eq!(job.text, "lib/old.rs → src/new.rs");
    }

    #[test]
    fn a_cut_path_keeps_its_end_after_an_ellipsis() {
        let pieces = path_pieces(&renamed());
        let (text, weak) = (Color32::WHITE, Color32::GRAY);
        // Cut inside the first file name, then inside the second folder.
        let job = path_job(&pieces, 6, &FontId::default(), text, weak);
        assert_eq!(job.text, "…d.rs → src/new.rs");
        assert_eq!(job.sections[1].format.color, text);
        let from = "lib/old.rs → s".len();
        let job = path_job(&pieces, from, &FontId::default(), text, weak);
        assert_eq!(job.text, "…rc/new.rs");
        // The ellipsis and the folder share one weak section.
        let colors: Vec<_> = job.sections.iter().map(|s| s.format.color).collect();
        assert_eq!(colors, [weak, text]);
    }

    /// The compare window's table against a stopped merge, frame by frame.
    struct Table {
        ctx: egui::Context,
        table: FileTable,
        listing: Listing,
        tree: WorkingTree,
        texts: Vec<(String, Rect)>,
    }

    impl Table {
        fn new(dir: &Path) -> Table {
            let (listing, tree) = Table::list(dir);
            let mut t = Table {
                ctx: egui::Context::default(),
                table: FileTable::default(),
                listing,
                tree,
                texts: Vec::new(),
            };
            t.frame(Vec::new());
            t
        }

        /// HEAD against the working tree in `dir`.
        fn list(dir: &Path) -> (Listing, WorkingTree) {
            let head = parterre_core::Oid::from_hex(&git(dir, &["rev-parse", "HEAD"])).unwrap();
            let compared = parterre_core::compare::Comparison::with_working_tree(head, false)
                .run(&Git::new(dir))
                .unwrap();
            (Ok(compared.files), compared.working_tree.unwrap())
        }

        fn relist(&mut self, dir: &Path) {
            (self.listing, self.tree) = Table::list(dir);
            self.frame(Vec::new());
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> Option<(String, WorkingTreePick)> {
            let mut picked = None;
            let (table, listing, tree) = (&mut self.table, &self.listing, &self.tree);
            let input = raw_input(vec2(900.0, 600.0), events);
            self.texts = run_frame(&self.ctx, input, |ui| {
                let action = table.show_working_tree(
                    ui,
                    "test",
                    Id::new("test"),
                    Some(listing),
                    Some(tree),
                    |_| {},
                );
                picked = action.working_tree.map(|(f, p)| (f.path.clone(), p));
            });
            assert_no_id_clashes(&self.texts);
            picked
        }

        fn shows(&self, text: &str) -> bool {
            self.texts.iter().any(|(t, _)| t == text)
        }

        fn click(
            &mut self,
            text: &str,
            button: egui::PointerButton,
        ) -> Option<(String, WorkingTreePick)> {
            let at = text_at(&self.texts, text);
            let mut picked = None;
            for events in click_events(at, button, Modifiers::NONE) {
                picked = picked.or(self.frame(events));
            }
            picked.or(self.frame(Vec::new()))
        }
    }

    #[test]
    fn conflicted_rows_show_what_each_side_did() {
        let dir = stuck_merge();
        let t = Table::new(dir.path());
        assert!(t.shows("MM"), "{:?}", t.texts);
        assert!(t.shows("MD"));
        // `gone.txt` is the same as HEAD's, which `git diff HEAD` leaves out.
        assert!(t.shows("gone.txt"));
    }

    #[test]
    fn while_conflicted_only_conflicts_are_listed_unless_asked() {
        let dir = stuck_merge();
        let p = dir.path();
        write(p, "new.txt", "new\n");
        git(p, &["add", "new.txt"]);
        let mut t = Table::new(p);
        assert!(t.shows("Conflicts only"), "{:?}", t.texts);
        assert!(t.shows("text.txt") && t.shows("gone.txt"));
        assert!(!t.shows("new.txt"));
        assert!(t.shows("2 of 3 files"));
        t.click("Conflicts only", egui::PointerButton::Primary);
        assert!(t.shows("new.txt"), "{:?}", t.texts);
        assert!(t.shows("3 files"));
        t.click("Conflicts only", egui::PointerButton::Primary);
        assert!(!t.shows("new.txt"));
        // A resolved file drops out.
        write(p, "text.txt", "one\ntwo\n");
        git(p, &["add", "text.txt"]);
        t.relist(p);
        assert!(t.shows("gone.txt") && !t.shows("text.txt"));
        // With none left, every file is listed and the checkbox goes; the next conflicts
        // start filtered even if it was unchecked.
        t.click("Conflicts only", egui::PointerButton::Primary);
        git(p, &["rm", "-q", "gone.txt"]);
        t.relist(p);
        assert!(!t.shows("Conflicts only"));
        assert!(t.shows("new.txt"), "{:?}", t.texts);
        assert!(!t.table.all_files);
    }

    #[test]
    fn a_file_deleted_by_them_takes_a_side_but_not_the_merge_tool() {
        let dir = stuck_merge();
        let mut t = Table::new(dir.path());
        t.click("gone.txt", egui::PointerButton::Secondary);
        for item in [
            "Open in merge tool",
            "Use mine (main)",
            "Use theirs (feature, deleted)",
            "Open terminal",
            "Open file system",
        ] {
            assert!(t.shows(item), "{item}: {:?}", t.texts);
        }
        // Greyed out: clicking it picks nothing.
        assert_eq!(
            t.click("Open in merge tool", egui::PointerButton::Primary),
            None
        );
        t.click("gone.txt", egui::PointerButton::Secondary);
        assert_eq!(
            t.click(
                "Use theirs (feature, deleted)",
                egui::PointerButton::Primary
            ),
            Some((
                "gone.txt".to_owned(),
                WorkingTreePick::Answer(Answer::Theirs)
            ))
        );
    }

    #[test]
    fn a_content_conflict_opens_in_the_merge_tool() {
        let dir = stuck_merge();
        let mut t = Table::new(dir.path());
        t.click("text.txt", egui::PointerButton::Secondary);
        // Or either side, whole.
        assert!(t.shows("Use mine (main)"));
        assert!(t.shows("Use theirs (feature)"));
        assert_eq!(
            t.click("Open in merge tool", egui::PointerButton::Primary),
            Some(("text.txt".to_owned(), WorkingTreePick::MergeTool))
        );
        t.click("text.txt", egui::PointerButton::Secondary);
        assert_eq!(
            t.click("Open file system", egui::PointerButton::Primary),
            Some(("text.txt".to_owned(), WorkingTreePick::FileSystem))
        );
    }

    #[test]
    fn a_file_s_folder_is_the_nearest_that_exists() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        assert_eq!(file_folder(root, "a/b/c.txt"), root.join("a/b"));
        assert_eq!(file_folder(root, "a/gone/c.txt"), root.join("a"));
        assert_eq!(file_folder(root, "top.txt"), root.to_owned());
    }
}
