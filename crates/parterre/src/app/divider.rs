use eframe::egui::{CursorIcon, Id, Pos2, Rect, Sense, Stroke, Ui};

use super::table_style::colors;
use crate::widgets;

pub(super) const DIVIDER_THICKNESS: f32 = 6.0;

/// A draggable divider, and how a pointer position maps to its fraction in
/// [`Dividers`](parterre_core::log_layout::Dividers).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Bar {
    pub(super) rect: Rect,
    pub(super) vertical: bool,
    /// Where the bar's middle is at fraction 0 (along x for a vertical bar, else y), and the
    /// room its fraction is of.
    pub(super) origin: f32,
    pub(super) room: f32,
}

impl Bar {
    pub(super) fn fraction(&self, middle: f32) -> f32 {
        (middle - self.origin) / self.room
    }
}

/// While dragged, returns where its middle should go: the pointer's position across the bar,
/// less where on the bar it was grabbed.
pub(super) fn divider(ui: &mut Ui, id: Id, bar: &Bar) -> Option<f32> {
    let c = colors(ui);
    let rect = bar.rect;
    let cursor = if bar.vertical {
        CursorIcon::ResizeHorizontal
    } else {
        CursorIcon::ResizeVertical
    };
    let response = ui.interact(rect, id, Sense::DRAG).on_hover_cursor(cursor);
    let active = response.hovered() || response.dragged();
    let painter = ui.painter();
    let fill = if active {
        widgets::tones(ui).accent
    } else {
        ui.visuals().panel_fill
    };
    painter.rect_filled(rect, 0.0, fill);
    let line = Stroke::new(1.0, c.line);
    if bar.vertical {
        painter.vline(rect.left() + 0.5, rect.y_range(), line);
        painter.vline(rect.right() - 0.5, rect.y_range(), line);
    } else {
        painter.hline(rect.x_range(), rect.top() + 0.5, line);
        painter.hline(rect.x_range(), rect.bottom() - 0.5, line);
    }
    let along = |p: Pos2| if bar.vertical { p.x } else { p.y };
    let pointer = response.interact_pointer_pos().map(along)?;
    if response.drag_started() {
        let middle = along(rect.center());
        ui.data_mut(|d| d.insert_temp(id, pointer - middle));
    }
    if !response.dragged() {
        return None;
    }
    let grabbed = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
    Some(pointer - grabbed)
}
