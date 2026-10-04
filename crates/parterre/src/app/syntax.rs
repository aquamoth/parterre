//! Colours for the syntax spans of `parterre_core::highlight` (#209), and the sections a
//! line's text is laid out in when its syntax colours and its changed words (background)
//! are combined.

use eframe::egui::text::TextFormat;
use eframe::egui::{Color32, FontId, Stroke};
use parterre_core::file_diff::display_map;
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
/// display text (tabs expanded, no ending), as `file_diff::display` moves the changed
/// words; one that lands empty (a lone carriage return) is dropped. Once per line when a
/// file is loaded, not when it is drawn.
pub fn moved(raw: &str, spans: &[(Range<usize>, Kind)]) -> Spans {
    if spans.is_empty() {
        return Vec::new();
    }
    let (_, map) = display_map(raw);
    let end = map.len() - 1;
    spans
        .iter()
        .filter_map(|(r, k)| {
            let (s, e) = (map[r.start.min(end)], map[r.end.min(end)]);
            (e > s).then_some((s..e, *k))
        })
        .collect()
}

/// `text` cut where either its syntax (`spans`) or its changed `words` change, both byte
/// ranges of `text` in order and not overlapping: each piece with its kind, and whether it
/// is inside a changed word. One pass over the boundaries.
pub fn sections(
    text: &str,
    words: &[Range<usize>],
    spans: &[(Range<usize>, Kind)],
) -> Vec<(Range<usize>, Option<Kind>, bool)> {
    let mut cuts = Vec::with_capacity(2 * (words.len() + spans.len()) + 2);
    cuts.push(0);
    cuts.push(text.len());
    cuts.extend(words.iter().flat_map(|r| [r.start, r.end]));
    cuts.extend(spans.iter().flat_map(|(r, _)| [r.start, r.end]));
    cuts.retain(|&c| c <= text.len());
    cuts.sort_unstable();
    cuts.dedup();
    let (mut si, mut wi) = (0, 0);
    cuts.windows(2)
        .map(|w| {
            let piece = w[0]..w[1];
            while si < spans.len() && spans[si].0.end <= piece.start {
                si += 1;
            }
            let kind = spans
                .get(si)
                .filter(|(r, _)| r.start <= piece.start && piece.end <= r.end)
                .map(|(_, k)| *k);
            while wi < words.len() && words[wi].end <= piece.start {
                wi += 1;
            }
            let changed = words
                .get(wi)
                .is_some_and(|r| r.start <= piece.start && piece.end <= r.end);
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
    fn sections_follow_many_spans_in_one_pass() {
        // A long line of many tokens: every piece lands on its own span.
        let text = "a ".repeat(5000);
        let spans: Vec<_> = (0..5000)
            .map(|i| (2 * i..2 * i + 1, Kind::Variable))
            .collect();
        let got = sections(&text, &[], &spans);
        assert_eq!(got.len(), 10000);
        assert!(
            got.iter()
                .step_by(2)
                .all(|(_, k, _)| *k == Some(Kind::Variable))
        );
        assert!(got.iter().skip(1).step_by(2).all(|(_, k, _)| k.is_none()));
    }

    #[test]
    fn spans_follow_tab_expansion() {
        let raw = "\tif x";
        let (text, _) = display_map(raw);
        let spans = vec![(1..3, Kind::Keyword)];
        let moved = moved(raw, &spans);
        assert_eq!(&text[moved[0].0.clone()], "if");
    }

    #[test]
    fn a_span_that_lands_empty_takes_no_other_spans_kind() {
        let raw = "if\r";
        let spans = vec![
            (0..0, Kind::Comment),
            (0..2, Kind::Keyword),
            (2..3, Kind::Comment),
        ];
        assert_eq!(moved(raw, &spans), vec![(0..2, Kind::Keyword)]);
    }
}
