//! User-adjustable settings, persisted between runs by eframe.

use parterre_core::Repo;
use parterre_core::blame::Moves;
use parterre_core::file_diff::{Whitespace, WordMode};
use parterre_core::layout::LayoutOptions;
use parterre_core::log::LogOptions;
use parterre_core::log_layout::{Dividers, LogLayout};
use parterre_core::physics::NetParams;
use parterre_core::revgraph::GraphOptions;
use serde::{Deserialize, Serialize};

use crate::theme::{BranchColor, ThemeChoice};

/// The app id: the Wayland app id and X11 window class, which a desktop pairs with the desktop
/// entry of the same name (`packaging/linux/se.trustfall.parterre.desktop`) for the icon. Also
/// the Flatpak id.
pub const APP_ID: &str = "se.trustfall.parterre";
/// Names the storage directory, which is older than the app id and stays where it was.
const STORAGE_ID: &str = "parterre";

/// The file eframe saves the settings in. Left to itself it would name the directory after the
/// root viewport's app id.
pub fn storage_file() -> Option<std::path::PathBuf> {
    eframe::storage_dir(STORAGE_ID).map(|dir| dir.join("app.ron"))
}

/// True if the file eframe saves in is there but can't be read: eframe then starts from
/// nothing, and overwrites it on saving.
pub fn storage_corrupt() -> bool {
    storage_file().is_some_and(|file| file_corrupt(&file))
}

fn file_corrupt(file: &std::path::Path) -> bool {
    match std::fs::read_to_string(file) {
        Ok(text) => ron::from_str::<std::collections::HashMap<String, String>>(&text).is_err(),
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

/// Storage key for remembered node positions: repository path -> commit hash -> rest offset
/// from the layout, and whether the node was moved by hand. Lists the children of displaced
/// nodes too, so that commits missing from it are new (see
/// `parterre_core::physics::Net::rest_offsets`).
pub const MOVES_KEY: &str = "parterre-moves";
/// Storage key for the recently opened repositories, newest first.
pub const RECENT_KEY: &str = "parterre-recent-repositories";
pub type RememberedMoves =
    std::collections::HashMap<String, std::collections::HashMap<String, (f32, f32, bool)>>;

/// Storage key for what is sent to PostHog: the first-run prompt's answer, the install ID and
/// the version that ran last (#261). Kept apart from the settings: not exported, imported or
/// reset with them.
pub const PRIVACY_KEY: &str = "parterre-privacy";

/// What is sent to PostHog, as stored under [`PRIVACY_KEY`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Privacy {
    /// The first-run prompt's answer, as Settings › Privacy has changed it since; `None` until
    /// it is answered.
    pub answer: Option<PrivacyAnswer>,
    /// The version that ran last, as `$app_version` names it, for `Application Updated`.
    pub last_version: Option<String>,
}

/// The first-run prompt's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacyAnswer {
    /// Made when the prompt was answered, and never changed.
    pub install_id: String,
    pub usage_statistics: bool,
    pub crash_reports: bool,
}

impl PrivacyAnswer {
    pub fn choices(&self) -> parterre_telemetry::Choices {
        parterre_telemetry::Choices {
            usage_statistics: self.usage_statistics,
            crash_reports: self.crash_reports,
        }
    }
}

impl Privacy {
    /// The stored one; as before the first run without one, or if it can't be read (asked
    /// again then, with a new install ID).
    pub fn load(storage: &dyn eframe::Storage) -> Privacy {
        eframe::get_value(storage, PRIVACY_KEY).unwrap_or_default()
    }

    /// The first-run prompt answered with `choices`: the install ID is made.
    pub fn answer(&mut self, choices: parterre_telemetry::Choices) {
        self.answer = Some(PrivacyAnswer {
            install_id: parterre_telemetry::new_install_id(),
            usage_statistics: choices.usage_statistics,
            crash_reports: choices.crash_reports,
        });
    }

    pub fn choices(&self) -> Option<parterre_telemetry::Choices> {
        self.answer.as_ref().map(PrivacyAnswer::choices)
    }
}

/// Loads remembered node positions.
pub fn load_moves(storage: &dyn eframe::Storage) -> RememberedMoves {
    eframe::get_value(storage, MOVES_KEY).unwrap_or_default()
}

/// The settings of one repository: what the Filter popover sets. Every other setting is
/// parterre's, the same in every repository.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoSettings {
    pub current_branch_only: bool,
    pub first_parent_only: bool,
    pub ref_filter: String,
    pub hide_branches: String,
}

impl RepoSettings {
    /// The repository settings among `graph`, which holds those of the repository shown.
    pub fn of(graph: &GraphOptions) -> RepoSettings {
        RepoSettings {
            current_branch_only: graph.current_branch_only,
            first_parent_only: graph.first_parent_only,
            ref_filter: graph.ref_filter.clone(),
            hide_branches: graph.hide_branches.clone(),
        }
    }

    pub fn apply(&self, graph: &mut GraphOptions) {
        graph.current_branch_only = self.current_branch_only;
        graph.first_parent_only = self.first_parent_only;
        graph.ref_filter.clone_from(&self.ref_filter);
        graph.hide_branches.clone_from(&self.hide_branches);
    }

    /// Which repository `repo` is, for its settings: its main worktree, so that every worktree
    /// of a repository has the same.
    pub fn key(repo: &Repo) -> String {
        let main = repo.worktrees.first().map_or(&repo.path, |main| &main.path);
        main.display().to_string()
    }

    /// The name of the repository `key` is, for showing.
    pub fn name(key: &str) -> String {
        let path = std::path::Path::new(key);
        path.file_name()
            .map_or_else(|| key.to_owned(), |n| n.to_string_lossy().into_owned())
    }
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
            Look::Classic => "Classic",
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
    /// Colour code by its language in the diff and blame windows (#209). Their toolbars
    /// toggle it too.
    pub syntax_colour: bool,
    /// Ask GitHub once a day whether a newer release is out (#258).
    pub check_for_updates: bool,
    /// Colours for branches by name; the first matching rule wins.
    pub branch_colors: Vec<BranchColor>,
    pub log_window: LogWindowSettings,
    pub diff_window: DiffWindowSettings,
    pub compare_window: CompareWindowSettings,
    pub blame_window: BlameWindowSettings,
    pub settings_window: SettingsWindowSettings,
}

/// What the settings window remembers across runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsWindowSettings {
    /// Inner size in points.
    pub size: [f32; 2],
}

impl Default for SettingsWindowSettings {
    fn default() -> Self {
        // The sidebar, a page and the margins.
        SettingsWindowSettings {
            size: [646.0, 480.0],
        }
    }
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

impl Settings {
    /// Puts back in reach what settings edited by hand or saved by another version may put out
    /// of it: a divider, the text size.
    pub fn sanitize(&mut self) {
        self.log_window.dividers = self.log_window.dividers.clamped();
        self.text_size = parterre_core::text_size::sanitize(self.text_size);
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
            syntax_colour: true,
            check_for_updates: true,
            branch_colors: Vec::new(),
            log_window: LogWindowSettings::default(),
            diff_window: DiffWindowSettings::default(),
            compare_window: CompareWindowSettings::default(),
            blame_window: BlameWindowSettings::default(),
            settings_window: SettingsWindowSettings::default(),
        };
        Look::Modern.apply(&mut s);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_file_that_does_not_parse_is_corrupt() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("app.ron");
        assert!(!file_corrupt(&file));
        std::fs::write(&file, r#"{"parterre-settings": "{}", "window": "()"}"#).unwrap();
        assert!(!file_corrupt(&file));
        for text in ["", r#"{"parterre-settings": "{"#, "\0\0\0"] {
            std::fs::write(&file, text).unwrap();
            assert!(file_corrupt(&file), "{text:?}");
        }
    }

    #[test]
    fn settings_stay_where_they_were_before_the_app_id() {
        // eframe would name the directory after the app id; the settings stay under parterre.
        assert_ne!(APP_ID, STORAGE_ID);
        assert_eq!(
            storage_file(),
            eframe::storage_dir("parterre").map(|dir| dir.join("app.ron"))
        );
    }

    #[test]
    fn desktop_files_carry_the_app_id() {
        // Only in the repository: the published crate has no packaging directory.
        let linux = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/linux");
        if !linux.exists() {
            return;
        }
        let entry = std::fs::read_to_string(linux.join(format!("{APP_ID}.desktop"))).unwrap();
        for line in [format!("Icon={APP_ID}"), format!("StartupWMClass={APP_ID}")] {
            assert!(entry.lines().any(|l| l == line), "{line} missing");
        }
        // Handling folders would make parterre the default folder app on desktops that name
        // none, such as Xfce.
        assert!(
            !entry
                .lines()
                .any(|l| l.starts_with("MimeType=") && l.contains("inode/"))
        );
        let metainfo =
            std::fs::read_to_string(linux.join(format!("{APP_ID}.metainfo.xml"))).unwrap();
        assert!(metainfo.contains(&format!("<id>{APP_ID}</id>")));
        assert!(metainfo.contains(&format!(
            r#"<launchable type="desktop-id">{APP_ID}.desktop<"#
        )));
    }

    /// Settings as stored: JSON, read one setting at a time onto the defaults.
    fn read(json: &str) -> Settings {
        let file = serde_json::from_str(json).unwrap();
        parterre_core::lenient::read(&Settings::default(), &file).value
    }

    fn round_trip(s: &Settings) -> Settings {
        read(&serde_json::to_string(s).unwrap())
    }

    #[test]
    fn log_window_settings_missing_get_the_defaults() {
        let s = read(r#"{"log_window": {"size": [900.0, 600.0]}}"#);
        assert_eq!(s.log_window.size, [900.0, 600.0]);
        assert_eq!(s.log_window.layout, LogLayout::Stacked);
        assert_eq!(s.log_window.dividers, Dividers::default());

        // Dividers of some layouts only keep the defaults of the others.
        let s = read(
            r#"{"log_window": {"layout": "FilesRight", "dividers": {"side_by_side": [0.3, 0.5]}}}"#,
        );
        assert_eq!(s.log_window.layout, LogLayout::FilesRight);
        assert_eq!(s.log_window.dividers.side_by_side, [0.3, 0.5]);
        assert_eq!(s.log_window.dividers.stacked, Dividers::default().stacked);
    }

    #[test]
    fn diff_window_settings_start_with_the_defaults_and_are_kept() {
        let s = read(r#"{"log_window": {"size": [900.0, 600.0]}}"#);
        assert_eq!(s.diff_window, DiffWindowSettings::default());
        assert!(s.diff_window.fold);

        let mut s = Settings::default();
        s.diff_window.form = DiffForm::Unified;
        s.diff_window.words = WordMode::Block;
        s.diff_window.whitespace = Whitespace::IgnoreAll;
        s.diff_window.fold = false;
        assert_eq!(round_trip(&s).diff_window, s.diff_window);
    }

    #[test]
    fn text_size_starts_at_100_percent_and_is_kept() {
        assert_eq!(read("{}").text_size, 1.0);

        let s = Settings {
            text_size: 1.25,
            ..Settings::default()
        };
        assert_eq!(round_trip(&s).text_size, 1.25);
    }

    #[test]
    fn the_history_pane_is_remembered_and_shown_by_default() {
        let s = read(r#"{"blame_window": {"size": [900.0, 600.0]}}"#);
        assert!(s.blame_window.show_history);
        assert_eq!(
            s.blame_window.history_height,
            BlameWindowSettings::default().history_height
        );

        let mut s = Settings::default();
        s.blame_window.show_history = false;
        s.blame_window.history_height = 300.0;
        assert_eq!(round_trip(&s).blame_window, s.blame_window);
    }

    #[test]
    fn the_privacy_answer_and_install_id_survive_a_round_trip() {
        let mut privacy = Privacy::default();
        assert_eq!(privacy.choices(), None, "unanswered before the first run");
        privacy.answer(parterre_telemetry::Choices {
            usage_statistics: false,
            crash_reports: true,
        });
        privacy.last_version = Some("0.6.0".into());
        let text = serde_json::to_string(&privacy).unwrap();
        let back: Privacy = serde_json::from_str(&text).unwrap();
        assert_eq!(back, privacy);
        let choices = back.choices().unwrap();
        assert!(!choices.usage_statistics && choices.crash_reports);
        // Whatever can't be read is unanswered: asked again.
        let back: Privacy = serde_json::from_str(r#"{"answer": 3}"#).unwrap_or_default();
        assert_eq!(back.answer, None);
    }

    #[test]
    fn the_update_check_is_on_by_default_and_turned_off_for_good() {
        assert!(read("{}").check_for_updates);

        let s = Settings {
            check_for_updates: false,
            ..Settings::default()
        };
        assert!(!round_trip(&s).check_for_updates);
    }

    #[test]
    fn log_layout_and_dividers_survive_a_round_trip() {
        let mut s = Settings::default();
        s.log_window.layout = LogLayout::DetailsBelow;
        s.log_window.dividers.set(LogLayout::DetailsBelow, 1, 0.3);
        assert_eq!(round_trip(&s).log_window, s.log_window);
    }
}
