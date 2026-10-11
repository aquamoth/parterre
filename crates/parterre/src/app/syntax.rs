use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{Color32, FontId, Stroke, Visuals};
use parterre_core::file_diff::display_map;
use parterre_highlight::{Kind, Spans};
use std::ops::Range;

/// VS Code's Light+ and Dark+ colours (#208).
fn color(kind: Kind, dark: bool) -> Option<Color32> {
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

fn text_format(
    kind: Option<Kind>,
    font: FontId,
    visuals: &Visuals,
    background: Color32,
) -> TextFormat {
    let color = kind
        .and_then(|k| color(k, visuals.dark_mode))
        .unwrap_or_else(|| visuals.text_color());
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

/// A line's `text` in the colours of its syntax `spans`, its changed `words` over
/// `word_background`.
pub fn layout_job(
    text: &str,
    words: &[Range<usize>],
    spans: &[(Range<usize>, Kind)],
    font: &FontId,
    visuals: &Visuals,
    word_background: Color32,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    for (piece, kind, in_word) in sections(text, words, spans) {
        let background = if in_word {
            word_background
        } else {
            Color32::TRANSPARENT
        };
        let format = text_format(kind, font.clone(), visuals, background);
        job.append(&text[piece], 0.0, format);
    }
    job
}

/// The spans of the `raw` line moved onto its display text, as
/// [`parterre_core::file_diff::display`] moves changed words.
pub fn display_spans(raw: &str, spans: &[(Range<usize>, Kind)]) -> Spans {
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

/// `text` cut wherever its syntax `spans` or its changed `words` (byte ranges, each in order
/// and not overlapping) start or end: each piece with its kind, and whether it is in a word.
fn sections(
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
    let (mut next_span, mut next_word) = (0, 0);
    cuts.windows(2)
        .map(|w| {
            let piece = w[0]..w[1];
            while next_span < spans.len() && spans[next_span].0.end <= piece.start {
                next_span += 1;
            }
            let kind = spans
                .get(next_span)
                .filter(|(r, _)| r.start <= piece.start && piece.end <= r.end)
                .map(|(_, k)| *k);
            while next_word < words.len() && words[next_word].end <= piece.start {
                next_word += 1;
            }
            let in_word = words
                .get(next_word)
                .is_some_and(|r| r.start <= piece.start && piece.end <= r.end);
            (piece, kind, in_word)
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
        let moved = display_spans(raw, &spans);
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
        assert_eq!(display_spans(raw, &spans), vec![(0..2, Kind::Keyword)]);
    }
}
