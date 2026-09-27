//! User-adjustable settings, persisted between runs by eframe.

use parterre_core::blame::Moves;
use parterre_core::file_diff::{Whitespace, WordMode};
use parterre_core::layout::LayoutOptions;
use parterre_core::log::LogOptions;
use parterre_core::log_layout::{Dividers, LogLayout};
use parterre_core::physics::NetParams;
use parterre_core::revgraph::GraphOptions;
use serde::{Deserialize, Serialize};

use crate::theme::{BranchColor, ThemeChoice};

/// The app id: names eframe's storage directory and, on Wayland, the window (matching
/// `packaging/linux/parterre.desktop`).
pub const APP_ID: &str = "parterre";
/// What the app was called up to 0.2, and its app id then.
const OLD_APP_ID: &str = "gitgraph";

// The storage keys still carry the old name, so settings saved before the rename keep loading.
pub const STORAGE_KEY: &str = "gitgraph-settings";
/// Storage key for remembered node positions: repository path -> commit hash -> rest offset
/// from the layout, and whether the node was moved by hand.
pub const MOVES_KEY: &str = "gitgraph-rest-offsets";
/// Storage key for the recently opened repositories, newest first.
pub const RECENT_KEY: &str = "parterre-recent-repositories";
/// The format before nodes gave way to each other: only dropped (pinned) nodes and offsets.
const OLD_MOVES_KEY: &str = "gitgraph-moved-nodes";

pub type RememberedMoves =
    std::collections::HashMap<String, std::collections::HashMap<String, (f32, f32, bool)>>;

/// Carries over what the app saved while it was called gitgraph: the first time it runs as
/// parterre, it copies the old storage directory's `app.ron`, the one file eframe keeps there.
pub fn adopt_old_storage() {
    if let (Some(from), Some(to)) = (eframe::storage_dir(OLD_APP_ID), eframe::storage_dir(APP_ID)) {
        copy_storage(&from, &to);
    }
}

/// Copies `app.ron` from one storage directory into another that has none yet. Best effort:
/// when it fails, the app starts with default settings.
fn copy_storage(from: &std::path::Path, to: &std::path::Path) {
    let (old, new) = (from.join("app.ron"), to.join("app.ron"));
    if old.exists() && !new.exists() {
        let _ = std::fs::create_dir_all(to).and_then(|()| std::fs::copy(&old, &new));
    }
}

/// Loads remembered node positions, converting the older format.
pub fn load_moves(storage: &dyn eframe::Storage) -> RememberedMoves {
    if let Some(moves) = eframe::get_value(storage, MOVES_KEY) {
        return moves;
    }
    let old: std::collections::HashMap<String, std::collections::HashMap<String, (f32, f32)>> =
        eframe::get_value(storage, OLD_MOVES_KEY).unwrap_or_default();
    old.into_iter()
        .map(|(repo, nodes)| {
            let nodes = nodes
                .into_iter()
                .map(|(hex, (dx, dy))| (hex, (dx, dy, true)))
                .collect();
            (repo, nodes)
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeStyle {
    /// Straight segments through the bend points, as TortoiseGit draws them.
    #[default]
    Straight,
    /// Smooth curves that leave and enter nodes along the direction of history.
    Curved,
}

impl EdgeStyle {
    pub const ALL: [EdgeStyle; 2] = [EdgeStyle::Straight, EdgeStyle::Curved];

    pub fn label(self) -> &'static str {
        match self {
            EdgeStyle::Straight => "Straight",
            EdgeStyle::Curved => "Curved",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Arrows {
    /// Arrowhead at the parent (older) end, TortoiseGit's default.
    #[default]
    ToParent,
    /// Arrowhead at the child end: "Arrows point towards merges".
    ToChild,
    None,
}

impl Arrows {
    pub const ALL: [Arrows; 3] = [Arrows::ToParent, Arrows::ToChild, Arrows::None];

    pub fn label(self) -> &'static str {
        match self {
            Arrows::ToParent => "Point to parents",
            Arrows::ToChild => "Point towards merges",
            Arrows::None => "No arrows",
        }
    }
}

/// Bundles of drawing choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// As close to TortoiseGit as possible: straight edges, every edge separate, rows as wide
    /// as they need to be.
    Classic,
    /// parterre's default: curved edges bundled into trunks, and very wide rows split so that
    /// sibling branches stack up.
    Modern,
}

impl Look {
    pub const ALL: [Look; 2] = [Look::Modern, Look::Classic];

    pub fn label(self) -> &'static str {
        match self {
            Look::Classic => "Classic (TortoiseGit)",
            Look::Modern => "Modern",
        }
    }

    pub fn apply(self, s: &mut Settings) {
        let defaults = LayoutOptions::default();
        match self {
            Look::Classic => {
                s.edge_style = EdgeStyle::Straight;
                s.layout.concentrate_edges = false;
                s.layout.max_layer_width = 0.0;
            }
            Look::Modern => {
                s.edge_style = EdgeStyle::Curved;
                s.layout.concentrate_edges = true;
                s.layout.max_layer_width = defaults.max_layer_width;
            }
        }
    }

    /// The look `s` currently matches, if any.
    pub fn of(s: &Settings) -> Option<Look> {
        Look::ALL.into_iter().find(|look| {
            let mut probe = s.clone();
            look.apply(&mut probe);
            probe == *s
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub graph: GraphOptions,
    pub layout: LayoutOptions,
    pub net: NetParams,
    pub theme: ThemeChoice,
    /// The text size of every window (egui's zoom factor; 1.0 = 100%). The graph has its own
    /// zoom.
    pub text_size: f32,
    pub edge_style: EdgeStyle,
    pub arrows: Arrows,
    /// Overview map of the whole graph in the bottom-right corner.
    pub show_overview: bool,
    pub show_status_bar: bool,
    /// Label edges with the number of commits collapsed into them.
    pub show_hidden_counts: bool,
    /// Highlight the edges of the hovered and selected nodes.
    pub highlight_edges: bool,
    /// Keep moved nodes where they are, per repository, across runs and relayouts.
    pub remember_moves: bool,
    /// Reload when the repository's refs change (TortoiseGit reloads only on F5).
    pub auto_reload: bool,
    /// Colours for branches by name; the first matching rule wins.
    pub branch_colors: Vec<BranchColor>,
    pub log_window: LogWindowSettings,
    pub diff_window: DiffWindowSettings,
    pub compare_window: CompareWindowSettings,
    pub blame_window: BlameWindowSettings,
}

/// What the compare window remembers across runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompareWindowSettings {
    /// Inner size in points.
    pub size: [f32; 2],
    /// Compare with the common ancestor rather than with the left commit.
    pub since_ancestor: bool,
}

impl Default for CompareWindowSettings {
    fn default() -> Self {
        CompareWindowSettings {
            size: [900.0, 640.0],
            since_ancestor: false,
        }
    }
}

/// The choices last made in a blame window, which the next one starts with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlameWindowSettings {
    /// Inner size in points.
    pub size: [f32; 2],
    /// Changes in whitespace alone don't make a line new (`git blame -w`).
    pub ignore_whitespace: bool,
    /// Whether moved and copied lines keep the commit that wrote them.
    pub moves: Moves,
    /// The history pane below the text is shown.
    pub show_history: bool,
    /// Height of the history pane in points, with its headings.
    pub history_height: f32,
}

impl Default for BlameWindowSettings {
    fn default() -> Self {
        BlameWindowSettings {
            size: [1100.0, 800.0],
            ignore_whitespace: false,
            moves: Moves::Off,
            show_history: true,
            // The headings and 8 rows.
            history_height: 26.0 + 8.0 * 24.0,
        }
    }
}

/// What the log window remembers across runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogWindowSettings {
    /// Inner size in points. (Its position can't be set on Wayland, so it isn't kept.)
    pub size: [f32; 2],
    /// How the panes are arranged.
    pub layout: LogLayout,
    /// Where the dividers are, for each layout.
    pub dividers: Dividers,
    /// Which commits the log walks, and their order.
    pub options: LogOptions,
}

impl Default for LogWindowSettings {
    fn default() -> Self {
        LogWindowSettings {
            size: [1100.0, 760.0],
            layout: LogLayout::default(),
            dividers: Dividers::default(),
            options: LogOptions::default(),
        }
    }
}

/// A diff window's two forms.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiffForm {
    /// The old version on the left, the new on the right.
    #[default]
    SideBySide,
    /// One column: a change's removed lines, then its added lines.
    Unified,
}

/// The choices last made in a diff window, which the next one starts with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiffWindowSettings {
    /// Inner size in points.
    pub size: [f32; 2],
    pub form: DiffForm,
    pub words: WordMode,
    pub whitespace: Whitespace,
    /// Fold unchanged stretches away.
    pub fold: bool,
}

impl Default for DiffWindowSettings {
    fn default() -> Self {
        DiffWindowSettings {
            size: [1200.0, 800.0],
            form: DiffForm::default(),
            words: WordMode::default(),
            whitespace: Whitespace::default(),
            fold: true,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        let mut s = Settings {
            graph: GraphOptions::default(),
            layout: LayoutOptions::default(),
            net: NetParams::default(),
            theme: ThemeChoice::default(),
            text_size: 1.0,
            edge_style: EdgeStyle::default(),
            arrows: Arrows::default(),
            show_overview: false,
            show_status_bar: true,
            show_hidden_counts: false,
            highlight_edges: true,
            remember_moves: false,
            auto_reload: true,
            branch_colors: Vec::new(),
            log_window: LogWindowSettings::default(),
            diff_window: DiffWindowSettings::default(),
            compare_window: CompareWindowSettings::default(),
            blame_window: BlameWindowSettings::default(),
        };
        Look::Modern.apply(&mut s);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_old_storage_once() {
        let tmp = tempfile::tempdir().unwrap();
        let (from, to) = (tmp.path().join("old"), tmp.path().join("new"));
        std::fs::create_dir(&from).unwrap();
        std::fs::write(from.join("app.ron"), "saved").unwrap();
        copy_storage(&from, &to);
        assert_eq!(
            std::fs::read_to_string(to.join("app.ron")).unwrap(),
            "saved"
        );

        // Once the new directory has its own file, the old one is left alone.
        std::fs::write(from.join("app.ron"), "saved later").unwrap();
        copy_storage(&from, &to);
        assert_eq!(
            std::fs::read_to_string(to.join("app.ron")).unwrap(),
            "saved"
        );
    }

    #[test]
    fn log_window_settings_saved_before_layouts_still_load() {
        // As the log window saved them with layout A only (#39).
        let old: Settings = ron::from_str("(log_window: (size: (900.0, 600.0)))").unwrap();
        assert_eq!(old.log_window.size, [900.0, 600.0]);
        assert_eq!(old.log_window.layout, LogLayout::Stacked);
        assert_eq!(old.log_window.dividers, Dividers::default());

        // Dividers saved for some layouts only keep the defaults of the others.
        let partial: LogWindowSettings =
            ron::from_str("(layout: FilesRight, dividers: (side_by_side: (0.3, 0.5)))").unwrap();
        assert_eq!(partial.layout, LogLayout::FilesRight);
        assert_eq!(partial.dividers.side_by_side, [0.3, 0.5]);
        assert_eq!(partial.dividers.stacked, Dividers::default().stacked);
    }

    #[test]
    fn settings_saved_before_diff_windows_start_with_the_defaults() {
        let old: Settings = ron::from_str("(log_window: (size: (900.0, 600.0)))").unwrap();
        assert_eq!(old.diff_window, DiffWindowSettings::default());
        assert!(old.diff_window.fold);

        let mut s = Settings::default();
        s.diff_window.form = DiffForm::Unified;
        s.diff_window.words = WordMode::Block;
        s.diff_window.whitespace = Whitespace::IgnoreAll;
        s.diff_window.fold = false;
        let back: Settings = ron::from_str(&ron::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.diff_window, s.diff_window);
    }

    #[test]
    fn text_size_starts_at_100_percent_and_is_kept() {
        let old: Settings = ron::from_str("(log_window: (size: (900.0, 600.0)))").unwrap();
        assert_eq!(old.text_size, 1.0);

        let s = Settings {
            text_size: 1.25,
            ..Settings::default()
        };
        let back: Settings = ron::from_str(&ron::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.text_size, 1.25);
    }

    #[test]
    fn the_history_pane_is_remembered_and_shown_by_default() {
        let old: Settings = ron::from_str("(blame_window: (size: (900.0, 600.0)))").unwrap();
        assert!(old.blame_window.show_history);
        assert_eq!(
            old.blame_window.history_height,
            BlameWindowSettings::default().history_height
        );

        let mut s = Settings::default();
        s.blame_window.show_history = false;
        s.blame_window.history_height = 300.0;
        let back: Settings = ron::from_str(&ron::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.blame_window, s.blame_window);
    }

    #[test]
    fn log_layout_and_dividers_survive_a_round_trip() {
        let mut s = Settings::default();
        s.log_window.layout = LogLayout::DetailsBelow;
        s.log_window.dividers.set(LogLayout::DetailsBelow, 1, 0.3);
        let text = ron::to_string(&s).unwrap();
        let back: Settings = ron::from_str(&text).unwrap();
        assert_eq!(back.log_window, s.log_window);
    }
}
