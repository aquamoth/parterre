//! PROTOTYPE (#208): syntax colour through tree-sitter, with SQL as the single grammar.
//!
//! The engine lives behind the `syntax` feature and hands out language-neutral spans: what
//! each piece of a line is ([`Kind`]), never a colour. The app maps a kind to a colour of its
//! palette.

use std::ops::Range;

/// What a span of text is, for colouring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Comment,
    String,
    Keyword,
    Type,
    Function,
    Number,
    Operator,
    Punctuation,
    Variable,
    Attribute,
}

/// A line's coloured spans: byte ranges of the line (without its ending), in order, not
/// overlapping.
pub type Spans = Vec<(Range<usize>, Kind)>;

/// A language the engine has a grammar for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Sql,
}

/// The language of a file, from its path. `None` for one the engine can't colour, or when
/// the `syntax` feature is off.
pub fn language_of(path: &str) -> Option<Language> {
    if !cfg!(feature = "syntax") {
        return None;
    }
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let (_, ext) = name.rsplit_once('.')?;
    match ext.to_ascii_lowercase().as_str() {
        "sql" => Some(Language::Sql),
        _ => None,
    }
}

/// Highlights a whole file (`text`, line endings included) as `language`: the spans of each
/// line, in order. Byte ranges are of the line without its ending, so a span that runs over
/// several lines (a block comment) is cut into one piece per line.
pub fn highlight(language: Language, text: &str) -> Vec<Spans> {
    #[cfg(feature = "syntax")]
    {
        engine::highlight(language, text)
    }
    #[cfg(not(feature = "syntax"))]
    {
        let _ = (language, text);
        Vec::new()
    }
}

#[cfg(feature = "syntax")]
mod engine {
    use super::{Kind, Language, Spans};
    use std::sync::OnceLock;
    use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

    /// The capture names recognised, and what each means. `configure` maps a grammar's
    /// capture to the longest recognised dotted prefix, so `keyword.operator` counts as
    /// `keyword` and `function.call` as `function`.
    const NAMES: &[(&str, Kind)] = &[
        ("attribute", Kind::Attribute),
        ("boolean", Kind::Number),
        ("comment", Kind::Comment),
        ("conditional", Kind::Keyword),
        ("constant", Kind::Number),
        ("field", Kind::Variable),
        ("float", Kind::Number),
        ("function", Kind::Function),
        ("keyword", Kind::Keyword),
        ("number", Kind::Number),
        ("operator", Kind::Operator),
        ("parameter", Kind::Variable),
        ("punctuation", Kind::Punctuation),
        ("repeat", Kind::Keyword),
        ("storageclass", Kind::Keyword),
        ("string", Kind::String),
        ("type", Kind::Type),
        ("variable", Kind::Variable),
    ];

    /// The grammar and its queries, compiled once.
    fn config(language: Language) -> &'static HighlightConfiguration {
        static SQL: OnceLock<HighlightConfiguration> = OnceLock::new();
        match language {
            Language::Sql => SQL.get_or_init(|| {
                let mut c = HighlightConfiguration::new(
                    tree_sitter_sequel::LANGUAGE.into(),
                    "sql",
                    tree_sitter_sequel::HIGHLIGHTS_QUERY,
                    "",
                    "",
                )
                .expect("the SQL highlight query compiles");
                c.configure(&NAMES.iter().map(|(n, _)| *n).collect::<Vec<_>>());
                c
            }),
        }
    }

    pub fn highlight(language: Language, text: &str) -> Vec<Spans> {
        // Where each line starts, and where its content ends (before `\n` or `\r\n`).
        let mut starts = vec![0];
        let mut ends = Vec::new();
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                let end = if i > 0 && text.as_bytes()[i - 1] == b'\r' {
                    i - 1
                } else {
                    i
                };
                ends.push(end);
                starts.push(i + 1);
            }
        }
        if *starts.last().unwrap() < text.len() {
            ends.push(text.len());
        } else {
            starts.pop();
        }
        let mut lines: Vec<Spans> = vec![Vec::new(); starts.len()];
        let mut highlighter = Highlighter::new();
        let Ok(events) =
            highlighter.highlight(config(language), text.as_bytes(), None, None, |_| None)
        else {
            return lines;
        };
        let mut stack: Vec<Kind> = Vec::new();
        for event in events {
            let Ok(event) = event else { break };
            match event {
                HighlightEvent::HighlightStart(h) => stack.push(NAMES[h.0].1),
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } => {
                    let Some(&kind) = stack.last() else { continue };
                    let mut i = starts.partition_point(|&s| s <= start).saturating_sub(1);
                    let mut at = start;
                    while at < end && i < starts.len() {
                        let (base, content_end) = (starts[i], ends[i]);
                        let stop = end.min(content_end);
                        if at < stop {
                            lines[i].push((at - base..stop - base, kind));
                        }
                        i += 1;
                        at = starts.get(i).copied().unwrap_or(end);
                    }
                }
            }
        }
        lines
    }
}

#[cfg(all(test, feature = "syntax"))]
mod tests {
    use super::*;

    #[test]
    fn sql_keywords_strings_and_comments_are_found() {
        let text = "/* a comment\n   over two lines */\nSELECT name FROM t WHERE id = 'x';\n";
        let lines = highlight(Language::Sql, text);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], vec![(0..12, Kind::Comment)]);
        assert_eq!(lines[1], vec![(0..20, Kind::Comment)]);
        let base = text.find("SELECT").unwrap();
        let kinds: Vec<_> = lines[2]
            .iter()
            .map(|(r, k)| (&text[base + r.start..base + r.end], *k))
            .collect();
        assert!(kinds.contains(&("SELECT", Kind::Keyword)), "{kinds:?}");
        assert!(kinds.contains(&("'x'", Kind::String)), "{kinds:?}");
    }

    #[test]
    fn crlf_lines_keep_their_offsets() {
        let text = "SELECT 1;\r\nSELECT 2;\r\n";
        let lines = highlight(Language::Sql, text);
        assert_eq!(lines.len(), 2);
        assert!(
            lines[1]
                .iter()
                .any(|(r, k)| *k == Kind::Keyword && r.start == 0)
        );
        assert!(lines[1].iter().all(|(r, _)| r.end <= 9));
    }

    #[test]
    fn language_from_path() {
        assert_eq!(language_of("db/schema.SQL"), Some(Language::Sql));
        assert_eq!(language_of("src/main.rs"), None);
        assert_eq!(language_of("Makefile"), None);
    }
}
