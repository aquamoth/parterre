//! The widths of a table's columns (#125): one column, the flexible one, takes what the others
//! leave; the others are as wide as the window's layout says until the user drags their
//! border, and then as wide as dragged. A border follows the pointer: left of the flexible
//! column it sizes the column on its left, right of it the column on its right, and the
//! flexible column takes up the difference. The widths the user picks last as long as the
//! table's window is open.

/// The narrowest a dragged column gets.
pub const MIN_WIDTH: f32 = 24.0;

/// The columns' x and widths, left to right.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub x: Vec<f32>,
    pub w: Vec<f32>,
}

impl Layout {
    /// The x of border `i`, between columns `i` and `i + 1`.
    pub fn border(&self, i: usize) -> f32 {
        self.x[i + 1]
    }
}

/// The widths the user picked, by column; `None` where the layout decides.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColumnWidths {
    set: Vec<Option<f32>>,
}

impl ColumnWidths {
    /// Column `i`'s width: the one picked, or else `default`.
    pub fn get(&self, i: usize, default: f32) -> f32 {
        self.picked(i).unwrap_or(default)
    }

    /// The width picked for column `i`, if one was.
    pub fn picked(&self, i: usize) -> Option<f32> {
        self.set.get(i).copied().flatten()
    }

    /// PROTOTYPE (#172): picks column `i`'s width, as if the user had dragged it.
    pub fn pick(&mut self, i: usize, width: f32) {
        if self.set.len() <= i {
            self.set.resize(i + 1, None);
        }
        self.set[i] = Some(width);
    }

    /// The columns from `left`, `width` wide in all: column `flex` takes what the others leave,
    /// but no less than `flex_min`; the others are as picked, or as in `defaults` (whose entry
    /// for `flex` is ignored).
    pub fn layout(
        &self,
        defaults: &[f32],
        flex: usize,
        flex_min: f32,
        left: f32,
        width: f32,
    ) -> Layout {
        let mut w: Vec<f32> = defaults
            .iter()
            .enumerate()
            .map(|(i, &d)| if i == flex { 0.0 } else { self.get(i, d) })
            .collect();
        w[flex] = (width - w.iter().sum::<f32>()).max(flex_min);
        let x = w
            .iter()
            .scan(left, |at, &w| {
                let x = *at;
                *at += w;
                Some(x)
            })
            .collect();
        Layout { x, w }
    }

    /// The column that border `i` (between columns `i` and `i + 1`) sizes: the one away from
    /// the flexible column.
    pub fn sized_by(border: usize, flex: usize) -> usize {
        if border < flex { border } else { border + 1 }
    }

    /// Border `border` of `layout` dragged to `pointer`: sizes its column so that the border is
    /// under the pointer, as far as leaves the column [`MIN_WIDTH`] and the flexible column
    /// `flex_min`.
    pub fn drag(
        &mut self,
        layout: &Layout,
        border: usize,
        flex: usize,
        flex_min: f32,
        pointer: f32,
    ) {
        let i = Self::sized_by(border, flex);
        let (x, w) = (layout.x[i], layout.w[i]);
        let wanted = if i < flex {
            pointer - x
        } else {
            x + w - pointer
        };
        let room = (layout.w[flex] - flex_min).max(0.0);
        let width = wanted.min(w + room).max(MIN_WIDTH.min(w));
        if self.set.len() <= i {
            self.set.resize(i + 1, None);
        }
        self.set[i] = Some(width);
    }

    /// Puts the column that border `border` sizes back to the layout's width.
    pub fn reset(&mut self, border: usize, flex: usize) {
        if let Some(w) = self.set.get_mut(Self::sized_by(border, flex)) {
            *w = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULTS: [f32; 4] = [50.0, 0.0, 100.0, 80.0];

    #[test]
    fn the_flexible_column_takes_what_the_others_leave() {
        let widths = ColumnWidths::default();
        let l = widths.layout(&DEFAULTS, 1, 60.0, 10.0, 500.0);
        assert_eq!(l.w, [50.0, 270.0, 100.0, 80.0]);
        assert_eq!(l.x, [10.0, 60.0, 330.0, 430.0]);
        // No narrower than its minimum, even if the table runs over.
        let l = widths.layout(&DEFAULTS, 1, 60.0, 0.0, 200.0);
        assert_eq!(l.w[1], 60.0);
    }

    #[test]
    fn a_dragged_border_follows_the_pointer() {
        let mut widths = ColumnWidths::default();
        let l = widths.layout(&DEFAULTS, 1, 60.0, 0.0, 500.0);
        // Left of the flexible column: the column on the left.
        widths.drag(&l, 0, 1, 60.0, 70.0);
        let l = widths.layout(&DEFAULTS, 1, 60.0, 0.0, 500.0);
        assert_eq!(l.border(0), 70.0);
        assert_eq!(l.w, [70.0, 250.0, 100.0, 80.0]);
        // Right of it: the column on the right.
        widths.drag(&l, 1, 1, 60.0, 300.0);
        let l = widths.layout(&DEFAULTS, 1, 60.0, 0.0, 500.0);
        assert_eq!(l.border(1), 300.0);
        assert_eq!(l.w, [70.0, 230.0, 120.0, 80.0]);
        widths.drag(&l, 2, 1, 60.0, 440.0);
        let l = widths.layout(&DEFAULTS, 1, 60.0, 0.0, 500.0);
        assert_eq!(l.border(2), 440.0);
        assert_eq!(l.w, [70.0, 250.0, 120.0, 60.0]);
    }

    #[test]
    fn a_drag_leaves_every_column_some_room() {
        let mut widths = ColumnWidths::default();
        let l = widths.layout(&DEFAULTS, 1, 60.0, 0.0, 500.0);
        widths.drag(&l, 0, 1, 60.0, -100.0);
        assert_eq!(widths.picked(0), Some(MIN_WIDTH));
        widths.drag(&l, 0, 1, 60.0, 1000.0);
        let l = widths.layout(&DEFAULTS, 1, 60.0, 0.0, 500.0);
        assert_eq!(l.w[1], 60.0);
        assert_eq!(l.w.iter().sum::<f32>(), 500.0);
    }

    #[test]
    fn a_reset_column_follows_the_layout_again() {
        let mut widths = ColumnWidths::default();
        let l = widths.layout(&DEFAULTS, 1, 60.0, 0.0, 500.0);
        widths.drag(&l, 2, 1, 60.0, 300.0);
        assert_eq!(widths.picked(3), Some(200.0));
        widths.reset(2, 1);
        assert_eq!(widths, ColumnWidths { set: vec![None; 4] });
        let l = widths.layout(&DEFAULTS, 1, 60.0, 0.0, 500.0);
        assert_eq!(l.w[3], 80.0);
    }
}
