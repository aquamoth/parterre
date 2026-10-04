//! parterre: a standalone TortoiseGit-style revision graph viewer.

// Release builds on Windows are GUI-subsystem apps (no console window); see `console`.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod automation;
mod browser;
mod console;
mod dialogs;
mod export;
mod file_dialog;
mod file_manager;
mod frame_pacing;
mod icon;
mod menu;
mod raster;
mod record;
mod render;
mod scene;
mod script;
mod settings;
mod settings_file;
mod system_theme;
mod text_size;
mod theme;
mod upstreams;
// Runs in build.rs; compiled here only for its tests.
#[cfg(test)]
mod version;
mod view;
mod widgets;
// Runs in build.rs; compiled here only for its tests.
#[cfg(test)]
mod win_resource;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use eframe::egui;
use parterre_core::file_diff::{Whitespace, WordMode};
use parterre_core::layout::Direction;
use parterre_core::log_layout::LogLayout;
use parterre_core::physics::DragModel;
use parterre_core::revgraph::Simplification;

use crate::automation::Automation;
use crate::theme::ThemeChoice;

/// This build's version: `0.5.0-rc1 (a1b2c3d)` for a tagged release,
/// `0.5.1-dev.3+a1b2c3d` three commits after `v0.5.0`.
const VERSION: &str = env!("PARTERRE_VERSION");

/// Show the revision graph of a git repository: how its branches and tags relate.
#[derive(Debug, Parser)]
#[command(version = VERSION, about)]
struct Cli {
    /// Repository to show (any directory inside it). Without one, the repository of the current
    /// directory, or none: the window then asks for one.
    path: Option<PathBuf>,

    /// Which commits to show.
    #[arg(long, value_enum)]
    mode: Option<Mode>,

    /// Where the newest commits go.
    #[arg(long, value_enum)]
    direction: Option<Dir>,

    /// Overall look: "modern" (curved, bundled edges) or "classic" (as TortoiseGit).
    #[arg(long, value_enum)]
    look: Option<LookArg>,

    /// Maximum row width before siblings stack up (0 = unlimited, as TortoiseGit).
    #[arg(long)]
    max_row_width: Option<f32>,

    /// Where the default branch's first-parent line goes: straight down the middle with the
    /// branches on the side that stays narrowest ("centred"), the same with branches taking
    /// turns ("alternating"), or leftmost, bending towards its branches ("leftmost").
    #[arg(long, value_enum)]
    trunk: Option<TrunkArg>,

    /// Show only the history of HEAD.
    #[arg(long)]
    current_branch: bool,

    /// Only branches and tags whose names contain one of these comma-separated words.
    #[arg(long, value_name = "WORDS")]
    filter: Option<String>,

    /// Hide branches matching these comma-separated wildcards, e.g. 'pipeline/*,release/*',
    /// unless a shown branch's history contains them.
    #[arg(long, value_name = "PATTERNS")]
    hide: Option<String>,

    /// Colour branches matching PATTERNS, e.g. 'feature/*=#9b59b6'. Repeat for more rules;
    /// the first match wins. Replaces the saved colour rules.
    #[arg(long, value_name = "PATTERNS=COLOR", value_parser = theme::BranchColor::parse)]
    branch_color: Vec<theme::BranchColor>,

    /// Hide remote-tracking branches.
    #[arg(long)]
    no_remotes: bool,

    /// Hide tags.
    #[arg(long)]
    no_tags: bool,

    /// Show open pull requests from GitHub on the commits they propose, even if turned off in
    /// the settings (needs a signed-in gh; not with --export).
    #[arg(long)]
    pull_requests: bool,

    /// Show the pull requests in FILE instead of asking GitHub, for screenshots and demos:
    /// a JSON array of {"number", "title", "author", "draft", "head", "base"}, head and base
    /// being branches of origin (see docs/automation.md). Implies --pull-requests.
    #[arg(long, value_name = "FILE", hide = true)]
    pull_requests_from: Option<PathBuf>,

    /// Show the other worktrees, even if turned off in the settings.
    #[arg(long)]
    worktrees: bool,

    /// Colour theme.
    #[arg(long, value_enum)]
    theme: Option<Theme>,

    /// Text size of every window (1 = 100%, 0.5 to 3); the graph keeps its own zoom.
    #[arg(long, value_name = "SIZE")]
    text_size: Option<f32>,

    /// Initial window size, e.g. 1600x1000.
    #[arg(long, value_parser = parse_size)]
    window_size: Option<(f32, f32)>,

    /// Write the graph to FILE and exit, without opening a window: SVG, or PNG or WebP if FILE
    /// ends in .png or .webp (at 100%, or --zoom).
    #[arg(long, value_name = "FILE")]
    export: Option<PathBuf>,

    /// Save the window as a PNG and exit, once the graph is in (after --script, if given).
    #[arg(long, value_name = "FILE")]
    screenshot: Option<PathBuf>,

    /// Run a workflow script (FILE, or - for standard input): click, type and take screenshots
    /// of any window, dialog or menu, one step per line (see docs/automation.md), then exit.
    #[arg(long, value_name = "FILE")]
    script: Option<PathBuf>,

    /// Record the window to FILE: a .gif, a .mp4, .webm, .mkv or .mov (made by ffmpeg), or a
    /// folder of PNG frames. Without --script, until the window is closed.
    #[arg(long, value_name = "FILE")]
    record: Option<PathBuf>,

    /// Start with the whole graph in view instead of at HEAD.
    #[arg(long)]
    fit: bool,

    /// Show the overview map.
    #[arg(long, hide = true)]
    overview: bool,

    /// Zoom (1 = 100%) of a PNG or WebP --export, or of a scripted run around the centre of its
    /// initial view.
    #[arg(long)]
    zoom: Option<f32>,

    /// The diff window's form (a setting).
    #[arg(long, value_enum, hide = true)]
    diff_form: Option<DiffFormArg>,

    /// How the diff window finds changed words (a setting).
    #[arg(long, value_enum, hide = true)]
    diff_words: Option<DiffWordsArg>,

    /// What whitespace counts in the diff window (a setting).
    #[arg(long, value_enum, hide = true)]
    diff_whitespace: Option<DiffWhitespaceArg>,

    /// Show the whole file in the diff window instead of folding unchanged stretches.
    #[arg(long, hide = true)]
    diff_unfolded: bool,

    /// Plain text in the diff and blame windows: no syntax colour (a setting).
    #[arg(long, hide = true)]
    no_syntax_colour: bool,

    /// The child process of the syntax colour (#209): highlights stdin as this language and
    /// writes the spans to stdout.
    #[arg(long, hide = true, value_name = "LANG")]
    highlight: Option<String>,

    /// The log window's layout: stacked, side-by-side, details-below or
    /// files-right, or a, b, c or d.
    #[arg(long, value_enum, hide = true)]
    log_layout: Option<LogLayoutArg>,

    /// What moves when dragging a node.
    #[arg(long, value_enum, hide = true)]
    drag_mode: Option<DragModeArg>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DiffFormArg {
    Side,
    Unified,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DiffWordsArg {
    Similar,
    Position,
    Block,
    Off,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DiffWhitespaceArg {
    Compare,
    IgnoreChanges,
    IgnoreAll,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DragModeArg {
    Adapt,
    Free,
    Subtree,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum LogLayoutArg {
    #[value(alias = "a")]
    Stacked,
    #[value(alias = "b")]
    SideBySide,
    #[value(alias = "c")]
    DetailsBelow,
    #[value(alias = "d")]
    FilesRight,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Mode {
    /// Commits with refs, and merges joining them (TortoiseGit default).
    Labelled,
    /// Also every fork point and merge (TortoiseGit "Show branchings and merges").
    Branches,
    /// Every commit.
    All,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum TrunkArg {
    Centred,
    Alternating,
    Leftmost,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum LookArg {
    Modern,
    Classic,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Dir {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Theme {
    System,
    Light,
    Dark,
}

fn parse_size(s: &str) -> Result<(f32, f32), String> {
    let (w, h) = s.split_once(['x', 'X']).ok_or("expected WIDTHxHEIGHT")?;
    Ok((
        w.parse().map_err(|_| "bad width")?,
        h.parse().map_err(|_| "bad height")?,
    ))
}

fn main() -> ExitCode {
    console::attach_parent();
    let mut cli = Cli::parse();
    if let Some(id) = cli.highlight.as_deref() {
        return ExitCode::from(parterre_highlight::serve(id));
    }
    cli.path = cli.path.take().map(repair_quoted_root);
    let script = match cli.script.as_deref().map(read_script).transpose() {
        Ok(script) => script,
        Err(e) => {
            eprintln!("parterre: {e}");
            return ExitCode::FAILURE;
        }
    };
    let record = match cli.record.as_deref().map(record::Format::of).transpose() {
        Ok(format) => cli.record.clone().zip(format),
        Err(e) => {
            eprintln!("parterre: {e}");
            return ExitCode::FAILURE;
        }
    };
    let canned_pull_requests = match &cli.pull_requests_from {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(json) => Some(json),
            Err(e) => {
                eprintln!("parterre: could not read {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };
    let scripted = cli.screenshot.is_some() || script.is_some();
    // Why the repository named on the command line didn't open, when the window says so.
    let mut open_error = None;
    let repo = match &cli.path {
        Some(path) => match parterre_core::git::load_repo(path) {
            Ok(repo) => Some(repo),
            // In a terminal or a scripted run, say why and stop. Started from Explorer's menu, a
            // desktop entry or a shortcut there is no one to read stderr, so the window opens
            // and shows it.
            Err(e) if cli.export.is_some() || scripted || std::io::stderr().is_terminal() => {
                eprintln!("parterre: {e}");
                return ExitCode::FAILURE;
            }
            Err(e) => {
                open_error = Some(format!("Could not open {}: {e}", path.display()));
                None
            }
        },
        // Started from a menu or file manager, the current directory is seldom a repository.
        None => parterre_core::git::load_repo(std::path::Path::new(".")).ok(),
    };

    if let Some(path) = cli.export.clone() {
        let Some(repo) = repo else {
            eprintln!("parterre: not in a git repository; name one to export");
            return ExitCode::FAILURE;
        };
        let mut settings = settings::Settings::default();
        apply_cli(&cli, &mut settings);
        let zoom = cli.zoom.unwrap_or(1.0);
        return match export_headless(&std::sync::Arc::new(repo), &settings, &path, zoom) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("parterre: could not write {}: {e}", path.display());
                ExitCode::FAILURE
            }
        };
    }

    // A screenshot run exits by itself and reports where it saved, as a recording does; an
    // interactive window must not be tied to the terminal it was started from.
    if !scripted && record.is_none() {
        console::detach();
    }

    let (w, h) = cli.window_size.unwrap_or((1400.0, 900.0));
    // On Wayland a vsync'ed swap of a hidden window blocks the whole app (egui#5145); see
    // `frame_pacing`. eframe reads this once, when it creates the GL context.
    let vsync = !frame_pacing::wayland_session();
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(app::window_title(repo.as_ref()))
            .with_app_id(settings::APP_ID)
            .with_inner_size([w, h])
            .with_min_inner_size([400.0, 300.0])
            .with_icon(std::sync::Arc::new(icon::icon())),
        persistence_path: settings::storage_file(),
        ..Default::default()
    };
    options.glow_options.vsync = vsync;
    let mut automation = Automation::default();
    automation.fit = cli.fit;
    automation.zoom = cli.zoom;
    automation.pull_requests = canned_pull_requests;
    // `--screenshot` alone is a script of one step.
    if script.is_some() || cli.screenshot.is_some() {
        let mut runner = automation::Runner::new(script.unwrap_or_default());
        if let Some(path) = cli.screenshot.clone() {
            runner.push_screenshot(path);
        }
        automation.script = Some(runner);
    }
    automation.record = record.map(|(path, format)| record::Recorder::new(path, format));
    let overrides = move |s: &mut settings::Settings| apply_cli(&cli, s);
    let result = eframe::run_native(
        settings::APP_ID,
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::ParterreApp::new(
                cc, repo, open_error, overrides, automation, vsync,
            )))
        }),
    );
    match result {
        Ok(()) if automation::failed() => ExitCode::FAILURE,
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("parterre: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The steps of a workflow script, from a file or (`-`) standard input.
fn read_script(path: &std::path::Path) -> Result<Vec<script::Line>, String> {
    let text = if path.as_os_str() == "-" {
        std::io::read_to_string(std::io::stdin())
    } else {
        std::fs::read_to_string(path)
    }
    .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    script::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Applies command-line options on top of the stored settings.
fn apply_cli(cli: &Cli, s: &mut settings::Settings) {
    if let Some(mode) = cli.mode {
        s.graph.simplification = match mode {
            Mode::Labelled => Simplification::Decorated,
            Mode::Branches => Simplification::BranchesAndMerges,
            Mode::All => Simplification::AllCommits,
        };
    }
    if let Some(dir) = cli.direction {
        s.layout.direction = match dir {
            Dir::Top => Direction::NewestTop,
            Dir::Bottom => Direction::NewestBottom,
            Dir::Left => Direction::NewestLeft,
            Dir::Right => Direction::NewestRight,
        };
    }
    match cli.look {
        Some(LookArg::Modern) => settings::Look::Modern.apply(s),
        Some(LookArg::Classic) => settings::Look::Classic.apply(s),
        None => {}
    }
    if let Some(w) = cli.max_row_width {
        s.layout.max_layer_width = w;
    }
    if let Some(trunk) = cli.trunk {
        use parterre_core::layout::Trunk;
        s.layout.trunk = match trunk {
            TrunkArg::Centred => Trunk::Centred,
            TrunkArg::Alternating => Trunk::Alternating,
            TrunkArg::Leftmost => Trunk::Leftmost,
        };
    }
    if cli.overview {
        s.show_overview = true;
    }
    if cli.current_branch {
        s.graph.current_branch_only = true;
    }
    if let Some(filter) = &cli.filter {
        s.graph.ref_filter = filter.clone();
    }
    if let Some(hide) = &cli.hide {
        s.graph.hide_branches = hide.clone();
    }
    if !cli.branch_color.is_empty() {
        s.branch_colors = cli.branch_color.clone();
    }
    if cli.no_remotes {
        s.graph.show_remote_branches = false;
    }
    if cli.no_tags {
        s.graph.show_tags = false;
    }
    if cli.pull_requests || cli.pull_requests_from.is_some() {
        s.graph.show_pull_requests = true;
    }
    if cli.worktrees {
        s.graph.show_worktrees = true;
    }
    if let Some(mode) = cli.drag_mode {
        s.net.model = match mode {
            DragModeArg::Adapt => DragModel::Adapt,
            DragModeArg::Free => DragModel::Free,
            DragModeArg::Subtree => DragModel::Subtree,
        };
    }
    if let Some(layout) = cli.log_layout {
        s.log_window.layout = match layout {
            LogLayoutArg::Stacked => LogLayout::Stacked,
            LogLayoutArg::SideBySide => LogLayout::SideBySide,
            LogLayoutArg::DetailsBelow => LogLayout::DetailsBelow,
            LogLayoutArg::FilesRight => LogLayout::FilesRight,
        };
    }
    if let Some(form) = cli.diff_form {
        s.diff_window.form = match form {
            DiffFormArg::Side => settings::DiffForm::SideBySide,
            DiffFormArg::Unified => settings::DiffForm::Unified,
        };
    }
    if let Some(words) = cli.diff_words {
        s.diff_window.words = match words {
            DiffWordsArg::Similar => WordMode::Similar,
            DiffWordsArg::Position => WordMode::Position,
            DiffWordsArg::Block => WordMode::Block,
            DiffWordsArg::Off => WordMode::Off,
        };
    }
    if let Some(ws) = cli.diff_whitespace {
        s.diff_window.whitespace = match ws {
            DiffWhitespaceArg::Compare => Whitespace::Compare,
            DiffWhitespaceArg::IgnoreChanges => Whitespace::IgnoreChanges,
            DiffWhitespaceArg::IgnoreAll => Whitespace::IgnoreAll,
        };
    }
    if cli.diff_unfolded {
        s.diff_window.fold = false;
    }
    if cli.no_syntax_colour {
        s.syntax_colour = false;
    }
    if let Some(size) = cli.text_size {
        s.text_size = size;
    }
    if let Some(theme) = cli.theme {
        s.theme = match theme {
            Theme::System => ThemeChoice::System,
            Theme::Light => ThemeChoice::Light,
            Theme::Dark => ThemeChoice::Dark,
        };
    }
}

/// Undoes a quirk of Windows command lines: a quoted path ending in a backslash, as Explorer
/// passes the root of a drive (`"C:\"`), arrives with the backslash taken as escaping the
/// closing quote (`C:"`). No Windows path can contain a quote, so a trailing one is put back
/// as the backslash. Elsewhere a quote is a valid character and is left alone.
fn repair_quoted_root(path: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return path;
    }
    match path.to_str().and_then(|s| s.strip_suffix('"')) {
        Some(root) => PathBuf::from(format!("{root}\\")),
        None => path,
    }
}

/// Lays the graph out without a window and writes it as SVG, PNG or WebP (by the extension).
/// PNG and WebP are drawn at `zoom`, one pixel per point.
fn export_headless(
    repo: &std::sync::Arc<parterre_core::Repo>,
    settings: &settings::Settings,
    path: &std::path::Path,
    zoom: f32,
) -> anyhow::Result<()> {
    // Checked before the layout, which can take seconds.
    anyhow::ensure!(
        export::Format::from_path(path).is_some(),
        "unknown format: name the file .svg, .png or .webp"
    );
    let scene = scene::Scene::headless(repo, settings);
    let palette = theme::Palette::new(
        settings.theme == theme::ThemeChoice::Dark,
        &settings.branch_colors,
    );
    let what = export::write(path, &scene, settings, &palette, zoom, 1.0)?;
    eprintln!(
        "wrote {} ({} nodes; {what})",
        path.display(),
        scene.node_count()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn repairs_a_quoted_drive_root() {
        // Explorer's `"%V"` on the background of C:\ gives `"C:\"`, which arrives as `C:"`.
        assert_eq!(
            repair_quoted_root(PathBuf::from("C:\"")),
            PathBuf::from("C:\\")
        );
        assert_eq!(
            repair_quoted_root(PathBuf::from("C:\\src\\repo")),
            PathBuf::from("C:\\src\\repo")
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn keeps_quotes_where_names_may_have_them() {
        assert_eq!(
            repair_quoted_root(PathBuf::from("a\"")),
            PathBuf::from("a\"")
        );
    }
}
