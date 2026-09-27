//! File diffs: what changed in one file between two versions of it.
//!
//! A diff compares two [`Version`]s, each a path at a revision; either can be missing (an added
//! or a deleted file). Nothing here assumes "a commit and its parent", so comparing any two
//! revisions reuses it. Git reads the versions ([`crate::git::Git::load_file_diff`], through
//! the user's textconv filters) and decides what is binary; lines and words are compared here
//! with `imara-diff` (histogram, then git's indent heuristic). Decided in #44, #45 and #46.
//!
//! [`FileDiff::new`] turns two texts into lines with their changed words, and into rows for the
//! two forms the diff window draws: side by side and unified. [`fold`] collapses the unchanged
//! stretches between changes.

use std::fmt::Write as _;
use std::ops::Range;

use imara_diff::{Algorithm, Diff, InternedInput};
use serde::{Deserialize, Serialize};

use crate::changed_files::{ChangedFile, FileStatus};
use crate::oid::Oid;

/// The mode git gives a submodule's entry.
pub const GITLINK_MODE: u32 = 0o160000;
/// Tabs are expanded to multiples of this many columns.
pub const TAB_WIDTH: usize = 4;
/// Unchanged lines kept around a change when the rest is folded away, as git's default
/// `diff.context`.
pub const CONTEXT_LINES: usize = 3;
/// A removed line is paired with the most similar of this many added lines after the last
/// pair ([`WordMode::Similar`]).
const PAIRING_REACH: usize = 12;
/// Share of their non-blank text two lines must have in common to be paired.
const PAIRING_SIMILARITY: f32 = 0.5;
/// Changes with more lines than this on a side get no word comparison: their lines are marked
/// whole. Keeps pathological changes from stalling the window.
const MAX_WORD_DIFF_LINES: usize = 5_000;

/// Where a version of a file is read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rev {
    Commit(Oid),
    /// The files on disk, staged or not, as `git diff` reads them: through the file's clean
    /// filter and line-ending conversion.
    WorkingTree,
}

impl Rev {
    /// The commit, unless this is the working tree.
    pub fn commit(self) -> Option<Oid> {
        match self {
            Rev::Commit(oid) => Some(oid),
            Rev::WorkingTree => None,
        }
    }
}

/// One version of a file: a path at a revision.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    pub rev: Rev,
    pub path: String,
}

/// What to compare: the old and new version of a file, and what git said about the change.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileDiffSpec {
    /// `None` for an added file.
    pub old: Option<Version>,
    /// `None` for a deleted file.
    pub new: Option<Version>,
    pub status: FileStatus,
    /// Old and new mode (0 where a side is missing).
    pub modes: [u32; 2],
    /// Git treats the file as binary.
    pub binary: bool,
}

impl FileDiffSpec {
    /// The change a commit made to one of its changed files, against `parent` (`None` for a
    /// root commit). Renamed and copied files take their old path on the old side.
    pub fn of_commit(commit: Oid, parent: Option<Oid>, file: &ChangedFile) -> FileDiffSpec {
        FileDiffSpec::between(parent.map(Rev::Commit), Rev::Commit(commit), file)
    }

    /// A changed file of a comparison of `old` (`None` for nothing, as for a root commit) with
    /// `new`. Renamed and copied files take their old path on the old side.
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

    /// True if a side is read from the working tree, and so can change after loading.
    pub fn reads_working_tree(&self) -> bool {
        [&self.old, &self.new]
            .into_iter()
            .flatten()
            .any(|v| v.rev == Rev::WorkingTree)
    }

    /// True if either side is a submodule.
    pub fn is_submodule(&self) -> bool {
        self.modes.contains(&GITLINK_MODE)
    }

    /// The path the diff is about: the new one, or the old one of a deleted file.
    pub fn path(&self) -> &str {
        self.new
            .as_ref()
            .or(self.old.as_ref())
            .map_or("", |v| v.path.as_str())
    }
}

/// What git gave for the two versions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    /// Both versions as text (a missing side is empty).
    Text {
        old: String,
        new: String,
        /// Bytes that were not valid UTF-8, now shown as `\xNN`.
        invalid_bytes: usize,
    },
    /// A binary file: only its sizes, where the side exists.
    Binary {
        old_size: Option<u64>,
        new_size: Option<u64>,
    },
    /// A submodule: the commit it pointed at on each side.
    Submodule { old: Option<Oid>, new: Option<Oid> },
}

/// A file diff's input, as loaded through git.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedDiff {
    pub spec: FileDiffSpec,
    pub content: Content,
    /// The textconv driver and its command (`name: command`), when one converted the text.
    pub textconv: Option<String>,
}

/// UTF-8, with every byte that isn't valid UTF-8 written as `\xNN`. Also returns how many
/// such bytes there were.
pub fn decode(bytes: &[u8]) -> (String, usize) {
    let mut out = String::with_capacity(bytes.len());
    let mut bad = 0;
    for chunk in bytes.utf8_chunks() {
        out.push_str(chunk.valid());
        for b in chunk.invalid() {
            let _ = write!(out, "\\x{b:02X}");
            bad += 1;
        }
    }
    (out, bad)
}

/// How changed words are found: which removed line is compared with which added line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WordMode {
    /// Each removed line with the most similar of the next added lines, if they share at
    /// least half their text. Paired lines share a row side by side; the other lines of a
    /// change are marked whole.
    #[default]
    Similar,
    /// The n-th removed line of a change with its n-th added line.
    Position,
    /// All removed lines of a change with all its added lines, as one text.
    Block,
    /// No changed words: only whole lines are marked.
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

    pub fn description(self) -> &'static str {
        match self {
            WordMode::Similar => {
                "Changed words: pair each removed line with the most similar added line"
            }
            WordMode::Position => {
                "Changed words: pair the removed and added lines of a change in order"
            }
            WordMode::Block => {
                "Changed words: compare a change's removed and added lines as one text"
            }
            WordMode::Off => "No changed words: mark whole lines only",
        }
    }
}

/// What counts as a change in whitespace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Whitespace {
    /// Every space, tab and line ending counts.
    #[default]
    Compare,
    /// Like `git diff -b`: more or fewer blanks count as the same, and blanks at the end of a
    /// line (a CR of a CRLF line ending included) don't count.
    IgnoreChanges,
    /// Like `git diff -w`: whitespace doesn't count at all.
    IgnoreAll,
}

impl Whitespace {
    pub const ALL: [Whitespace; 3] = [
        Whitespace::Compare,
        Whitespace::IgnoreChanges,
        Whitespace::IgnoreAll,
    ];

    pub fn description(self) -> &'static str {
        match self {
            Whitespace::Compare => "Compare whitespace: every space, tab and line ending counts",
            Whitespace::IgnoreChanges => {
                "Ignore whitespace changes (git diff -b): more or fewer blanks, and line endings, don't count"
            }
            Whitespace::IgnoreAll => "Ignore all whitespace (git diff -w)",
        }
    }

    /// The key a line is compared by.
    fn key(self, raw: &str) -> String {
        match self {
            Whitespace::Compare => raw.to_owned(),
            Whitespace::IgnoreChanges => {
                let mut out = String::with_capacity(raw.len());
                let mut gap = false;
                for c in raw.trim_end().chars() {
                    if c.is_whitespace() {
                        gap = true;
                    } else {
                        if gap {
                            out.push(' ');
                        }
                        gap = false;
                        out.push(c);
                    }
                }
                out
            }
            Whitespace::IgnoreAll => raw.chars().filter(|c| !c.is_whitespace()).collect(),
        }
    }
}

/// How a diff is computed.
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

/// A line of one version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    /// Line number in its version, from 1.
    pub no: u32,
    pub kind: LineKind,
    /// The text to show: no line ending, tabs expanded.
    pub text: String,
    /// The line as it is in the file, without its ending: what copying gives.
    pub raw: String,
    /// Changed words, as byte ranges of `text`, in order.
    pub spans: Vec<Range<usize>>,
}

/// A row of a form: indexes into [`FileDiff::old`] and [`FileDiff::new`]. Side by side, a
/// missing side is a filler; unified, a row has one side unless the line is the same in both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Row {
    pub old: Option<u32>,
    pub new: Option<u32>,
}

/// Line endings of a version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Endings {
    Lf,
    Crlf,
    Mixed,
}

impl Endings {
    pub fn name(self) -> &'static str {
        match self {
            Endings::Lf => "LF",
            Endings::Crlf => "CRLF",
            Endings::Mixed => "mixed",
        }
    }

    /// From the number of lines and of those ending in CRLF. A last line without an ending
    /// doesn't make a file mixed. `None` for an empty text.
    fn of(lines: &[&str]) -> Option<Endings> {
        let ended: Vec<&&str> = lines.iter().filter(|l| l.ends_with('\n')).collect();
        if ended.is_empty() {
            return None;
        }
        let crlf = ended.iter().filter(|l| l.ends_with("\r\n")).count();
        Some(if crlf == 0 {
            Endings::Lf
        } else if crlf == ended.len() {
            Endings::Crlf
        } else {
            Endings::Mixed
        })
    }
}

/// Something the diff window says about a diff besides its lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// Shown through a textconv filter (`name: command`).
    Textconv(String),
    LineEndings {
        old: Endings,
        new: Endings,
        /// The whitespace setting ignores the difference.
        ignored: bool,
    },
    Mode {
        old: u32,
        new: u32,
    },
    /// Bytes that are not valid UTF-8, shown as `\xNN`.
    InvalidUtf8(usize),
    /// Both versions are the same text (a pure rename or mode change).
    Unchanged,
    /// The versions differ in whitespace only, which the setting ignores.
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

/// The lines of both versions, their changes, and the rows of both forms.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub old: Vec<DiffLine>,
    pub new: Vec<DiffLine>,
    /// Side by side: removed and added lines of a change share rows, paired lines ([`WordMode::Similar`])
    /// on the same row.
    pub side: Vec<Row>,
    /// Unified, in git's order: a change's removed lines, then its added lines.
    pub unified: Vec<Row>,
    /// The first row of each change, in [`FileDiff::side`] and in [`FileDiff::unified`].
    pub side_changes: Vec<usize>,
    pub unified_changes: Vec<usize>,
    /// Lines added and removed as shown (after textconv and the whitespace setting).
    pub added: u32,
    pub removed: u32,
    pub old_endings: Option<Endings>,
    pub new_endings: Option<Endings>,
    /// The widest line, in characters (tabs expanded).
    pub widest: usize,
}

impl FileDiff {
    /// Compares `old` with `new`.
    pub fn new(old: &str, new: &str, options: DiffOptions) -> FileDiff {
        let ws = options.whitespace;
        let mode = options.words;
        let ol: Vec<&str> = imara_diff::sources::lines(old).collect();
        let nl: Vec<&str> = imara_diff::sources::lines(new).collect();
        let mut input: InternedInput<String> = InternedInput::default();
        input.update_before(ol.iter().map(|l| ws.key(l)));
        input.update_after(nl.iter().map(|l| ws.key(l)));
        let mut diff = Diff::compute(Algorithm::Histogram, &input);
        diff.postprocess_lines(&input);

        let mut d = FileDiff {
            added: diff.count_additions(),
            removed: diff.count_removals(),
            old_endings: Endings::of(&ol),
            new_endings: Endings::of(&nl),
            ..FileDiff::default()
        };
        let line = |raw: &str, i: usize, kind: LineKind, spans: &[Range<usize>]| {
            let (text, spans) = display(raw, spans);
            DiffLine {
                no: i as u32 + 1,
                kind,
                text,
                raw: without_ending(raw).to_owned(),
                spans,
            }
        };
        let (mut o, mut n) = (0usize, 0usize);
        let same = |d: &mut FileDiff, o: usize, n: usize| {
            d.old.push(line(ol[o], o, LineKind::Same, &[]));
            d.new.push(line(nl[n], n, LineKind::Same, &[]));
            let row = Row {
                old: Some(o as u32),
                new: Some(n as u32),
            };
            d.side.push(row);
            d.unified.push(row);
        };
        for h in diff.hunks() {
            while o < h.before.start as usize {
                same(&mut d, o, n);
                o += 1;
                n += 1;
            }
            let rem = &ol[h.before.start as usize..h.before.end as usize];
            let add = &nl[h.after.start as usize..h.after.end as usize];
            let (rs, as_, pairs) = changed_words(rem, add, options);
            d.side_changes.push(d.side.len());
            d.unified_changes.push(d.unified.len());
            for (i, r) in rem.iter().enumerate() {
                d.old.push(line(r, o + i, LineKind::Removed, &rs[i]));
            }
            for (j, a) in add.iter().enumerate() {
                d.new.push(line(a, n + j, LineKind::Added, &as_[j]));
            }
            let old_row = |i: usize| Some((o + i) as u32);
            let new_row = |j: usize| Some((n + j) as u32);
            d.unified.extend((0..rem.len()).map(|i| Row {
                old: old_row(i),
                new: None,
            }));
            d.unified.extend((0..add.len()).map(|j| Row {
                old: None,
                new: new_row(j),
            }));
            // Side by side: the lines between pairs share rows in order; each pair has a row.
            let stops = pairs
                .iter()
                .copied()
                .filter(|_| mode == WordMode::Similar)
                .chain([(rem.len(), add.len())]);
            let (mut i, mut j) = (0, 0);
            for (pi, pj) in stops {
                for k in 0..(pi - i).max(pj - j) {
                    d.side.push(Row {
                        old: (i + k < pi).then(|| old_row(i + k)).flatten(),
                        new: (j + k < pj).then(|| new_row(j + k)).flatten(),
                    });
                }
                if pi < rem.len() && pj < add.len() {
                    d.side.push(Row {
                        old: old_row(pi),
                        new: new_row(pj),
                    });
                }
                (i, j) = (pi + 1, pj + 1);
            }
            o = h.before.end as usize;
            n = h.after.end as usize;
        }
        while o < ol.len() && n < nl.len() {
            same(&mut d, o, n);
            o += 1;
            n += 1;
        }
        d.widest = d
            .old
            .iter()
            .chain(&d.new)
            .map(|l| l.text.chars().count())
            .max()
            .unwrap_or(0);
        d
    }

    /// The text of `row` as a form shows it: side by side, one side's line (`None` for a
    /// filler); unified, the line, which for an unchanged one is its new version.
    pub fn line(&self, row: Row, side: Option<bool>) -> Option<&DiffLine> {
        let old = row.old.map(|i| &self.old[i as usize]);
        let new = row.new.map(|i| &self.new[i as usize]);
        match side {
            Some(true) => old,
            Some(false) => new,
            None => new.or(old),
        }
    }

    /// True if a row shows a removed or an added line.
    pub fn is_changed(&self, row: Row) -> bool {
        let changed = |lines: &[DiffLine], i: Option<u32>| {
            i.is_some_and(|i| lines[i as usize].kind != LineKind::Same)
        };
        changed(&self.old, row.old) || changed(&self.new, row.new)
    }

    /// What to say about the diff besides its lines.
    pub fn notes(&self, loaded: &LoadedDiff, whitespace: Whitespace) -> Vec<Note> {
        let mut notes = Vec::new();
        if let Some(driver) = &loaded.textconv {
            notes.push(Note::Textconv(driver.clone()));
        }
        let both = loaded.spec.old.is_some() && loaded.spec.new.is_some();
        if let Content::Text {
            old,
            new,
            invalid_bytes,
        } = &loaded.content
        {
            if both
                && let (Some(a), Some(b)) = (self.old_endings, self.new_endings)
                && a != b
            {
                notes.push(Note::LineEndings {
                    old: a,
                    new: b,
                    ignored: whitespace != Whitespace::Compare,
                });
            }
            if *invalid_bytes > 0 {
                notes.push(Note::InvalidUtf8(*invalid_bytes));
            }
            if both && self.added == 0 && self.removed == 0 {
                notes.push(if old == new {
                    Note::Unchanged
                } else {
                    Note::OnlyWhitespace
                });
            }
        }
        let [a, b] = loaded.spec.modes;
        if both && a != b && a != 0 && b != 0 {
            notes.push(Note::Mode { old: a, new: b });
        }
        notes
    }
}

/// The changed words of a change's removed and added lines, and which lines were paired.
#[allow(clippy::type_complexity)]
fn changed_words(
    rem: &[&str],
    add: &[&str],
    options: DiffOptions,
) -> (
    Vec<Vec<Range<usize>>>,
    Vec<Vec<Range<usize>>>,
    Vec<(usize, usize)>,
) {
    let mut rs = vec![Vec::new(); rem.len()];
    let mut as_ = vec![Vec::new(); add.len()];
    let mixed = !rem.is_empty() && !add.is_empty();
    if !mixed || options.words == WordMode::Off {
        return (rs, as_, Vec::new());
    }
    let ws = options.whitespace;
    if rem.len().max(add.len()) > MAX_WORD_DIFF_LINES {
        return (
            rem.iter().map(|l| whole(l)).collect(),
            add.iter().map(|l| whole(l)).collect(),
            Vec::new(),
        );
    }
    let pairs: Vec<(usize, usize)> = match options.words {
        WordMode::Similar => pair_similar(rem, add, ws),
        WordMode::Position => (0..rem.len().min(add.len())).map(|i| (i, i)).collect(),
        WordMode::Block | WordMode::Off => Vec::new(),
    };
    if options.words == WordMode::Block {
        (rs, as_) = word_spans(rem, add, ws);
    } else {
        // Lines left unpaired in a change with both removals and additions are changed whole.
        rs = rem.iter().map(|l| whole(l)).collect();
        as_ = add.iter().map(|l| whole(l)).collect();
        for &(i, j) in &pairs {
            let (a, b) = word_spans(&rem[i..=i], &add[j..=j], ws);
            rs[i] = a.into_iter().next().unwrap_or_default();
            as_[j] = b.into_iter().next().unwrap_or_default();
        }
    }
    (rs, as_, pairs)
}

/// Words: runs of letters, digits and `_`; runs of spaces and tabs; any other character on its
/// own (a line ending too).
fn words(line: &str) -> Vec<&str> {
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c == ' ' || c == '\t' {
            1
        } else {
            2
        }
    };
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev: Option<u8> = None;
    for (i, c) in line.char_indices() {
        let k = class(c);
        if !(prev == Some(k) && k != 2) && i > start {
            out.push(&line[start..i]);
            start = i;
        }
        prev = Some(k);
    }
    if start < line.len() {
        out.push(&line[start..]);
    }
    out
}

/// Compares the words of `before` with those of `after` (each a list of raw lines, endings
/// included) and returns the changed byte ranges of every line. When whitespace is ignored,
/// blank words are left out of the comparison.
#[allow(clippy::type_complexity)]
fn word_spans(
    before: &[&str],
    after: &[&str],
    ws: Whitespace,
) -> (Vec<Vec<Range<usize>>>, Vec<Vec<Range<usize>>>) {
    type Located = (usize, Range<usize>);
    fn tokens<'a>(lines: &[&'a str], ws: Whitespace) -> (Vec<&'a str>, Vec<Located>) {
        let mut toks = Vec::new();
        let mut at = Vec::new();
        for (li, line) in lines.iter().enumerate() {
            let mut off = 0;
            for w in words(line) {
                if ws == Whitespace::Compare || !w.trim().is_empty() {
                    toks.push(w);
                    at.push((li, off..off + w.len()));
                }
                off += w.len();
            }
        }
        (toks, at)
    }
    let (bt, bat) = tokens(before, ws);
    let (at_, aat) = tokens(after, ws);
    let mut input: InternedInput<&str> = InternedInput::default();
    input.update_before(bt.iter().copied());
    input.update_after(at_.iter().copied());
    let diff = Diff::compute(Algorithm::Histogram, &input);
    let collect = |n: usize, at: &[Located], toks: &[&str], changed: &dyn Fn(u32) -> bool| {
        let mut spans: Vec<Vec<Range<usize>>> = vec![Vec::new(); n];
        for (i, (li, r)) in at.iter().enumerate() {
            if !changed(i as u32) || toks[i].trim_end_matches(['\n', '\r']).is_empty() {
                continue;
            }
            let v = &mut spans[*li];
            match v.last_mut() {
                Some(last) if last.end == r.start => last.end = r.end,
                _ => v.push(r.clone()),
            }
        }
        spans
    };
    (
        collect(before.len(), &bat, &bt, &|i| diff.is_removed(i)),
        collect(after.len(), &aat, &at_, &|i| diff.is_added(i)),
    )
}

/// Pairs removed lines with added lines, in order: each removed line takes the most similar of
/// the next [`PAIRING_REACH`] added lines after the last pair, if they share enough text.
fn pair_similar(rem: &[&str], add: &[&str], ws: Whitespace) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    let mut from = 0;
    for (i, r) in rem.iter().enumerate() {
        let mut best: Option<(f32, usize)> = None;
        for (j, a) in add.iter().enumerate().skip(from).take(PAIRING_REACH) {
            let sim = similarity(r, a, ws);
            if sim >= PAIRING_SIMILARITY && best.is_none_or(|(b, _)| sim > b) {
                best = Some((sim, j));
            }
        }
        if let Some((_, j)) = best {
            pairs.push((i, j));
            from = j + 1;
        }
    }
    pairs
}

/// The share of two lines' non-blank characters that their word comparison keeps, from 0 to 1.
fn similarity(a: &str, b: &str, ws: Whitespace) -> f32 {
    let (x, y) = word_spans(&[a], &[b], ws);
    let solid = |line: &str, spans: &[Range<usize>]| {
        let count = |s: &str| s.chars().filter(|c| !c.is_whitespace()).count();
        let total = count(line);
        let changed: usize = spans.iter().map(|s| count(&line[s.clone()])).sum();
        (total.saturating_sub(changed), total)
    };
    let (ka, ta) = solid(a, &x[0]);
    let (kb, tb) = solid(b, &y[0]);
    if ta + tb == 0 {
        1.0
    } else {
        (ka + kb) as f32 / (ta + tb) as f32
    }
}

/// A line's non-blank text as one span: the whole line is changed.
fn whole(raw: &str) -> Vec<Range<usize>> {
    let body = raw.trim_end();
    let start = body.len() - body.trim_start().len();
    if start < body.len() {
        std::iter::once(start..body.len()).collect()
    } else {
        Vec::new()
    }
}

/// A line without its ending (`\n` or `\r\n`).
fn without_ending(raw: &str) -> &str {
    let body = raw.strip_suffix('\n').unwrap_or(raw);
    body.strip_suffix('\r').unwrap_or(body)
}

impl DiffLine {
    /// Where display column `col` (a character of [`DiffLine::text`]) falls in
    /// [`DiffLine::raw`], as a byte offset ([`raw_offset`]).
    pub fn raw_offset(&self, col: usize) -> usize {
        raw_offset(&self.raw, col)
    }
}

/// Where display column `col` (a character of the line as [`display`] shows it) falls in the
/// `raw` line, as a byte offset. A column inside a tab's run of spaces counts as after the tab.
pub fn raw_offset(raw: &str, col: usize) -> usize {
    let mut at = 0;
    for (i, c) in raw.char_indices() {
        if at >= col {
            return i;
        }
        at += if c == '\t' {
            TAB_WIDTH - at % TAB_WIDTH
        } else {
            1
        };
        if at > col {
            return i + c.len_utf8();
        }
    }
    raw.len()
}

/// The display form of a raw line, and its spans moved along: the line ending dropped and tabs
/// expanded to multiples of [`TAB_WIDTH`].
pub fn display(raw: &str, spans: &[Range<usize>]) -> (String, Vec<Range<usize>>) {
    let body = without_ending(raw);
    let mut text = String::with_capacity(body.len());
    // Where each byte of `body` lands in `text`, plus the end.
    let mut map = Vec::with_capacity(body.len() + 1);
    let mut col = 0;
    for c in body.chars() {
        for _ in 0..c.len_utf8() {
            map.push(text.len());
        }
        if c == '\t' {
            let n = TAB_WIDTH - col % TAB_WIDTH;
            text.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            text.push(c);
            col += 1;
        }
    }
    map.push(text.len());
    let spans = spans
        .iter()
        .filter_map(|r| {
            let s = map[r.start.min(body.len())];
            let e = map[r.end.min(body.len())];
            (e > s).then_some(s..e)
        })
        .collect();
    (text, spans)
}

/// A row of a form as the window shows it: a line row, or a fold standing for hidden rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    Row(usize),
    /// Unchanged rows folded away; the fold is known by its first row.
    Fold(Range<usize>),
}

/// The rows to show when unchanged stretches are folded: [`CONTEXT_LINES`] rows are kept on
/// each side of a change, and rows whose line of the new version lies in `open` stay shown
/// (folds opened by hand; kept by line, so they outlast a change of form or options). What is
/// left folds, but runs of fewer than two rows never do, since the fold would take as much room.
pub fn fold(rows: &[Row], diff: &FileDiff, open: &[Range<u32>]) -> Vec<Shown> {
    let is_open = |row: Row| row.new.is_some_and(|n| open.iter().any(|r| r.contains(&n)));
    let mut shown = Vec::with_capacity(rows.len().min(4096));
    let hide = |shown: &mut Vec<Shown>, run: Range<usize>| {
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
        // Within the stretch, even when it is shorter than the context on both sides.
        let from = (start + keep_before).min(i);
        let hidden = from..i.saturating_sub(keep_after).max(from);
        shown.extend((start..hidden.start).map(Shown::Row));
        let mut run = hidden.start;
        for r in hidden.clone() {
            if is_open(rows[r]) {
                hide(&mut shown, run..r);
                shown.push(Shown::Row(r));
                run = r + 1;
            }
        }
        hide(&mut shown, run..hidden.end);
        shown.extend((hidden.end..i).map(Shown::Row));
    }
    shown
}

/// The lines of the new version a fold stands for, to open it with [`fold`].
pub fn fold_lines(rows: &[Row], hidden: &Range<usize>) -> Option<Range<u32>> {
    let first = rows.get(hidden.start)?.new?;
    let last = rows.get(hidden.end.checked_sub(1)?)?.new?;
    Some(first..last + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(words: WordMode, whitespace: Whitespace) -> DiffOptions {
        DiffOptions { words, whitespace }
    }

    /// The side-by-side rows as `(old text, new text)`, `""` for a filler.
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
        assert!(d.side_changes.is_empty());
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
        assert_eq!(d.side_changes, [0]);
        assert_eq!(d.unified_changes, [0]);
        // Unified: removed, then added, then the same line with both numbers.
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
        // Each edited line sits beside its new version; the inserted line gets a filler.
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
        // Rows by position: the second old line beside the inserted line.
        assert_eq!(side(&d)[1].0, "    tax = total * rate;");
        assert_eq!(side(&d)[1].1, "    log(total);");
        // The unpaired third new line is changed whole.
        assert_eq!(marked(&d)[4], "+    [tax = total * vat_rate;]");
    }

    #[test]
    fn whole_block_compares_all_lines_of_a_change_as_one_text() {
        let d = FileDiff::new(
            "a b c\n",
            "a b\nc\n",
            opts(WordMode::Block, Whitespace::Compare),
        );
        // Only the break between b and c changed; no word of the text did.
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
        // Histogram keeps `main` in place: the function is removed above it and added below,
        // each a change of its own with no words to compare.
        assert_eq!(d.side_changes.len(), 2);
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

        // An expanded fold shows its rows again.
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
        for ws in Whitespace::ALL {
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
        // A root commit has nothing to compare with.
        assert!(FileDiffSpec::of_commit(c, None, &file).old.is_none());
    }
}
