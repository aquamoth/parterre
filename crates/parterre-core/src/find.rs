use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub line: usize,
    /// Bytes of the line.
    pub range: Range<usize>,
}

/// Ignores case; matches don't overlap, and an empty query matches nothing.
pub fn find<S: AsRef<str>>(lines: impl IntoIterator<Item = S>, query: &str) -> Vec<Match> {
    let query = query.to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for (line, text) in lines.into_iter().enumerate() {
        let text = text.as_ref();
        let (lower, origin) = lowercase_with_origins(text);
        for (start, _) in lower.match_indices(&query) {
            let end = start + query.len();
            // A character can lower to several, as 'İ' does: a match must cover whole ones.
            let whole = (start == 0 || origin[start - 1] != origin[start])
                && origin[end - 1] != origin[end];
            if whole {
                found.push(Match {
                    line,
                    range: origin[start]..origin[end],
                });
            }
        }
    }
    found
}

pub trait Place {
    fn line(&self) -> usize;
}

impl Place for Match {
    fn line(&self) -> usize {
        self.line
    }
}

/// A row found as a whole, as the log's are.
impl Place for usize {
    fn line(&self) -> usize {
        *self
    }
}

/// `text` lowercased, and for each of its bytes, plus its end, where in `text` the character
/// it came from starts.
fn lowercase_with_origins(text: &str) -> (String, Vec<usize>) {
    let mut lower = String::with_capacity(text.len());
    let mut origin = Vec::with_capacity(text.len() + 1);
    for (i, c) in text.char_indices() {
        let before = lower.len();
        lower.extend(c.to_lowercase());
        origin.resize(origin.len() + lower.len() - before, i);
    }
    origin.push(text.len());
    (lower, origin)
}

/// The first match at or after `line`, else the first of all.
pub fn first_from<P: Place>(matches: &[P], line: usize) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let at = matches.partition_point(|m| m.line() < line);
    Some(if at < matches.len() { at } else { 0 })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Next,
    Previous,
}

/// Goes round the ends. With no match current, starts from `line`.
pub fn step<P: Place>(
    matches: &[P],
    current: Option<usize>,
    step: Step,
    line: usize,
) -> Option<usize> {
    let n = matches.len();
    if n == 0 {
        return None;
    }
    Some(match (current, step) {
        (Some(c), Step::Next) => (c + 1) % n,
        (Some(c), Step::Previous) => (c + n - 1) % n,
        (None, Step::Next) => first_from(matches, line)?,
        (None, Step::Previous) => {
            let at = matches.partition_point(|m| m.line() < line);
            (at + n - 1) % n
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places(lines: &[&str], query: &str) -> Vec<(usize, String)> {
        find(lines, query)
            .into_iter()
            .map(|m| (m.line, lines[m.line][m.range].to_owned()))
            .collect()
    }

    #[test]
    fn finds_every_place_ignoring_case() {
        let lines = ["let Foo = foo(FOO);", "none here", "\tfoofoo"];
        assert_eq!(
            places(&lines, "foo"),
            [
                (0, "Foo".into()),
                (0, "foo".into()),
                (0, "FOO".into()),
                (2, "foo".into()),
                (2, "foo".into()),
            ]
        );
        assert_eq!(places(&lines, "FOO("), [(0, "foo(".into())]);
        // The query as typed: blanks count.
        assert_eq!(places(&lines, " here"), [(1, " here".into())]);
        assert_eq!(places(&lines, "\tf"), [(2, "\tf".into())]);
        assert!(find(lines, "").is_empty());
        assert!(find(lines, "bar").is_empty());
    }

    #[test]
    fn places_do_not_overlap() {
        assert_eq!(
            places(&["aaaa"], "aa"),
            [(0, "aa".into()), (0, "aa".into())]
        );
    }

    #[test]
    fn ranges_are_bytes_of_the_line_as_it_is() {
        let lines = ["Ärger über ÄRGER", "İstanbul"];
        assert_eq!(
            places(&lines, "ärger"),
            [(0, "Ärger".into()), (0, "ÄRGER".into())]
        );
        assert_eq!(find(lines, "über")[0].range, 7..12);
        // 'İ' lowers to "i̇": the whole of it matches, not half of it.
        assert_eq!(places(&lines, "i̇stan"), [(1, "İstan".into())]);
        assert!(find(lines, "i").is_empty());
    }

    fn at(lines: &[usize]) -> Vec<Match> {
        lines
            .iter()
            .map(|&line| Match { line, range: 0..1 })
            .collect()
    }

    #[test]
    fn steps_round_the_ends() {
        let matches = at(&[2, 5, 5, 9]);
        assert_eq!(step(&matches, Some(0), Step::Next, 0), Some(1));
        assert_eq!(step(&matches, Some(3), Step::Next, 0), Some(0));
        assert_eq!(step(&matches, Some(0), Step::Previous, 0), Some(3));
        assert_eq!(step(&matches, Some(2), Step::Previous, 0), Some(1));
        assert_eq!(step::<Match>(&[], Some(0), Step::Next, 0), None);
    }

    #[test]
    fn with_none_current_steps_from_a_line() {
        let matches = at(&[2, 5, 5, 9]);
        assert_eq!(first_from(&matches, 0), Some(0));
        assert_eq!(first_from(&matches, 5), Some(1));
        assert_eq!(first_from(&matches, 6), Some(3));
        // Past the last: round to the first.
        assert_eq!(first_from(&matches, 10), Some(0));
        assert_eq!(first_from::<Match>(&[], 3), None);
        assert_eq!(step(&matches, None, Step::Next, 6), Some(3));
        assert_eq!(step(&matches, None, Step::Previous, 6), Some(2));
        assert_eq!(step(&matches, None, Step::Previous, 0), Some(3));
    }
}
