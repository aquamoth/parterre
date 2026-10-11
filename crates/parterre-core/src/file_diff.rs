//! Git reads the two versions, through the user's textconv filters, and decides what is
//! binary; lines and words are compared here with `imara-diff`: histogram, then git's indent
//! heuristic (#44, #45, #46).

use std::borrow::Cow;
use std::fmt::Write as _;
use std::ops::Range;

use imara_diff::{Algorithm, Diff, InternedInput};
use serde::{Deserialize, Serialize};

use crate::changed_files::{ChangedFile, FileStatus};
use crate::oid::Oid;

pub const GITLINK_MODE: u32 = 0o160000;
const TAB_WIDTH: usize = 4;
/// git's default `diff.context`.
pub const CONTEXT_LINES: usize = 3;
const PAIRING_LOOKAHEAD: usize = 12;
const MIN_PAIRING_SIMILARITY: f32 = 0.5;
/// Larger changes are marked whole: comparing their words could stall the window.
const MAX_WORD_DIFF_LINES: usize = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rev {
    Commit(Oid),
    /// The files on disk, staged or not, as `git diff` reads them: through the clean filter and
    /// line-ending conversion.
    WorkingTree,
}

impl Rev {
    pub fn commit(self) -> Option<Oid> {
        match self {
            Rev::Commit(oid) => Some(oid),
            Rev::WorkingTree => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    pub rev: Rev,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileDiffSpec {
    pub old: Option<Version>,
    pub new: Option<Version>,
    pub status: FileStatus,
    /// Old and new; 0 for a missing side.
    pub modes: [u32; 2],
    pub binary: bool,
}

impl FileDiffSpec {
    pub fn of_commit(commit: Oid, parent: Option<Oid>, file: &ChangedFile) -> FileDiffSpec {
        FileDiffSpec::between(parent.map(Rev::Commit), Rev::Commit(commit), file)
    }

    pub fn between(old: Option<Rev>, new: Rev, file: &ChangedFile) -> FileDiffSpec {
        let old = match old {
            Some(rev) if file.status != FileStatus::Added => Some(Version {
                rev,
                path: file.old_path.clone().unwrap_or_else(|| file.path.clone()),
            }),
            _ => None,
        };
        let new = (file.status != FileStatus::Deleted).then(|| Version {
            rev: new,
            path: file.path.clone(),
        });
        FileDiffSpec {
            old,
            new,
            status: file.status,
            modes: file.modes,
            binary: file.is_binary(),
        }
    }

    pub fn reads_working_tree(&self) -> bool {
        [&self.old, &self.new]
            .into_iter()
            .flatten()
            .any(|v| v.rev == Rev::WorkingTree)
    }

    pub fn is_submodule(&self) -> bool {
        self.modes.contains(&GITLINK_MODE)
    }

    pub fn path(&self) -> &str {
        self.new
            .as_ref()
            .or(self.old.as_ref())
            .map_or("", |v| v.path.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    /// A missing side is empty.
    Text {
        old: String,
        new: String,
        invalid_bytes: usize,
    },
    Binary {
        old_size: Option<u64>,
        new_size: Option<u64>,
    },
    Submodule {
        old: Option<Oid>,
        new: Option<Oid>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedDiff {
    pub spec: FileDiffSpec,
    pub content: Content,
    /// `driver: command` of the textconv filter that converted the text.
    pub textconv: Option<String>,
}

/// Writes each byte that isn't valid UTF-8 as `\xNN`, and counts them.
pub fn decode(bytes: &[u8]) -> (String, usize) {
    let mut out = String::with_capacity(bytes.len());
    let mut invalid = 0;
    for chunk in bytes.utf8_chunks() {
        out.push_str(chunk.valid());
        for b in chunk.invalid() {
            let _ = write!(out, "\\x{b:02X}");
            invalid += 1;
        }
    }
    (out, invalid)
}

/// Which removed line's words are compared with which added line's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WordMode {
    /// Each removed line with the most similar added line close by. Paired lines share a row
    /// side by side; the other lines of a change are marked whole.
    #[default]
    Similar,
    /// The n-th removed line of a change with its n-th added line.
    Position,
    /// All removed lines of a change with all its added lines, as one text.
    Block,
    Off,
}

impl WordMode {
    pub const ALL: [WordMode; 4] = [
        WordMode::Similar,
        WordMode::Position,
        WordMode::Block,
        WordMode::Off,
    ];

    pub fn label(self) -> &'static str {
        match self {
            WordMode::Similar => "Similar",
            WordMode::Position => "By position",
            WordMode::Block => "Whole block",
            WordMode::Off => "Off",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Whitespace {
    #[default]
    Compare,
    /// As `git diff -b`, which also ignores a CRLF line ending's CR.
    IgnoreChanges,
    /// As `git diff -w`.
    IgnoreAll,
}

impl Whitespace {
    /// Bytes: imara-diff's indent heuristic reads lines as bytes.
    fn comparison_key(self, raw: &str) -> Cow<'_, [u8]> {
        match self {
            Whitespace::Compare => Cow::Borrowed(raw.as_bytes()),
            Whitespace::IgnoreChanges => {
                let mut out = String::with_capacity(raw.len());
                let mut after_blank = false;
                for c in raw.trim_end().chars() {
                    if c.is_whitespace() {
                        after_blank = true;
                    } else {
                        if after_blank {
                            out.push(' ');
                        }
                        after_blank = false;
                        out.push(c);
                    }
                }
                Cow::Owned(out.into_bytes())
            }
            Whitespace::IgnoreAll => {
                let solid: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
                Cow::Owned(solid.into_bytes())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DiffOptions {
    pub words: WordMode,
    pub whitespace: Whitespace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineKind {
    Same,
    Removed,
    Added,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    /// The line's index in its version.
    pub index: u32,
    pub kind: LineKind,
    /// As shown: no line ending, tabs expanded.
    pub text: String,
    /// As in the file, without the line ending.
    pub raw: String,
    /// Changed words, as byte ranges of `text`.
    pub spans: Vec<Range<usize>>,
}

/// Indexes into [`FileDiff::old`] and [`FileDiff::new`]. Side by side, a missing side is a
/// filler; unified, only an unchanged line has both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Row {
    pub old: Option<u32>,
    pub new: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Endings {
    Lf,
    Crlf,
    Mixed,
}

impl Endings {
    fn name(self) -> &'static str {
        match self {
            Endings::Lf => "LF",
            Endings::Crlf => "CRLF",
            Endings::Mixed => "mixed",
        }
    }

    /// Only lines with an ending count, so an unterminated last line doesn't make a file mixed.
    fn of(lines: &[&str]) -> Option<Endings> {
        let ended = lines.iter().filter(|l| l.ends_with('\n')).count();
        let crlf = lines.iter().filter(|l| l.ends_with("\r\n")).count();
        if ended == 0 {
            None
        } else if crlf == 0 {
            Some(Endings::Lf)
        } else if crlf == ended {
            Some(Endings::Crlf)
        } else {
            Some(Endings::Mixed)
        }
    }
}

/// What the diff window says about a diff besides its lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    Textconv(String),
    LineEndings {
        old: Endings,
        new: Endings,
        ignored: bool,
    },
    Mode {
        old: u32,
        new: u32,
    },
    InvalidUtf8(usize),
    Unchanged,
    OnlyWhitespace,
}

impl std::fmt::Display for Note {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Note::Textconv(driver) => write!(f, "Shown through textconv ({driver})"),
            Note::LineEndings { old, new, ignored } => {
                write!(f, "Line endings: {} → {}", old.name(), new.name())?;
                if *ignored {
                    write!(f, " (ignored)")?;
                }
                Ok(())
            }
            Note::Mode { old, new } => write!(f, "Mode {old:o} → {new:o}"),
            Note::InvalidUtf8(n) => write!(
                f,
                "{n} byte{} not valid UTF-8, shown as \\xNN",
                if *n == 1 { " is" } else { "s are" }
            ),
            Note::Unchanged => write!(f, "Content unchanged"),
            Note::OnlyWhitespace => write!(f, "Only whitespace changed (ignored)"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub old: Vec<DiffLine>,
    pub new: Vec<DiffLine>,
    /// Paired lines ([`WordMode::Similar`]) share a row.
    pub side: Vec<Row>,
    /// In git's order: a change's removed lines, then its added lines.
    pub unified: Vec<Row>,
    pub side_change_starts: Vec<usize>,
    pub unified_change_starts: Vec<usize>,
    /// As shown: after textconv and the whitespace setting.
    pub added: u32,
    pub removed: u32,
    pub old_endings: Option<Endings>,
    pub new_endings: Option<Endings>,
    /// In characters, tabs expanded.
    pub widest: usize,
}

impl FileDiff {
    pub fn new(old: &str, new: &str, options: DiffOptions) -> FileDiff {
        let old_lines: Vec<&str> = imara_diff::sources::lines(old).collect();
        let new_lines: Vec<&str> = imara_diff::sources::lines(new).collect();
        let whitespace = options.whitespace;
        let mut input = InternedInput::default();
        input.update_before(old_lines.iter().map(|&l| whitespace.comparison_key(l)));
        input.update_after(new_lines.iter().map(|&l| whitespace.comparison_key(l)));
        let mut line_diff = Diff::compute(Algorithm::Histogram, &input);
        line_diff.postprocess_lines(&input);

        let mut diff = FileDiff {
            added: line_diff.count_additions(),
            removed: line_diff.count_removals(),
            old_endings: Endings::of(&old_lines),
            new_endings: Endings::of(&new_lines),
            ..FileDiff::default()
        };
        let (mut o, mut n) = (0, 0);
        for hunk in line_diff.hunks() {
            for (o, n) in (o..hunk.before.start).zip(n..) {
                diff.push_unchanged(&old_lines, o, &new_lines, n);
            }
            (o, n) = (hunk.before.end, hunk.after.end);
            diff.push_change(&old_lines, hunk.before, &new_lines, hunk.after, options);
        }
        let unchanged_tail = old_lines.len() - o as usize;
        for (o, n) in (o..).zip(n..).take(unchanged_tail) {
            diff.push_unchanged(&old_lines, o, &new_lines, n);
        }
        diff.widest = diff
            .old
            .iter()
            .chain(&diff.new)
            .map(|l| l.text.chars().count())
            .max()
            .unwrap_or(0);
        diff
    }

    fn push_unchanged(&mut self, old_lines: &[&str], o: u32, new_lines: &[&str], n: u32) {
        self.old
            .push(DiffLine::new(old_lines[o as usize], o, LineKind::Same, &[]));
        self.new
            .push(DiffLine::new(new_lines[n as usize], n, LineKind::Same, &[]));
        let row = Row {
            old: Some(o),
            new: Some(n),
        };
        self.side.push(row);
        self.unified.push(row);
    }

    fn push_change(
        &mut self,
        old_lines: &[&str],
        removed: Range<u32>,
        new_lines: &[&str],
        added: Range<u32>,
        options: DiffOptions,
    ) {
        let removed_lines = &old_lines[removed.start as usize..removed.end as usize];
        let added_lines = &new_lines[added.start as usize..added.end as usize];
        let words = changed_words(removed_lines, added_lines, options);
        self.side_change_starts.push(self.side.len());
        self.unified_change_starts.push(self.unified.len());
        for ((raw, o), spans) in removed_lines
            .iter()
            .zip(removed.clone())
            .zip(&words.removed)
        {
            self.old
                .push(DiffLine::new(raw, o, LineKind::Removed, spans));
        }
        for ((raw, n), spans) in added_lines.iter().zip(added.clone()).zip(&words.added) {
            self.new.push(DiffLine::new(raw, n, LineKind::Added, spans));
        }
        let old_row = |o| Row {
            old: Some(o),
            new: None,
        };
        let new_row = |n| Row {
            old: None,
            new: Some(n),
        };
        self.unified.extend(removed.clone().map(old_row));
        self.unified.extend(added.clone().map(new_row));

        // Side by side, the lines between two pairs share rows in order.
        let old_index = |i: usize| removed.start + i as u32;
        let new_index = |j: usize| added.start + j as u32;
        let end = (removed_lines.len(), added_lines.len());
        let (mut i, mut j) = (0, 0);
        for (pair_i, pair_j) in words.pairs.into_iter().chain([end]) {
            for k in 0..(pair_i - i).max(pair_j - j) {
                self.side.push(Row {
                    old: (i + k < pair_i).then(|| old_index(i + k)),
                    new: (j + k < pair_j).then(|| new_index(j + k)),
                });
            }
            if (pair_i, pair_j) != end {
                self.side.push(Row {
                    old: Some(old_index(pair_i)),
                    new: Some(new_index(pair_j)),
                });
            }
            (i, j) = (pair_i + 1, pair_j + 1);
        }
    }

    /// `side`: `Some(true)` for the old line, `Some(false)` for the new one, `None` for the
    /// unified form's, which for an unchanged line is its new version.
    pub fn line(&self, row: Row, side: Option<bool>) -> Option<&DiffLine> {
        let old = row.old.map(|i| &self.old[i as usize]);
        let new = row.new.map(|i| &self.new[i as usize]);
        match side {
            Some(true) => old,
            Some(false) => new,
            None => new.or(old),
        }
    }

    fn is_changed(&self, row: Row) -> bool {
        let changed = |lines: &[DiffLine], i: Option<u32>| {
            i.is_some_and(|i| lines[i as usize].kind != LineKind::Same)
        };
        changed(&self.old, row.old) || changed(&self.new, row.new)
    }

    pub fn notes(&self, loaded: &LoadedDiff, whitespace: Whitespace) -> Vec<Note> {
        let mut notes = Vec::new();
        if let Some(driver) = &loaded.textconv {
            notes.push(Note::Textconv(driver.clone()));
        }
        let both_sides = loaded.spec.old.is_some() && loaded.spec.new.is_some();
        if let Content::Text {
            old,
            new,
            invalid_bytes,
        } = &loaded.content
        {
            if both_sides
                && let (Some(old_endings), Some(new_endings)) = (self.old_endings, self.new_endings)
                && old_endings != new_endings
            {
                notes.push(Note::LineEndings {
                    old: old_endings,
                    new: new_endings,
                    ignored: whitespace != Whitespace::Compare,
                });
            }
            if *invalid_bytes > 0 {
                notes.push(Note::InvalidUtf8(*invalid_bytes));
            }
            if both_sides && self.added == 0 && self.removed == 0 {
                notes.push(if old == new {
                    Note::Unchanged
                } else {
                    Note::OnlyWhitespace
                });
            }
        }
        let [old_mode, new_mode] = loaded.spec.modes;
        if both_sides && old_mode != new_mode && old_mode != 0 && new_mode != 0 {
            notes.push(Note::Mode {
                old: old_mode,
                new: new_mode,
            });
        }
        notes
    }
}

impl DiffLine {
    fn new(raw: &str, index: u32, kind: LineKind, changed: &[Range<usize>]) -> DiffLine {
        let (text, spans) = display(raw, changed);
        DiffLine {
            index,
            kind,
            text,
            raw: without_ending(raw).to_owned(),
            spans,
        }
    }

    pub fn raw_offset(&self, col: usize) -> usize {
        raw_offset(&self.raw, col)
    }
}

type LineSpans = Vec<Range<usize>>;

struct ChangedWords {
    removed: Vec<LineSpans>,
    added: Vec<LineSpans>,
    pairs: Vec<(usize, usize)>,
}

impl ChangedWords {
    fn unmarked(removed: &[&str], added: &[&str]) -> ChangedWords {
        ChangedWords {
            removed: vec![Vec::new(); removed.len()],
            added: vec![Vec::new(); added.len()],
            pairs: Vec::new(),
        }
    }

    fn whole_lines(removed: &[&str], added: &[&str]) -> ChangedWords {
        ChangedWords {
            removed: removed.iter().map(|l| non_blank_span(l)).collect(),
            added: added.iter().map(|l| non_blank_span(l)).collect(),
            pairs: Vec::new(),
        }
    }
}

fn changed_words(removed: &[&str], added: &[&str], options: DiffOptions) -> ChangedWords {
    if options.words == WordMode::Off || removed.is_empty() || added.is_empty() {
        return ChangedWords::unmarked(removed, added);
    }
    if removed.len().max(added.len()) > MAX_WORD_DIFF_LINES {
        return ChangedWords::whole_lines(removed, added);
    }
    let whitespace = options.whitespace;
    if options.words == WordMode::Block {
        let (removed, added) = word_spans(removed, added, whitespace);
        return ChangedWords {
            removed,
            added,
            pairs: Vec::new(),
        };
    }
    let pairs = if options.words == WordMode::Similar {
        pair_similar(removed, added, whitespace)
    } else {
        (0..removed.len().min(added.len()))
            .map(|i| (i, i))
            .collect()
    };
    let mut words = ChangedWords {
        pairs,
        ..ChangedWords::whole_lines(removed, added)
    };
    for &(i, j) in &words.pairs {
        let (r, a) = word_spans(&removed[i..=i], &added[j..=j], whitespace);
        words.removed[i] = r.into_iter().next().unwrap_or_default();
        words.added[j] = a.into_iter().next().unwrap_or_default();
    }
    words
}

fn words(line: &str) -> Vec<&str> {
    #[derive(Clone, Copy, PartialEq)]
    enum Class {
        Word,
        Blank,
        Other,
    }
    let class_of = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c == ' ' || c == '\t' {
            Class::Blank
        } else {
            Class::Other
        }
    };
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev = None;
    for (i, c) in line.char_indices() {
        let class = class_of(c);
        let extends_run = prev == Some(class) && class != Class::Other;
        if !extends_run && i > start {
            out.push(&line[start..i]);
            start = i;
        }
        prev = Some(class);
    }
    if start < line.len() {
        out.push(&line[start..]);
    }
    out
}

/// Blank words are left out of the comparison when whitespace doesn't count.
fn word_spans(
    before: &[&str],
    after: &[&str],
    whitespace: Whitespace,
) -> (Vec<LineSpans>, Vec<LineSpans>) {
    let before_words = located_words(before, whitespace);
    let after_words = located_words(after, whitespace);
    let mut input = InternedInput::default();
    input.update_before(before_words.iter().map(|w| w.text));
    input.update_after(after_words.iter().map(|w| w.text));
    let diff = Diff::compute(Algorithm::Histogram, &input);
    (
        changed_spans(before.len(), &before_words, |i| diff.is_removed(i)),
        changed_spans(after.len(), &after_words, |i| diff.is_added(i)),
    )
}

struct LocatedWord<'a> {
    text: &'a str,
    line: usize,
    range: Range<usize>,
}

fn located_words<'a>(lines: &[&'a str], whitespace: Whitespace) -> Vec<LocatedWord<'a>> {
    let mut located = Vec::new();
    for (line, text) in lines.iter().enumerate() {
        let mut offset = 0;
        for word in words(text) {
            if whitespace == Whitespace::Compare || !word.trim().is_empty() {
                located.push(LocatedWord {
                    text: word,
                    line,
                    range: offset..offset + word.len(),
                });
            }
            offset += word.len();
        }
    }
    located
}

/// Line endings are never marked, and adjacent changed words merge into one span.
fn changed_spans(
    line_count: usize,
    words: &[LocatedWord],
    is_changed: impl Fn(u32) -> bool,
) -> Vec<LineSpans> {
    let mut spans: Vec<LineSpans> = vec![Vec::new(); line_count];
    for (i, word) in (0..).zip(words) {
        if !is_changed(i) || word.text.trim_end_matches(['\n', '\r']).is_empty() {
            continue;
        }
        let line = &mut spans[word.line];
        match line.last_mut() {
            Some(last) if last.end == word.range.start => last.end = word.range.end,
            _ => line.push(word.range.clone()),
        }
    }
    spans
}

/// Greedy and in order, so that paired lines keep their order side by side.
fn pair_similar(removed: &[&str], added: &[&str], whitespace: Whitespace) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    let mut first_free = 0;
    for (i, r) in removed.iter().enumerate() {
        let mut best: Option<(f32, usize)> = None;
        for (j, a) in added
            .iter()
            .enumerate()
            .skip(first_free)
            .take(PAIRING_LOOKAHEAD)
        {
            let score = similarity(r, a, whitespace);
            if score >= MIN_PAIRING_SIMILARITY && best.is_none_or(|(top, _)| score > top) {
                best = Some((score, j));
            }
        }
        if let Some((_, j)) = best {
            pairs.push((i, j));
            first_free = j + 1;
        }
    }
    pairs
}

/// The share of the two lines' non-blank characters that their word comparison keeps, from 0
/// to 1.
fn similarity(a: &str, b: &str, whitespace: Whitespace) -> f32 {
    let (a_spans, b_spans) = word_spans(&[a], &[b], whitespace);
    let non_blank = |s: &str| s.chars().filter(|c| !c.is_whitespace()).count();
    let kept_and_total = |line: &str, changed: &[Range<usize>]| {
        let total = non_blank(line);
        let changed: usize = changed.iter().map(|r| non_blank(&line[r.clone()])).sum();
        (total.saturating_sub(changed), total)
    };
    let (a_kept, a_total) = kept_and_total(a, &a_spans[0]);
    let (b_kept, b_total) = kept_and_total(b, &b_spans[0]);
    if a_total + b_total == 0 {
        1.0
    } else {
        (a_kept + b_kept) as f32 / (a_total + b_total) as f32
    }
}

fn non_blank_span(raw: &str) -> LineSpans {
    let body = raw.trim_end();
    let start = body.len() - body.trim_start().len();
    (start < body.len())
        .then_some(start..body.len())
        .into_iter()
        .collect()
}

fn without_ending(raw: &str) -> &str {
    let body = raw.strip_suffix('\n').unwrap_or(raw);
    body.strip_suffix('\r').unwrap_or(body)
}

fn next_column(col: usize, c: char) -> usize {
    if c == '\t' {
        col + TAB_WIDTH - col % TAB_WIDTH
    } else {
        col + 1
    }
}

/// The byte offset in `raw` of display column `col`. A column inside a tab's spaces counts as
/// after the tab.
pub fn raw_offset(raw: &str, col: usize) -> usize {
    let mut at = 0;
    for (i, c) in raw.char_indices() {
        if at >= col {
            return i;
        }
        at = next_column(at, c);
        if at > col {
            return i + c.len_utf8();
        }
    }
    raw.len()
}

/// The display column of byte `offset` of `raw`: the inverse of [`raw_offset`].
pub fn display_column(raw: &str, offset: usize) -> usize {
    raw.char_indices()
        .take_while(|&(i, _)| i < offset)
        .fold(0, |col, (_, c)| next_column(col, c))
}

/// `raw` as shown, line ending dropped and tabs expanded, with `spans` moved along.
pub fn display(raw: &str, spans: &[Range<usize>]) -> (String, LineSpans) {
    let (text, map) = display_map(raw);
    let last = map.len() - 1;
    let spans = spans
        .iter()
        .filter_map(|r| {
            let start = map[r.start.min(last)];
            let end = map[r.end.min(last)];
            (end > start).then_some(start..end)
        })
        .collect();
    (text, spans)
}

/// `raw` as shown, and for each byte of `raw` (its ending dropped), plus its end, the byte of
/// the shown text it lands on.
pub fn display_map(raw: &str) -> (String, Vec<usize>) {
    let body = without_ending(raw);
    let mut text = String::with_capacity(body.len());
    let mut map = Vec::with_capacity(body.len() + 1);
    let mut col = 0;
    for c in body.chars() {
        map.extend(std::iter::repeat_n(text.len(), c.len_utf8()));
        let next = next_column(col, c);
        if c == '\t' {
            text.extend(std::iter::repeat_n(' ', next - col));
        } else {
            text.push(c);
        }
        col = next;
    }
    map.push(text.len());
    (text, map)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    Row(usize),
    Fold(Range<usize>),
}

/// Folds unchanged stretches beyond [`CONTEXT_LINES`] of a change. `open` holds lines of the
/// new version opened by hand: lines, not rows, so that they stay open in the other form and
/// under other options. A single row never folds: its fold would take as much room.
pub fn fold(rows: &[Row], diff: &FileDiff, open: &[Range<u32>]) -> Vec<Shown> {
    let is_open = |row: Row| row.new.is_some_and(|n| open.iter().any(|r| r.contains(&n)));
    let mut shown = Vec::new();
    let push_hidden = |shown: &mut Vec<Shown>, run: Range<usize>| {
        if run.len() < 2 {
            shown.extend(run.map(Shown::Row));
        } else {
            shown.push(Shown::Fold(run));
        }
    };
    let mut i = 0;
    while i < rows.len() {
        if diff.is_changed(rows[i]) {
            shown.push(Shown::Row(i));
            i += 1;
            continue;
        }
        let start = i;
        while i < rows.len() && !diff.is_changed(rows[i]) {
            i += 1;
        }
        let keep_before = if start > 0 { CONTEXT_LINES } else { 0 };
        let keep_after = if i < rows.len() { CONTEXT_LINES } else { 0 };
        // Clamped: the stretch can be shorter than the context on both sides.
        let from = (start + keep_before).min(i);
        let hidden = from..i.saturating_sub(keep_after).max(from);
        shown.extend((start..hidden.start).map(Shown::Row));
        let mut run = hidden.start;
        for r in hidden.clone() {
            if is_open(rows[r]) {
                push_hidden(&mut shown, run..r);
                shown.push(Shown::Row(r));
                run = r + 1;
            }
        }
        push_hidden(&mut shown, run..hidden.end);
        shown.extend((hidden.end..i).map(Shown::Row));
    }
    shown
}

/// The new version's lines of a fold, for [`fold`]'s `open`.
pub fn fold_lines(rows: &[Row], hidden: &Range<usize>) -> Option<Range<u32>> {
    let first = rows.get(hidden.start)?.new?;
    let last = rows.get(hidden.end.checked_sub(1)?)?.new?;
    Some(first..last + 1)
}

/// `columns` are display columns of the line's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub row: usize,
    pub old: bool,
    pub columns: Range<usize>,
}

impl crate::find::Place for Found {
    fn line(&self) -> usize {
        self.row
    }
}

/// Finds in the lines as in the file, in reading order. Side by side, an unchanged line counts
/// once, as its new version, unless its versions differ in whitespace that doesn't count.
pub fn find(diff: &FileDiff, rows: &[Row], side_by_side: bool, query: &str) -> Vec<Found> {
    let mut lines = Vec::new();
    for (r, &row) in rows.iter().enumerate() {
        let old = diff.line(row, Some(true));
        let new = diff.line(row, Some(false));
        if side_by_side {
            if let Some(o) = old
                && !(o.kind == LineKind::Same && new.is_some_and(|n| n.raw == o.raw))
            {
                lines.push((r, true, o));
            }
            if let Some(n) = new {
                lines.push((r, false, n));
            }
        } else if let Some(line) = new.or(old) {
            lines.push((r, new.is_none(), line));
        }
    }
    crate::find::find(lines.iter().map(|(_, _, l)| &l.raw), query)
        .into_iter()
        .map(|m| {
            let (row, old, line) = lines[m.line];
            let columns =
                display_column(&line.raw, m.range.start)..display_column(&line.raw, m.range.end);
            Found { row, old, columns }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(words: WordMode, whitespace: Whitespace) -> DiffOptions {
        DiffOptions { words, whitespace }
    }

    fn side(d: &FileDiff) -> Vec<(String, String)> {
        let text = |lines: &[DiffLine], i: Option<u32>| {
            i.map_or(String::new(), |i| lines[i as usize].text.clone())
        };
        d.side
            .iter()
            .map(|r| (text(&d.old, r.old), text(&d.new, r.new)))
            .collect()
    }

    /// The changed words of every changed line: `-text` or `+text`, spans in brackets.
    fn marked(d: &FileDiff) -> Vec<String> {
        let mark = |l: &DiffLine, sign: char| {
            let mut s = String::from(sign);
            let mut at = 0;
            for r in &l.spans {
                s.push_str(&l.text[at..r.start]);
                s.push('[');
                s.push_str(&l.text[r.clone()]);
                s.push(']');
                at = r.end;
            }
            s.push_str(&l.text[at..]);
            s
        };
        d.old
            .iter()
            .filter(|l| l.kind == LineKind::Removed)
            .map(|l| mark(l, '-'))
            .chain(
                d.new
                    .iter()
                    .filter(|l| l.kind == LineKind::Added)
                    .map(|l| mark(l, '+')),
            )
            .collect()
    }

    #[test]
    fn identical_texts_have_no_changes() {
        let d = FileDiff::new("a\nb\n", "a\nb\n", DiffOptions::default());
        assert_eq!((d.added, d.removed), (0, 0));
        assert!(d.side_change_starts.is_empty());
        assert_eq!(d.side.len(), 2);
        assert_eq!(d.unified.len(), 2);
    }

    #[test]
    fn an_edited_line_shows_its_changed_word() {
        let d = FileDiff::new(
            "let x = 1;\nkeep\n",
            "let x = 2;\nkeep\n",
            DiffOptions::default(),
        );
        assert_eq!((d.added, d.removed), (1, 1));
        assert_eq!(marked(&d), ["-let x = [1];", "+let x = [2];"]);
        assert_eq!(d.side_change_starts, [0]);
        assert_eq!(d.unified_change_starts, [0]);
        assert_eq!(
            d.unified,
            [
                Row {
                    old: Some(0),
                    new: None
                },
                Row {
                    old: None,
                    new: Some(0)
                },
                Row {
                    old: Some(1),
                    new: Some(1)
                },
            ]
        );
    }

    #[test]
    fn a_line_inserted_in_an_edited_block_pairs_the_edits_by_similarity() {
        let old = "    total = price * qty;\n    tax = total * rate;\n";
        let new = "    total = price * quantity;\n    log(total);\n    tax = total * vat_rate;\n";
        let d = FileDiff::new(old, new, DiffOptions::default());
        assert_eq!(
            side(&d),
            [
                (
                    "    total = price * qty;".into(),
                    "    total = price * quantity;".into()
                ),
                (String::new(), "    log(total);".into()),
                (
                    "    tax = total * rate;".into(),
                    "    tax = total * vat_rate;".into()
                ),
            ]
        );
        assert_eq!(
            marked(&d),
            [
                "-    total = price * [qty];",
                "-    tax = total * [rate];",
                "+    total = price * [quantity];",
                "+    [log(total);]",
                "+    tax = total * [vat_rate];",
            ]
        );
    }

    #[test]
    fn by_position_pairs_the_nth_lines_whatever_they_hold() {
        let old = "    total = price * qty;\n    tax = total * rate;\n";
        let new = "    total = price * quantity;\n    log(total);\n    tax = total * vat_rate;\n";
        let d = FileDiff::new(old, new, opts(WordMode::Position, Whitespace::Compare));
        assert_eq!(side(&d)[1].0, "    tax = total * rate;");
        assert_eq!(side(&d)[1].1, "    log(total);");
        assert_eq!(marked(&d)[4], "+    [tax = total * vat_rate;]");
    }

    #[test]
    fn whole_block_compares_all_lines_of_a_change_as_one_text() {
        let d = FileDiff::new(
            "a b c\n",
            "a b\nc\n",
            opts(WordMode::Block, Whitespace::Compare),
        );
        // Only the break between b and c changed.
        assert!(marked(&d).iter().all(|l| !l.contains("[a")));
    }

    #[test]
    fn off_marks_no_words() {
        let d = FileDiff::new(
            "x = 1\n",
            "x = 2\n",
            opts(WordMode::Off, Whitespace::Compare),
        );
        assert_eq!(marked(&d), ["-x = 1", "+x = 2"]);
    }

    #[test]
    fn a_moved_function_is_a_removal_and_an_addition() {
        let f = "fn helper() {\n    work();\n}\n";
        let old = format!("{f}\nfn main() {{\n    helper();\n}}\n");
        let new = format!("fn main() {{\n    helper();\n}}\n\n{f}");
        let d = FileDiff::new(&old, &new, DiffOptions::default());
        assert_eq!(d.side_change_starts.len(), 2);
        assert_eq!(
            marked(&d),
            [
                "-fn helper() {",
                "-    work();",
                "-}",
                "-",
                "+",
                "+fn helper() {",
                "+    work();",
                "+}",
            ]
        );
        assert_eq!(side(&d)[4], ("fn main() {".into(), "fn main() {".into()));
    }

    #[test]
    fn whitespace_only_edits_depend_on_the_setting() {
        let old = "if x {\n\tgo(a,b);\n}\n";
        let new = "if x {\n    go(a,  b);  \n}\n";
        let compare = FileDiff::new(old, new, DiffOptions::default());
        assert_eq!((compare.added, compare.removed), (1, 1));
        let changes = FileDiff::new(old, new, opts(WordMode::Similar, Whitespace::IgnoreChanges));
        // A tab against four spaces is still "some blank" against "some blank", but `a,b`
        // against `a,  b` gains a blank where there was none.
        assert_eq!((changes.added, changes.removed), (1, 1));
        let all = FileDiff::new(old, new, opts(WordMode::Similar, Whitespace::IgnoreAll));
        assert_eq!((all.added, all.removed), (0, 0));
    }

    #[test]
    fn ignoring_whitespace_changes_ignores_crlf() {
        let crlf = "one\r\ntwo\r\n";
        let lf = "one\ntwo\n";
        let compare = FileDiff::new(crlf, lf, DiffOptions::default());
        assert_eq!((compare.added, compare.removed), (2, 2));
        assert_eq!(compare.old_endings, Some(Endings::Crlf));
        assert_eq!(compare.new_endings, Some(Endings::Lf));
        let ignore = FileDiff::new(crlf, lf, opts(WordMode::Similar, Whitespace::IgnoreChanges));
        assert_eq!((ignore.added, ignore.removed), (0, 0));
    }

    #[test]
    fn ignored_blanks_are_not_changed_words() {
        let d = FileDiff::new(
            "call(a,b) + x\n",
            "call(a, b) + y\n",
            opts(WordMode::Similar, Whitespace::IgnoreChanges),
        );
        assert_eq!(marked(&d), ["-call(a,b) + [x]", "+call(a, b) + [y]"]);
    }

    #[test]
    fn tabs_expand_and_spans_follow() {
        let (text, spans) = display("\tab\tc\r\n", &[3..4, 4..5]);
        assert_eq!(text, "    ab  c");
        // The second tab fills columns 6 and 7; `c` moves to column 8.
        assert_eq!(spans, [6..8, 8..9]);
        let (text, _) = display("é\tx", &[]);
        assert_eq!(text, "é   x");
    }

    #[test]
    fn display_columns_map_back_to_the_raw_line() {
        let d = FileDiff::new("", "\tab\tcé\r\n", DiffOptions::default());
        let l = &d.new[0];
        assert_eq!(l.text, "    ab  cé");
        assert_eq!(l.raw, "\tab\tcé");
        // Columns 0..4 are the first tab: 0 is before it, 1 to 3 inside it (after it).
        assert_eq!(l.raw_offset(0), 0);
        assert_eq!(l.raw_offset(2), 1);
        assert_eq!(l.raw_offset(4), 1);
        assert_eq!(l.raw_offset(5), 2);
        assert_eq!(l.raw_offset(8), 4);
        assert_eq!(&l.raw[l.raw_offset(4)..l.raw_offset(9)], "ab\tc");
        assert_eq!(l.raw_offset(10), l.raw.len());
        assert_eq!(l.raw_offset(99), l.raw.len());
        assert_eq!(display_column(&l.raw, 0), 0);
        assert_eq!(display_column(&l.raw, 1), 4);
        assert_eq!(display_column(&l.raw, 3), 6);
        assert_eq!(display_column(&l.raw, 4), 8);
        assert_eq!(display_column(&l.raw, l.raw.len()), 10);
    }

    #[test]
    fn invalid_utf8_becomes_escapes() {
        let (s, bad) = decode(b"Caf\xe9 ok\xff");
        assert_eq!(s, "Caf\\xE9 ok\\xFF");
        assert_eq!(bad, 2);
        assert_eq!(decode("Café".as_bytes()), ("Café".into(), 0));
    }

    #[test]
    fn endings_of_a_last_line_without_newline_do_not_make_a_file_mixed() {
        assert_eq!(Endings::of(&["a\r\n", "b"]), Some(Endings::Crlf));
        assert_eq!(Endings::of(&["a\r\n", "b\n"]), Some(Endings::Mixed));
        assert_eq!(Endings::of(&[]), None);
    }

    fn numbered(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}\n")).collect()
    }

    #[test]
    fn unchanged_stretches_fold_beyond_the_context() {
        let old = numbered(30);
        let new = old.replace("line 15\n", "line fifteen\n");
        let d = FileDiff::new(&old, &new, DiffOptions::default());
        let shown = fold(&d.side, &d, &[]);
        // Rows 0..11 fold, 11..14 are context, 14 is the change, 15..18 context, 18..30 fold.
        assert_eq!(shown[0], Shown::Fold(0..11));
        assert_eq!(
            shown[1..4],
            [Shown::Row(11), Shown::Row(12), Shown::Row(13)]
        );
        assert_eq!(shown[4], Shown::Row(14));
        assert_eq!(shown[8], Shown::Fold(18..30));
        assert_eq!(shown.len(), 9);

        let lines = fold_lines(&d.side, &(18..30)).unwrap();
        assert_eq!(lines, 18..30);
        let open = fold(&d.side, &d, &[lines]);
        assert_eq!(open.len(), 8 + 12);
        assert!(!open.contains(&Shown::Fold(18..30)));
    }

    #[test]
    fn short_stretches_do_not_fold() {
        let old = numbered(9);
        let new = old
            .replace("line 1\n", "line one\n")
            .replace("line 9\n", "line nine\n");
        let d = FileDiff::new(&old, &new, DiffOptions::default());
        // Seven unchanged rows between the changes: three and three kept, one left to fold,
        // which is not worth a fold.
        let shown = fold(&d.side, &d, &[]);
        assert!(shown.iter().all(|s| matches!(s, Shown::Row(_))));
    }

    #[test]
    fn an_unchanged_file_folds_into_one() {
        let text = numbered(10);
        let d = FileDiff::new(&text, &text, DiffOptions::default());
        assert_eq!(fold(&d.side, &d, &[]), [Shown::Fold(0..10)]);
    }

    #[test]
    fn every_row_is_shown_once_between_close_changes() {
        // One unchanged line between two changes, fewer than the context kept around them.
        let old = numbered(12);
        let new = old
            .replace("line 5\n", "line five\n")
            .replace("line 7\n", "line seven\n");
        let d = FileDiff::new(&old, &new, DiffOptions::default());
        for rows in [&d.side, &d.unified] {
            let shown = fold(rows, &d, &[]);
            let mut covered: Vec<usize> = shown
                .iter()
                .flat_map(|s| match s {
                    Shown::Row(r) => *r..*r + 1,
                    Shown::Fold(range) => range.clone(),
                })
                .collect();
            let in_order = covered.windows(2).all(|w| w[0] < w[1]);
            covered.dedup();
            assert!(in_order, "{shown:?}");
            assert_eq!(covered, (0..rows.len()).collect::<Vec<_>>());
        }
    }

    #[test]
    fn lines_opened_inside_a_fold_split_it() {
        let old = numbered(30);
        let new = old.replace("line 15\n", "line fifteen\n");
        let d = FileDiff::new(&old, &new, DiffOptions::default());
        let shown = fold(&d.side, &d, std::slice::from_ref(&(20..22)));
        let tail: Vec<_> = shown[8..].to_vec();
        assert_eq!(
            tail,
            [
                Shown::Fold(18..20),
                Shown::Row(20),
                Shown::Row(21),
                Shown::Fold(22..30)
            ]
        );
    }

    #[test]
    fn opened_lines_stay_open_in_the_other_form_and_whitespace_setting() {
        let old = numbered(30).replace("line 15\n", "line  15\n");
        let new = numbered(30).replace("line 5\n", "line five\n");
        let open = std::slice::from_ref(&(10..20));
        for ws in [
            Whitespace::Compare,
            Whitespace::IgnoreChanges,
            Whitespace::IgnoreAll,
        ] {
            let d = FileDiff::new(&old, &new, opts(WordMode::Similar, ws));
            for rows in [&d.side, &d.unified] {
                let shown = fold(rows, &d, open);
                let row_of_line = |n: u32| rows.iter().position(|r| r.new == Some(n)).unwrap();
                for n in 10..20 {
                    assert!(
                        shown.contains(&Shown::Row(row_of_line(n))),
                        "{ws:?}: line {n}"
                    );
                }
            }
        }
    }

    #[test]
    fn an_added_file_has_only_new_lines() {
        let d = FileDiff::new("", "a\nb\n", DiffOptions::default());
        assert_eq!((d.added, d.removed), (2, 0));
        assert!(d.old.is_empty());
        assert_eq!(side(&d), [("".into(), "a".into()), ("".into(), "b".into())]);
        // A pure addition marks no words: the tint says it all.
        assert_eq!(marked(&d), ["+a", "+b"]);
    }

    fn spec(old: bool, new: bool, modes: [u32; 2]) -> FileDiffSpec {
        let rev = Rev::Commit(Oid::from_hex("0123456789012345678901234567890123456789").unwrap());
        let v = |p: &str| Version {
            rev,
            path: p.into(),
        };
        FileDiffSpec {
            old: old.then(|| v("a")),
            new: new.then(|| v("a")),
            status: FileStatus::Modified,
            modes,
            binary: false,
        }
    }

    #[test]
    fn notes_name_what_the_lines_do_not_show() {
        let loaded = LoadedDiff {
            spec: spec(true, true, [0o100644, 0o100755]),
            content: Content::Text {
                old: "a\r\n".into(),
                new: "a\n".into(),
                invalid_bytes: 1,
            },
            textconv: Some("table: column -t".into()),
        };
        let d = FileDiff::new(
            "a\r\n",
            "a\n",
            opts(WordMode::Similar, Whitespace::IgnoreChanges),
        );
        let notes = d.notes(&loaded, Whitespace::IgnoreChanges);
        assert_eq!(
            notes,
            [
                Note::Textconv("table: column -t".into()),
                Note::LineEndings {
                    old: Endings::Crlf,
                    new: Endings::Lf,
                    ignored: true
                },
                Note::InvalidUtf8(1),
                Note::OnlyWhitespace,
                Note::Mode {
                    old: 0o100644,
                    new: 0o100755
                },
            ]
        );
        assert_eq!(notes[1].to_string(), "Line endings: CRLF → LF (ignored)");
        assert_eq!(notes[4].to_string(), "Mode 100644 → 100755");
    }

    #[test]
    fn an_added_file_gets_no_unchanged_note() {
        let loaded = LoadedDiff {
            spec: spec(false, true, [0, 0o100644]),
            content: Content::Text {
                old: String::new(),
                new: String::new(),
                invalid_bytes: 0,
            },
            textconv: None,
        };
        let d = FileDiff::new("", "", DiffOptions::default());
        assert!(d.notes(&loaded, Whitespace::Compare).is_empty());
    }

    #[test]
    fn a_spec_from_a_commit_takes_the_old_path_of_a_rename() {
        let c = Oid::from_hex("1111111111111111111111111111111111111111").unwrap();
        let p = Oid::from_hex("2222222222222222222222222222222222222222").unwrap();
        let file = ChangedFile {
            path: "new.rs".into(),
            old_path: Some("old.rs".into()),
            status: FileStatus::Renamed,
            modes: [0o100644; 2],
            added: Some(1),
            removed: Some(1),
        };
        let s = FileDiffSpec::of_commit(c, Some(p), &file);
        assert_eq!(
            s.old.as_ref().map(|v| (v.rev, v.path.as_str())),
            Some((Rev::Commit(p), "old.rs"))
        );
        assert_eq!(
            s.new.as_ref().map(|v| (v.rev, v.path.as_str())),
            Some((Rev::Commit(c), "new.rs"))
        );

        let added = ChangedFile {
            status: FileStatus::Added,
            old_path: None,
            ..file.clone()
        };
        assert!(FileDiffSpec::of_commit(c, Some(p), &added).old.is_none());
        let deleted = ChangedFile {
            status: FileStatus::Deleted,
            old_path: None,
            ..file.clone()
        };
        assert!(FileDiffSpec::of_commit(c, Some(p), &deleted).new.is_none());
        assert!(FileDiffSpec::of_commit(c, None, &file).old.is_none());
    }

    fn places(d: &FileDiff, side_by_side: bool, query: &str) -> Vec<(usize, char, String)> {
        let rows = if side_by_side { &d.side } else { &d.unified };
        find(d, rows, side_by_side, query)
            .into_iter()
            .map(|f| {
                let line = d.line(rows[f.row], Some(f.old)).unwrap();
                let text = line.text.chars().collect::<Vec<_>>()[f.columns]
                    .iter()
                    .collect();
                (f.row, if f.old { '-' } else { '+' }, text)
            })
            .collect()
    }

    #[test]
    fn finds_in_the_lines_a_form_shows_in_reading_order() {
        let old = "same foo\nwas foo\n\tFoo\n";
        let new = "same foo\nnow FOO foo\n\tFoo\n";
        let d = FileDiff::new(old, new, opts(WordMode::Position, Whitespace::Compare));
        assert_eq!(
            places(&d, true, "foo"),
            [
                (0, '+', "foo".into()),
                (1, '-', "foo".into()),
                (1, '+', "FOO".into()),
                (1, '+', "foo".into()),
                (2, '+', "Foo".into()),
            ]
        );
        assert_eq!(find(&d, &d.side, true, "foo")[4].columns, 4..7);
        assert_eq!(
            places(&d, false, "foo"),
            [
                (0, '+', "foo".into()),
                (1, '-', "foo".into()),
                (2, '+', "FOO".into()),
                (2, '+', "foo".into()),
                (3, '+', "Foo".into()),
            ]
        );
        // Found as in the file: a tab is a tab.
        assert_eq!(places(&d, true, "\tf"), [(2, '+', "    F".into())]);
        assert!(find(&d, &d.side, true, "").is_empty());

        let found = find(&d, &d.side, true, "foo");
        assert_eq!(crate::find::first_from(&found, 2), Some(4));
        assert_eq!(crate::find::step(&found, None, false, 1), Some(0));
    }

    #[test]
    fn an_unchanged_line_whose_versions_differ_counts_twice_side_by_side() {
        // Unchanged only because whitespace doesn't count.
        let d = FileDiff::new(
            "a  foo\n",
            "a foo\n",
            opts(WordMode::Similar, Whitespace::IgnoreAll),
        );
        assert_eq!(
            places(&d, true, "foo"),
            [(0, '-', "foo".into()), (0, '+', "foo".into())]
        );
        assert_eq!(places(&d, false, "foo"), [(0, '+', "foo".into())]);
    }
}
