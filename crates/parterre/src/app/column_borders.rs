//! The borders between a table's column headings, dragged to resize the columns and
//! double-clicked to put one back to the layout's width (#125). The widths are
//! [`ColumnWidths`]; the table lays its columns out again after [`drag`], so that the headings
//! and rows follow the pointer in the same frame, then [`paint`]s the borders.

use eframe::egui::{CursorIcon, Id, Rect, Sense, Stroke, Ui};
use parterre_core::columns::{ColumnWidths, Layout};

use super::log_window::Colors;
use crate::widgets;

/// A table's columns: laid out as `layout`, `flex` taking what the others leave, down to
/// `flex_min`.
pub struct Columns<'a> {
    pub layout: &'a Layout,
    pub flex: usize,
    pub flex_min: f32,
}

impl Columns<'_> {
    /// The borders that size a column, and where they are.
    fn borders(&self) -> impl Iterator<Item = (usize, f32)> + '_ {
        (0..self.layout.w.len().saturating_sub(1))
            .filter(|&b| self.layout.w[ColumnWidths::sized_by(b, self.flex)] > 0.0)
            .map(|b| (b, self.layout.border(b)))
    }
}

/// Lets the borders of `columns` in the headings `head` be dragged, sizing the columns in
/// `widths`, or double-clicked, resetting them. Returns the border under the pointer or being
/// dragged, to [`paint`] highlighted. Call after the headings' own widgets, so that the borders
/// are on top of them.
pub fn drag(
    ui: &Ui,
    id: Id,
    head: Rect,
    columns: &Columns,
    widths: &mut ColumnWidths,
) -> Option<usize> {
    let mut active = None;
    for (border, x) in columns.borders() {
        let rect = Rect::from_x_y_ranges(x - 4.0..=x + 4.0, head.y_range());
        let id = id.with(("column-border", border));
        let response = ui
            .interact(rect, id, Sense::click_and_drag())
            .on_hover_cursor(CursorIcon::ResizeHorizontal);
        let response = widgets::tip(response, "Drag to resize", "Double-click resets");
        if response.hovered() || response.dragged() {
            active = Some(border);
        }
        if response.double_clicked() {
            widths.reset(border, columns.flex);
        } else if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            // Where on the border it was grabbed, so that it doesn't jump to the pointer.
            if response.drag_started() {
                let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(pointer);
                ui.data_mut(|d| d.insert_temp(id, origin.x - x));
            }
            let grab: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0);
            let (flex, flex_min) = (columns.flex, columns.flex_min);
            widths.drag(columns.layout, border, flex, flex_min, pointer.x - grab);
        }
    }
    active
}

/// Paints the borders of `columns` in the headings `head`: a short line, the `active` one
/// full height in the accent colour.
pub fn paint(ui: &Ui, head: Rect, columns: &Columns, active: Option<usize>, c: &Colors) {
    let painter = ui.painter();
    for (border, x) in columns.borders() {
        let x = x.round() - 0.5;
        if active == Some(border) {
            let stroke = Stroke::new(2.0, widgets::tones(ui).accent);
            painter.vline(x, head.y_range(), stroke);
        } else {
            let y = head.shrink2(eframe::egui::vec2(0.0, 6.0)).y_range();
            painter.vline(x, y, Stroke::new(1.0, c.line));
        }
    }
}
