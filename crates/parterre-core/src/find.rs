//! Finding text in the lines of a file, as the blame and diff windows' find fields do: every
//! place a query occurs, ignoring case as the main window's find does, and stepping from one
//! to the next or previous, round the ends.

use std::ops::Range;

/// A place the query occurs: a line (an index into the lines searched) and a range in it.
/// [`find`] gives byte ranges in the lines as searched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub line: usize,
    pub range: Range<usize>,
}

/// Every place `query` occurs in `lines`, in order, ignoring case (compared in lower case);
/// places in a line don't overlap. An empty query occurs nowhere.
pub fn find<S: AsRef<str>>(lines: impl IntoIterator<Item = S>, query: &str) -> Vec<Match> {
    let query = query.to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for (line, text) in lines.into_iter().enumerate() {
        let text = text.as_ref();
        let (folded, from) = fold(text);
        for (start, _) in folded.match_indices(&query) {
            let end = start + query.len();
            // A match must start and end where whole characters of the line do (a character
            // can lower to several, as 'İ' does).
            let whole =
                (start == 0 || from[start - 1] != from[start]) && from[end - 1] != from[end];
            if whole {
                found.push(Match {
                    line,
                    range: from[start]..from[end],
                });
            }
        }
    }
    found
}

/// A place found, by the line it is on, for stepping with [`first_from`] and [`step`].
pub trait Place {
    fn line(&self) -> usize;
}

impl Place for Match {
    fn line(&self) -> usize {
        self.line
    }
}

/// A line found as a whole, known by its number alone (the log's rows).
impl Place for usize {
    fn line(&self) -> usize {
        *self
    }
}

/// `text` in lower case, and for each of its bytes (and its end) the byte in `text` where the
/// character it came from starts (and `text`'s end).
fn fold(text: &str) -> (String, Vec<usize>) {
    let mut folded = String::with_capacity(text.len());
    let mut from = Vec::with_capacity(text.len() + 1);
    for (i, c) in text.char_indices() {
        let before = folded.len();
        folded.extend(c.to_lowercase());
        from.resize(from.len() + folded.len() - before, i);
    }
    from.push(text.len());
    (folded, from)
}

/// The first match at or after line `line`, else the first of all (going round the end).
pub fn first_from<P: Place>(matches: &[P], line: usize) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let at = matches.partition_point(|m| m.line() < line);
    Some(if at < matches.len() { at } else { 0 })
}

/// The match after `current` (`forward`) or before it, going round the ends. With none
/// current, the first at or after line `line` going forward, or the last before it going
/// back.
pub fn step<P: Place>(
    matches: &[P],
    current: Option<usize>,
    forward: bool,
    line: usize,
) -> Option<usize> {
    let n = matches.len();
    if n == 0 {
        return None;
    }
    Some(match (current, forward) {
        (Some(c), true) => (c + 1) % n,
        (Some(c), false) => (c + n - 1) % n,
        (None, true) => first_from(matches, line)?,
        (None, false) => {
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
        assert_eq!(step(&matches, Some(0), true, 0), Some(1));
        assert_eq!(step(&matches, Some(3), true, 0), Some(0));
        assert_eq!(step(&matches, Some(0), false, 0), Some(3));
        assert_eq!(step(&matches, Some(2), false, 0), Some(1));
        assert_eq!(step::<Match>(&[], Some(0), true, 0), None);
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
        assert_eq!(step(&matches, None, true, 6), Some(3));
        assert_eq!(step(&matches, None, false, 6), Some(2));
        assert_eq!(step(&matches, None, false, 0), Some(3));
    }
}
