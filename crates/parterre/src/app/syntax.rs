//! PROTOTYPE (#208): colours for the syntax spans of `parterre_core::highlight`, and the
//! sections a line's text is laid out in when its syntax colours and its changed words
//! (background) are combined.

use eframe::egui::Color32;
use parterre_core::file_diff::display_column;
use parterre_core::highlight::{Kind, Spans};
use std::ops::Range;

/// The colour of a kind of span, or `None` for the plain text colour. The light colours
/// are VS Code's Light+, the dark ones Dark+.
pub fn color(kind: Kind, dark: bool) -> Option<Color32> {
    let rgb = if dark {
        match kind {
            Kind::Comment => (106, 153, 85),
            Kind::String => (206, 145, 120),
            Kind::Keyword => (86, 156, 214),
            Kind::Type => (78, 201, 176),
            Kind::Function | Kind::Attribute => (220, 220, 170),
            Kind::Number => (181, 206, 168),
            Kind::Variable => (156, 220, 254),
            Kind::Operator | Kind::Punctuation => return None,
        }
    } else {
        match kind {
            Kind::Comment => (0, 128, 0),
            Kind::String => (163, 21, 21),
            Kind::Keyword => (0, 0, 255),
            Kind::Type => (38, 127, 153),
            Kind::Function | Kind::Attribute => (121, 94, 38),
            Kind::Number => (9, 134, 88),
            Kind::Variable => (0, 16, 128),
            Kind::Operator | Kind::Punctuation => return None,
        }
    };
    Some(Color32::from_rgb(rgb.0, rgb.1, rgb.2))
}

/// The spans of a line moved from byte offsets of its `raw` form to byte offsets of its
/// display `text` (tabs expanded, no ending).
pub fn in_text(raw: &str, text: &str, spans: &Spans) -> Spans {
    let byte_at = |col: usize| text.char_indices().nth(col).map_or(text.len(), |(b, _)| b);
    spans
        .iter()
        .map(|(r, k)| {
            let start = byte_at(display_column(raw, r.start));
            let end = byte_at(display_column(raw, r.end));
            (start..end, *k)
        })
        .filter(|(r, _)| r.start < r.end)
        .collect()
}

/// `text` cut where either its syntax (`spans`, byte ranges of `text`) or its changed
/// `words` change: each piece with its kind, and whether it is inside a changed word.
pub fn sections(
    text: &str,
    words: &[Range<usize>],
    spans: &[(Range<usize>, Kind)],
) -> Vec<(Range<usize>, Option<Kind>, bool)> {
    let mut cuts = vec![0, text.len()];
    cuts.extend(words.iter().flat_map(|r| [r.start, r.end]));
    cuts.extend(spans.iter().flat_map(|(r, _)| [r.start, r.end]));
    cuts.retain(|&c| c <= text.len());
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .map(|w| {
            let piece = w[0]..w[1];
            let kind = spans
                .iter()
                .find(|(r, _)| r.start <= piece.start && piece.end <= r.end)
                .map(|(_, k)| *k);
            let changed = words
                .iter()
                .any(|r| r.start <= piece.start && piece.end <= r.end);
            (piece, kind, changed)
        })
        .collect()
}
