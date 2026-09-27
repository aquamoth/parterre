//! Text helpers for the windows, free of any GUI: web links in commit messages, paths cut at
//! the start, counts with thousands separators (and line numbers typed with them), and the word
//! under a double-click.

use std::ops::Range;

/// The byte ranges of the plain `http://` and `https://` URLs in `text`.
///
/// A URL runs to the next whitespace or `<`, `>`, `"`. Punctuation that more likely ends the
/// sentence than the URL is left out: a trailing `.`, `,`, `;`, `:`, `!`, `?` or `'`, and a
/// closing bracket without an opening one inside the URL (`(see https://x.org/a)`).
pub fn find_urls(text: &str) -> Vec<Range<usize>> {
    let mut urls = Vec::new();
    let mut from = 0;
    while let Some(found) = next_scheme(&text[from..]) {
        let start = from + found;
        let len = text[start..]
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"'))
            .unwrap_or(text.len() - start);
        let mut end = start + len;
        loop {
            let url = &text[start..end];
            let Some(last) = url.chars().last() else {
                break;
            };
            let unbalanced = |open: char| url.matches(open).count() < url.matches(last).count();
            let trim = match last {
                '.' | ',' | ';' | ':' | '!' | '?' | '\'' => true,
                ')' => unbalanced('('),
                ']' => unbalanced('['),
                '}' => unbalanced('{'),
                _ => false,
            };
            if !trim {
                break;
            }
            end -= last.len_utf8();
        }
        // Nothing after the scheme: not a link.
        let scheme_len = if text[start..].starts_with("https") {
            8
        } else {
            7
        };
        if end > start + scheme_len {
            urls.push(start..end);
        }
        from = end.max(start + scheme_len);
    }
    urls
}

/// Offset of the next `http://` or `https://` that starts a word.
fn next_scheme(text: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = text[from..].find("http") {
        let at = from + i;
        let rest = &text[at..];
        let word_start = text[..at]
            .chars()
            .last()
            .is_none_or(|c| !c.is_alphanumeric());
        if word_start && (rest.starts_with("http://") || rest.starts_with("https://")) {
            return Some(at);
        }
        from = at + 4;
    }
    None
}

/// `text`, or its longest end that fits after an ellipsis (`…` then the end) when the whole
/// doesn't. `fits` measures a candidate. Used for paths, so that the file name stays in view.
pub fn elide_start(text: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(text) {
        return text.to_owned();
    }
    // Char boundaries where the kept end may start; binary search for the earliest that fits.
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).skip(1).collect();
    let (mut lo, mut hi) = (0, starts.len());
    let candidate = |k: usize| match starts.get(k) {
        Some(&i) => format!("…{}", &text[i..]),
        None => "…".to_owned(),
    };
    while lo < hi {
        let mid = (lo + hi) / 2;
        if fits(&candidate(mid)) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    candidate(lo)
}

/// `n` with commas between thousands: `13,786`.
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The line (from 0) that `text` names as a line number from 1 to `lines`, as typed into a
/// "go to line" field: digits, blanks around them, and commas only where [`thousands`] puts
/// them. `None` for anything else, or a number out of range.
pub fn line_number(text: &str, lines: usize) -> Option<usize> {
    let text = text.trim();
    let digits: String = text.chars().filter(|&c| c != ',').collect();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: usize = digits.parse().ok()?;
    if digits.len() != text.len() && thousands(n) != text {
        return None;
    }
    (1..=lines).contains(&n).then(|| n - 1)
}

/// The word (or run of blanks, or other character) at character `col` of `text`, as a range
/// of characters; the last character's when `col` is past the end. `None` for an empty text.
pub fn word_at(text: &str, col: usize) -> Option<Range<usize>> {
    let text: Vec<char> = text.chars().collect();
    let col = col.min(text.len().checked_sub(1)?);
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    };
    let k = class(text[col]);
    if k == 2 {
        return Some(col..col + 1);
    }
    let start = (0..col)
        .rev()
        .take_while(|&i| class(text[i]) == k)
        .last()
        .unwrap_or(col);
    let end = (col..text.len())
        .take_while(|&i| class(text[i]) == k)
        .last()
        .unwrap_or(col)
        + 1;
    Some(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(text: &str) -> Vec<&str> {
        find_urls(text).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn finds_http_and_https_urls() {
        assert_eq!(
            urls("See https://github.com/aquamoth/parterre/issues/28 and http://x.org/a?b=c#d"),
            [
                "https://github.com/aquamoth/parterre/issues/28",
                "http://x.org/a?b=c#d"
            ]
        );
        assert_eq!(
            urls("https://a.b/c\nhttps://d.e"),
            ["https://a.b/c", "https://d.e"]
        );
        assert!(urls("no links, ftp://x.org, shttp://x.org or https:// alone").is_empty());
    }

    #[test]
    fn leaves_out_the_punctuation_around_urls() {
        assert_eq!(urls("Fixed (see https://x.org/a)."), ["https://x.org/a"]);
        assert_eq!(
            urls("https://en.wikipedia.org/wiki/Rust_(language), too"),
            ["https://en.wikipedia.org/wiki/Rust_(language)"]
        );
        assert_eq!(urls("<https://x.org/a>"), ["https://x.org/a"]);
        assert_eq!(urls("\"https://x.org/ä\"!"), ["https://x.org/ä"]);
        assert_eq!(urls("[link](https://x.org/b)"), ["https://x.org/b"]);
    }

    #[test]
    fn elides_at_the_start_so_the_end_stays() {
        let fits = |n: usize| move |s: &str| s.chars().count() <= n;
        assert_eq!(elide_start("src/app/log.rs", fits(20)), "src/app/log.rs");
        assert_eq!(elide_start("src/app/log.rs", fits(10)), "…pp/log.rs");
        assert_eq!(elide_start("src/app/log.rs", fits(1)), "…");
        assert_eq!(elide_start("src/app/log.rs", fits(0)), "…");
        assert_eq!(elide_start("dir/☃☃☃.txt", fits(8)), "…☃☃☃.txt");
        assert_eq!(elide_start("", fits(0)), "");
    }

    #[test]
    fn groups_thousands() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(13786), "13,786");
        assert_eq!(thousands(1234567), "1,234,567");
    }

    #[test]
    fn line_numbers_as_typed() {
        assert_eq!(line_number("1", 1429), Some(0));
        assert_eq!(line_number(" 250 ", 1429), Some(249));
        assert_eq!(line_number("1429", 1429), Some(1428));
        // As the range shows it, with the thousands separated.
        assert_eq!(line_number("1,429", 1429), Some(1428));
        assert_eq!(line_number("007", 1429), Some(6));
        // Out of range.
        assert_eq!(line_number("0", 1429), None);
        assert_eq!(line_number("1430", 1429), None);
        assert_eq!(line_number("99999999999999999999999", 1429), None);
        assert_eq!(line_number("1", 0), None);
        // Not a line number.
        for text in [
            "", "  ", "-3", "+3", "1.5", "12a", "1 2", ",", "14,29", "1,4290",
        ] {
            assert_eq!(line_number(text, 100_000), None, "{text:?}");
        }
    }

    #[test]
    fn a_word_is_letters_digits_and_underscores_or_a_run_of_blanks() {
        let text = "let näme_2  = a.b;";
        let at = |col| {
            word_at(text, col).map(|r| text.chars().skip(r.start).take(r.len()).collect::<String>())
        };
        assert_eq!(at(5).as_deref(), Some("näme_2"));
        assert_eq!(at(11).as_deref(), Some("  "));
        assert_eq!(at(14).as_deref(), Some("a"));
        assert_eq!(at(15).as_deref(), Some("."));
        // Past the end: the last character's.
        assert_eq!(at(99).as_deref(), Some(";"));
        assert_eq!(word_at("", 0), None);
    }
}
