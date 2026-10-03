//! Colours for the syntax spans of `parterre_core::highlight` (#209), and the sections a
//! line's text is laid out in when its syntax colours and its changed words (background)
//! are combined.

use eframe::egui::text::TextFormat;
use eframe::egui::{Color32, FontId, Stroke};
use parterre_core::file_diff::display_column;
use parterre_core::highlight::{Kind, Spans};
use std::ops::Range;

/// The colour of a kind of span, or `None` for the plain text colour. VS Code's Light+ and
/// Dark+ themes, as decided in #208.
pub fn color(kind: Kind, dark: bool) -> Option<Color32> {
    let rgb = if dark {
        match kind {
            Kind::Comment => (106, 153, 85),
            Kind::String | Kind::Raw => (206, 145, 120),
            Kind::Escape => (215, 186, 125),
            Kind::Keyword | Kind::Constant | Kind::Tag | Kind::Heading | Kind::Strong => {
                (86, 156, 214)
            }
            Kind::Type => (78, 201, 176),
            Kind::Function => (220, 220, 170),
            Kind::Number => (181, 206, 168),
            Kind::Variable | Kind::Property | Kind::Attribute => (156, 220, 254),
            Kind::Link => (86, 156, 214),
            Kind::Operator | Kind::Punctuation | Kind::Emphasis => return None,
        }
    } else {
        match kind {
            Kind::Comment => (0, 128, 0),
            Kind::String => (163, 21, 21),
            Kind::Escape => (238, 0, 0),
            Kind::Keyword | Kind::Constant | Kind::Link => (0, 0, 255),
            Kind::Type => (38, 127, 153),
            Kind::Function => (121, 94, 38),
            Kind::Number => (9, 134, 88),
            Kind::Variable | Kind::Property => (0, 16, 128),
            Kind::Attribute => (229, 0, 0),
            Kind::Tag | Kind::Heading | Kind::Raw => (128, 0, 0),
            Kind::Strong => (0, 0, 128),
            Kind::Operator | Kind::Punctuation | Kind::Emphasis => return None,
        }
    };
    Some(Color32::from_rgb(rgb.0, rgb.1, rgb.2))
}

/// How a piece of a line is drawn: its kind's colour (or `plain`), emphasis in italics,
/// links underlined, over `background`.
pub fn text_format(
    kind: Option<Kind>,
    font: FontId,
    dark: bool,
    plain: Color32,
    background: Color32,
) -> TextFormat {
    let color = kind.and_then(|k| color(k, dark)).unwrap_or(plain);
    TextFormat {
        font_id: font,
        color,
        background,
        italics: kind == Some(Kind::Emphasis),
        underline: if kind == Some(Kind::Link) {
            Stroke::new(1.0, color)
        } else {
            Stroke::NONE
        },
        ..Default::default()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_cut_at_both_kinds_of_boundary() {
        let text = "let x = 10;";
        let words = vec![8..10, 0..0];
        let spans = vec![(0..3, Kind::Keyword), (8..10, Kind::Number)];
        let got = sections(text, &words, &spans);
        assert_eq!(
            got,
            vec![
                (0..3, Some(Kind::Keyword), false),
                (3..8, None, false),
                (8..10, Some(Kind::Number), true),
                (10..11, None, false),
            ]
        );
    }

    #[test]
    fn spans_follow_tab_expansion() {
        let raw = "\tif x";
        let (text, _) = parterre_core::file_diff::display(raw, &[]);
        let spans = vec![(1..3, Kind::Keyword)];
        let moved = in_text(raw, &text, &spans);
        assert_eq!(&text[moved[0].0.clone()], "if");
    }
}
