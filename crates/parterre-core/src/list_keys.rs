//! Where the keyboard moves the selection of a list or a row of choices that is one stop for Tab
//! (#387): tabs and segments by Left and Right, a list's rows by Up and Down, a page at a time,
//! or to either end.

/// A move of the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// Up or Left.
    Back,
    /// Down or Right.
    Forward,
    /// Page Up and Page Down: by a page of rows.
    PageBack(usize),
    PageForward(usize),
    /// Home and End.
    First,
    Last,
}

/// The item `step` goes to from the `selected` one of `len`: none past either end, and from no
/// selection, the first. `None` with no items.
pub fn target(selected: Option<usize>, len: usize, step: Move) -> Option<usize> {
    let last = len.checked_sub(1)?;
    let Some(at) = selected.map(|s| s.min(last)) else {
        return Some(if step == Move::Last { last } else { 0 });
    };
    Some(match step {
        Move::Back => at.saturating_sub(1),
        Move::Forward => (at + 1).min(last),
        Move::PageBack(page) => at.saturating_sub(page.max(1)),
        Move::PageForward(page) => (at + page.max(1)).min(last),
        Move::First => 0,
        Move::Last => last,
    })
}

#[cfg(test)]
mod tests {
    use super::{Move, target};

    #[test]
    fn moves_stop_at_the_ends() {
        assert_eq!(target(Some(0), 3, Move::Back), Some(0));
        assert_eq!(target(Some(1), 3, Move::Back), Some(0));
        assert_eq!(target(Some(1), 3, Move::Forward), Some(2));
        assert_eq!(target(Some(2), 3, Move::Forward), Some(2));
    }

    #[test]
    fn a_page_moves_as_far_as_it_can() {
        assert_eq!(target(Some(1), 10, Move::PageForward(4)), Some(5));
        assert_eq!(target(Some(8), 10, Move::PageForward(4)), Some(9));
        assert_eq!(target(Some(3), 10, Move::PageBack(4)), Some(0));
        // A list shorter than a row still moves by one.
        assert_eq!(target(Some(3), 10, Move::PageBack(0)), Some(2));
    }

    #[test]
    fn home_and_end_go_to_the_ends() {
        assert_eq!(target(Some(4), 10, Move::First), Some(0));
        assert_eq!(target(Some(4), 10, Move::Last), Some(9));
    }

    #[test]
    fn from_no_selection_a_move_goes_to_the_first_and_end_to_the_last() {
        assert_eq!(target(None, 3, Move::Forward), Some(0));
        assert_eq!(target(None, 3, Move::Back), Some(0));
        assert_eq!(target(None, 3, Move::Last), Some(2));
    }

    #[test]
    fn a_selection_past_a_shortened_list_counts_from_its_last() {
        assert_eq!(target(Some(7), 3, Move::Back), Some(1));
        assert_eq!(target(None, 0, Move::Forward), None);
    }
}
