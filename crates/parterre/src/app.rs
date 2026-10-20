use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use eframe::egui::{
    self, Color32, FontId, Key, PointerButton, Pos2, Rect, RichText, Sense, Ui, Vec2, vec2,
};
use parterre_core::git::CommitDetails;
use parterre_core::layout::{Direction, LayoutOptions};
use parterre_core::physics::{DragModel, RestPlace};
use parterre_core::recent::Recent;
use parterre_core::revgraph::{GraphOptions, RevEdge};
use parterre_core::{Oid, Repo};

mod about;
mod auto_reload;
mod blame_window;
mod branches;
mod cherry_pick;
mod clone;
mod column_borders;
mod commands;
mod commit_table;
mod compare_window;
mod diff_window;
mod divider;
mod file_drop;
mod file_table;
mod git_menu;
mod log_window;
mod menu_bar;
mod merge;
mod merge_tool;
mod merged;
mod node_menu;
mod privacy;
mod pull_requests;
mod rebase;
mod remote;
mod reset;
mod revert;
mod settings_window;
mod syntax;
mod table_style;
mod text_view;
#[cfg(test)]
mod tool_harness;
mod toolbar;
mod updates;
mod viewport;

use toolbar::popup_id;
pub use toolbar::toolbar_button_id;

use settings_window::SettingsPage;

use crate::automation::Automation;
use crate::export::{self, Format};
use crate::file_dialog::Pending;
use crate::frame_pacing::FrameLimiter;
use crate::keys;
use crate::menu;
use crate::render::{self, Marks};
use crate::reveal::Reveal;
use crate::scene::{FONT_SIZE, Scene, to_point};
use crate::settings::{
    CLONE_PARENT_KEY, MOVES_KEY, PRIVACY_KEY, Privacy, RECENT_KEY, RememberedMoves, RepoSettings,
    Settings, load_moves,
};
use crate::settings_file::{self, Stored};
use crate::system_theme::SystemTheme;
use crate::text_size;
use crate::theme::{Palette, ThemeChoice};
use crate::usage;
use crate::view::View;
use crate::worker;
use compare_window::CompareRequest;

#[derive(Clone, Copy, Debug)]
pub(crate) enum PickFor {
    Folder,
    Export(Format),
    ExportSettings,
    ImportSettings,
}

/// Positions in world units.
#[derive(Clone, Copy, Debug)]
enum Drag {
    Node { grab_offset: Vec2 },
    Pan,
    Select { start: Pos2, end: Pos2 },
}

/// In the order selected: the last is the current node.
#[derive(Clone, Debug, Default)]
struct Selection {
    nodes: Vec<usize>,
}

impl Selection {
    fn current(&self) -> Option<usize> {
        self.nodes.last().copied()
    }

    fn contains(&self, node: usize) -> bool {
        self.nodes.contains(&node)
    }

    fn len(&self) -> usize {
        self.nodes.len()
    }

    fn set(&mut self, node: Option<usize>) {
        self.nodes.clear();
        self.nodes.extend(node);
    }

    fn add(&mut self, node: usize) {
        self.nodes.retain(|&n| n != node);
        self.nodes.push(node);
    }

    /// Keeps the current node current.
    fn extend(&mut self, nodes: impl IntoIterator<Item = usize>) {
        let mut seen: HashSet<usize> = self.nodes.iter().copied().collect();
        let current = self.nodes.pop();
        self.nodes.extend(
            nodes
                .into_iter()
                .filter(|&n| Some(n) != current && seen.insert(n)),
        );
        self.nodes.extend(current);
    }

    fn toggle(&mut self, node: usize) {
        if self.contains(node) {
            self.nodes.retain(|&n| n != node);
        } else {
            self.nodes.push(node);
        }
    }

    fn mask(&self, node_count: usize) -> Vec<bool> {
        let mut mask = vec![false; node_count];
        for &n in &self.nodes {
            if n < node_count {
                mask[n] = true;
            }
        }
        mask
    }

    /// The selection if it holds `node`, else `node` alone.
    fn group_of(&self, node: usize) -> Vec<usize> {
        if self.contains(node) {
            self.nodes.clone()
        } else {
            vec![node]
        }
    }
}

#[derive(Debug, Default)]
struct Details(file_table::Lister<Oid, Result<CommitDetails, String>>);

impl Details {
    /// `None` while loading.
    fn get(
        &mut self,
        repo_path: &Path,
        oid: Oid,
        ctx: &egui::Context,
    ) -> Option<&Result<CommitDetails, String>> {
        self.0.get(repo_path, oid, ctx, |git, oid| {
            git.details(oid).map_err(|e| e.to_string())
        })
    }

    fn message(&mut self, repo_path: &Path, oid: Oid, ctx: &egui::Context) -> Option<String> {
        self.get(repo_path, oid, ctx).map(|d| match d {
            Ok(d) => d.message.clone(),
            Err(e) => format!("({e})"),
        })
    }
}

#[derive(Debug)]
struct LayoutJob {
    rx: mpsc::Receiver<Scene>,
    options: (GraphOptions, LayoutOptions),
    view_anchor: Option<(Oid, Pos2)>,
    selected_commits: Vec<Oid>,
    /// Child and parent.
    selected_edge: Option<(Oid, Oid)>,
}

/// A [`RestPlace`] by commit, carried over a relayout.
#[derive(Clone, Debug, PartialEq)]
struct CarriedPlace {
    offset: (f32, f32),
    by_hand: bool,
    /// The first parent's commit, and the offset from it.
    beside: Option<(String, (f32, f32))>,
}

type Carried = HashMap<String, CarriedPlace>;

#[derive(Debug, Default)]
struct Search {
    query: String,
    hits: Vec<usize>,
    current_hit: Option<usize>,
    request_focus: bool,
}

/// For the status bar, or the settings window's.
#[derive(Debug)]
enum Note {
    Info(String),
    Error(String),
}

impl Note {
    fn text(&self) -> &str {
        match self {
            Note::Info(text) | Note::Error(text) => text,
        }
    }

    fn is_error(&self) -> bool {
        matches!(self, Note::Error(_))
    }
}

const CORRUPT_SETTINGS_STATUS: &str = "The saved settings were corrupt: defaults restored";

pub struct ParterreApp {
    /// The latest snapshot, for new layouts. The scene on screen keeps its own until a new
    /// layout replaces it.
    repo: Option<Arc<Repo>>,
    branches: branches::Tool,
    recent: Recent,
    folder_picker_requested: bool,
    file_dialog: Option<Pending<PickFor>>,
    title_shown: String,
    /// The repository parterre last asked to be started again on after an update (`restart`),
    /// `None` before it first asked.
    restart_on: Option<Option<PathBuf>>,
    settings: Settings,
    /// Every repository's settings. The shown one's live in `settings` until kept here.
    stored: Stored,
    persist: bool,
    scene: Option<Scene>,
    requested_options: Option<(GraphOptions, LayoutOptions)>,
    scene_options: Option<(GraphOptions, LayoutOptions)>,
    job: Option<LayoutJob>,
    view: View,
    view_at_frame_start: View,
    needs_initial_view: bool,
    canvas: Rect,
    /// Over the graph or its overview map, where Ctrl+wheel zooms the graph, not the text.
    graph_hovered: bool,
    hovered: Option<usize>,
    hovered_edge: Option<usize>,
    selection: Selection,
    selected_edge: Option<usize>,
    /// Roots, and the nodes a Subtree drag of them would move.
    subtree_preview: Option<(Vec<usize>, Vec<usize>)>,
    context_node: Option<usize>,
    context_menu_requested: bool,
    select_after_layout: Vec<Oid>,
    drag: Option<Drag>,
    search: Search,
    status: Option<Note>,
    show_shortcuts: bool,
    show_legend: bool,
    show_settings: bool,
    settings_page: SettingsPage,
    about: about::About,
    export_requested: Option<Format>,
    export_dir: Option<PathBuf>,
    settings_file_requested: Option<PickFor>,
    settings_dir: Option<PathBuf>,
    settings_note: Option<Note>,
    settings_import: Option<settings_window::Import>,
    confirm_reset_settings: bool,
    details: Details,
    log: log_window::LogWindow,
    diffs: diff_window::DiffWindows,
    blames: blame_window::BlameWindows,
    compare: compare_window::CompareWindow,
    merge_tools: merge_tool::MergeTools,
    /// The commit marked for comparison, and its label.
    marked: Option<(Oid, String)>,
    moves: RememberedMoves,
    watcher: Option<auto_reload::Watcher>,
    opening: Option<Opening>,
    reveal: Option<Reveal>,
    reloading: Option<(PathBuf, mpsc::Receiver<Result<Repo, String>>)>,
    pull_requests: pull_requests::PullRequestLoader,
    pull_requests_were_on: bool,
    /// The local branches squash- or rebase-merged (#384).
    merged: merged::MergedLoader,
    pull_requests_error: Option<parterre_forge::ForgeError>,
    /// With the ring it was started for: a ring chosen in Settings starts it again.
    update_check: Option<(parterre_telemetry::Ring, parterre_telemetry::UpdateCheck)>,
    release_told: Option<String>,
    release_dialog: Option<parterre_telemetry::Update>,
    telemetry: privacy::Telemetry,
    system_theme: SystemTheme,
    /// The theme last sent to the window, for its title bar.
    window_theme: Option<egui::SystemTheme>,
    settings_viewport: viewport::Viewport,
    settings_window_text_size: Option<f32>,
    settings_window_size: Option<Vec2>,
    window_icon: Arc<egui::IconData>,
    /// Typed into the zoom level while it has the focus.
    zoom_text: String,
    automation: Automation,
    frame_limiter: Option<FrameLimiter>,
    #[cfg(target_os = "macos")]
    native_menu: menu_bar::macos::NativeMenu,
    #[cfg(target_os = "macos")]
    cli_install: Option<mpsc::Receiver<Result<String, String>>>,
    #[cfg(target_os = "macos")]
    input_method: crate::macos::InputMethod,
}

impl std::fmt::Debug for ParterreApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParterreApp")
            .field("repo", &self.repo.as_ref().map(|r| &r.path))
            .finish_non_exhaustive()
    }
}

impl ParterreApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        repo: Option<Repo>,
        opening: Option<Opening>,
        overrides: impl FnOnce(&mut Settings),
        automation: Automation,
        vsync: bool,
        reveal: Option<Reveal>,
    ) -> ParterreApp {
        crate::startup::mark("window created");
        let persist = !automation.is_scripted();
        let storage = cc.storage.filter(|_| persist);
        let settings_file::Loaded {
            mut settings,
            stored,
            corrupt,
        } = settings_file::load(storage);
        // eframe starts from nothing when its whole file is corrupt.
        let corrupt = corrupt || persist && crate::settings::storage_corrupt();
        let repo_settings = repo
            .as_ref()
            .map_or_else(RepoSettings::default, |repo| stored.settings_of(repo));
        repo_settings.apply(&mut settings.graph);
        overrides(&mut settings);
        settings.sanitize();
        cc.egui_ctx.set_zoom_factor(settings.text_size);
        let moves: RememberedMoves = storage.map(load_moves).unwrap_or_default();
        let mut recent: Recent = storage
            .and_then(|s| eframe::get_value(s, RECENT_KEY))
            .unwrap_or_default();
        if let Some(repo) = &repo {
            recent.add(&repo.path);
        }
        let privacy = storage.map(Privacy::load).unwrap_or_default();
        let mut telemetry = privacy::Telemetry::new(privacy, automation.is_scripted());
        if let Some(repo) = &repo {
            telemetry.opened(repo);
        }
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        // Screenshots and recordings show one window: the others go in it.
        cc.egui_ctx
            .set_embed_viewports(automation.is_scripted() || automation.record.is_some());
        if automation.is_scripted() {
            cc.egui_ctx
                .add_plugin(crate::automation::TextCollector(automation.texts.clone()));
        }
        add_input_plugins(&cc.egui_ctx);
        // Scripted runs show one window and are short: they may run as fast as they can.
        let frame_limiter = (!vsync && !automation.is_scripted()).then(FrameLimiter::default);
        // On at startup, they weren't turned on by the user: loading them says nothing.
        let pull_requests_were_on = settings.graph.show_pull_requests;
        ParterreApp {
            title_shown: opening
                .as_ref()
                .map_or_else(|| window_title(repo.as_ref()), Opening::title),
            restart_on: None,
            repo: repo.map(Arc::new),
            recent,
            folder_picker_requested: false,
            file_dialog: None,
            settings,
            stored,
            persist,
            scene: None,
            requested_options: None,
            scene_options: None,
            job: None,
            view: View::default(),
            view_at_frame_start: View::default(),
            needs_initial_view: true,
            canvas: Rect::NOTHING,
            graph_hovered: false,
            hovered: None,
            hovered_edge: None,
            selection: Selection::default(),
            selected_edge: None,
            subtree_preview: None,
            branches: {
                let mut tool = branches::Tool::with_canned_pull_requests(
                    automation.canned_pull_requests.as_deref(),
                );
                tool.canned_repositories = automation
                    .canned_github_repositories
                    .as_deref()
                    .map(Arc::from);
                tool.read_clipboard = persist;
                tool.clone_parent = storage
                    .and_then(|s| eframe::get_value(s, CLONE_PARENT_KEY))
                    .flatten();
                tool
            },
            context_node: None,
            context_menu_requested: false,
            select_after_layout: Vec::new(),
            drag: None,
            search: Search::default(),
            status: corrupt
                .then(|| Note::Error(CORRUPT_SETTINGS_STATUS.to_owned()))
                .or_else(|| opening.as_ref().and_then(Opening::status)),
            opening,
            reveal,
            show_shortcuts: false,
            show_legend: false,
            show_settings: false,
            settings_page: SettingsPage::default(),
            about: about::About::default(),
            export_requested: None,
            export_dir: None,
            settings_file_requested: None,
            settings_dir: None,
            settings_note: None,
            settings_import: None,
            confirm_reset_settings: false,
            details: Details::default(),
            log: log_window::LogWindow::default(),
            diffs: diff_window::DiffWindows::new(highlight_engine()),
            blames: blame_window::BlameWindows::new(highlight_engine()),
            compare: compare_window::CompareWindow::default(),
            merge_tools: merge_tool::MergeTools::default(),
            marked: None,
            moves,
            watcher: None,
            reloading: None,
            pull_requests: match &automation.canned_pull_requests {
                Some(json) => pull_requests::PullRequestLoader::canned(json),
                None => pull_requests::PullRequestLoader::default(),
            },
            pull_requests_were_on,
            merged: merged::MergedLoader::new(
                storage.map(merged::load).unwrap_or_default(),
                automation.canned_pull_requests.is_some(),
            ),
            pull_requests_error: None,
            update_check: None,
            release_told: storage.and_then(updates::release_told),
            release_dialog: None,
            telemetry,
            system_theme: SystemTheme::watch(&cc.egui_ctx),
            window_theme: None,
            settings_viewport: viewport::Viewport::default(),
            settings_window_text_size: None,
            settings_window_size: None,
            window_icon: Arc::new(crate::icon::icon()),
            zoom_text: String::new(),
            automation,
            frame_limiter,
            #[cfg(target_os = "macos")]
            native_menu: Default::default(),
            #[cfg(target_os = "macos")]
            cli_install: None,
            #[cfg(target_os = "macos")]
            input_method: Default::default(),
        }
    }

    /// Lays out on a worker thread while the previous scene stays on screen.
    fn ensure_scene(&mut self, ctx: &egui::Context) {
        let Some(repo) = &self.repo else { return };
        let options = (self.settings.graph.clone(), self.settings.layout.clone());
        if self.requested_options.as_ref() != Some(&options) {
            self.requested_options = Some(options.clone());
            let font = FontId::monospace(FONT_SIZE);
            let text_height = ctx.fonts_mut(|f| f.row_height(&font));
            let input = ctx.fonts_mut(|f| {
                let mut width = |s: &str| {
                    f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE)
                        .size()
                        .x
                };
                let pull_requests = self.pull_requests.list().map(|p| &**p);
                let merged = self.merged.shown();
                Scene::prepare(
                    repo,
                    &self.settings,
                    pull_requests,
                    merged,
                    &mut width,
                    text_height,
                )
            });
            let rx = worker::spawn(ctx, move || input.lay_out());
            let pending = std::mem::take(&mut self.select_after_layout);
            self.job = Some(LayoutJob {
                rx,
                options,
                // A scene not yet placed on screen has no place to keep.
                view_anchor: self.view_anchor().filter(|_| !self.needs_initial_view),
                selected_commits: if pending.is_empty() {
                    self.selected_commits()
                } else {
                    pending
                },
                selected_edge: self.selected_edge_commits(),
            });
        }

        let Some(job) = &self.job else { return };
        let Ok(scene) = job.rx.try_recv() else {
            return;
        };
        let job = self.job.take().expect("job exists");
        // Moved nodes go along to a newer snapshot of the same graph, and to any relayout if
        // they are remembered: as they rest now, after any drop during the layout.
        let same_options = self.scene_options.as_ref() == Some(&job.options);
        let carried = if self.settings.remember_moves || same_options {
            self.carried_places()
        } else {
            None
        };
        crate::startup::mark("graph shown");
        self.scene = Some(scene);
        self.scene_options = Some(job.options);
        self.forget_pointer_state();
        let selected: Vec<usize> = job
            .selected_commits
            .iter()
            .filter_map(|oid| self.node_for(oid))
            .collect();
        self.selection.set(None);
        self.selection.extend(selected);
        self.selected_edge = job.selected_edge.and_then(|(c, p)| self.edge_for(&c, &p));
        self.update_search();
        self.restore_moves(carried);
        if let (Some((oid, screen)), Some(scene)) = (job.view_anchor, &self.scene)
            && let Some(node) = self.node_for(&oid)
            && self.canvas.is_positive()
        {
            let world = scene.node_center(node);
            let fraction = (screen - self.canvas.min) / self.canvas.size();
            self.view.show_at(self.canvas, world, fraction);
        }
    }

    fn forget_pointer_state(&mut self) {
        self.hovered = None;
        self.hovered_edge = None;
        self.context_node = None;
        self.drag = None;
        self.subtree_preview = None;
    }

    fn edge_for(&self, child: &Oid, parent: &Oid) -> Option<usize> {
        let (child, parent) = (self.node_for(child)?, self.node_for(parent)?);
        let edges = &self.scene.as_ref()?.graph.edges;
        edges
            .iter()
            .position(|e| e.child as usize == child && e.parent as usize == parent)
    }

    /// The commit's own node, or the one it is collapsed into.
    fn node_for(&self, oid: &Oid) -> Option<usize> {
        let scene = self.scene.as_ref()?;
        let commit = scene.repo.lookup(oid)?;
        scene.graph.represented_by(commit).map(|n| n as usize)
    }

    fn repo_key(&self) -> Option<String> {
        Some(self.repo.as_ref()?.path.display().to_string())
    }

    /// Puts the carried nodes back, and the remembered ones the scene before didn't show.
    fn restore_moves(&mut self, carried: Option<Carried>) {
        let remembered = || {
            let moves = self.moves.get(&self.repo_key()?)?;
            let places = moves.iter().map(|(hex, &(dx, dy, by_hand))| {
                let place = CarriedPlace {
                    offset: (dx, dy),
                    by_hand,
                    beside: None,
                };
                (hex.clone(), place)
            });
            Some(places.collect::<Carried>())
        };
        let remembered = self.settings.remember_moves.then(remembered).flatten();
        let places = match (carried, remembered) {
            (Some(carried), Some(mut remembered)) => {
                remembered.extend(carried);
                remembered
            }
            (carried, remembered) => {
                let Some(places) = carried.or(remembered) else {
                    return;
                };
                places
            }
        };
        let Some(scene) = &mut self.scene else { return };
        let node_of = |hex: &str| {
            Oid::from_hex(hex)
                .and_then(|oid| scene.repo.lookup(&oid))
                .and_then(|c| scene.graph.node_of(c))
                .map(|n| n as usize)
        };
        let point = |(x, y): (f32, f32)| parterre_core::layout::Point::new(x, y);
        let saved: Vec<RestPlace> = places
            .iter()
            .filter_map(|(hex, place)| {
                Some(RestPlace {
                    node: node_of(hex)?,
                    offset: point(place.offset),
                    by_hand: place.by_hand,
                    beside: place
                        .beside
                        .as_ref()
                        .and_then(|(parent, apart)| Some((node_of(parent)?, point(*apart)))),
                })
            })
            .collect();
        scene.net.restore(saved, &self.settings.net);
    }

    fn carried_places(&self) -> Option<Carried> {
        let scene = self.scene.as_ref()?;
        let hex = |node: usize| scene.node_oid(node).to_hex();
        let places = scene.net.rest_places().map(|p| {
            let place = CarriedPlace {
                offset: (p.offset.x, p.offset.y),
                by_hand: p.by_hand,
                beside: p.beside.map(|(parent, d)| (hex(parent), (d.x, d.y))),
            };
            (hex(p.node), place)
        });
        Some(places.collect())
    }

    /// As [`RememberedMoves`] keeps them.
    fn rest_offsets(&self) -> Option<HashMap<String, (f32, f32, bool)>> {
        let places = self.carried_places()?;
        let offsets = places
            .into_iter()
            .map(|(hex, p)| (hex, (p.offset.0, p.offset.1, p.by_hand)))
            .collect();
        Some(offsets)
    }

    fn record_moves(&mut self) {
        if self.settings.remember_moves {
            let offsets = self.rest_offsets();
            self.remember_offsets(offsets);
        }
    }

    /// For the repository shown.
    fn remember_offsets(&mut self, offsets: Option<HashMap<String, (f32, f32, bool)>>) {
        let (Some(offsets), Some(key)) = (offsets, self.repo_key()) else {
            return;
        };
        if offsets.is_empty() {
            self.moves.remove(&key);
        } else {
            self.moves.insert(key, offsets);
        }
    }

    /// The current node or the one nearest the view's centre, and where it is on screen.
    fn view_anchor(&self) -> Option<(Oid, Pos2)> {
        let scene = self.scene.as_ref()?;
        if !self.canvas.is_positive() {
            return None;
        }
        let node = self.selection.current().or_else(|| {
            let centre = self.view.to_world(self.canvas, self.canvas.center());
            (0..scene.node_count()).min_by(|&a, &b| {
                let da = scene.node_center(a).distance_sq(centre);
                let db = scene.node_center(b).distance_sq(centre);
                da.total_cmp(&db)
            })
        })?;
        Some((
            scene.node_oid(node),
            self.view.to_screen(self.canvas, scene.node_center(node)),
        ))
    }

    fn selected_commit(&self) -> Option<Oid> {
        let scene = self.scene.as_ref()?;
        Some(scene.node_oid(self.selection.current()?))
    }

    /// The current node's commit last.
    fn selected_commits(&self) -> Vec<Oid> {
        let Some(scene) = &self.scene else {
            return Vec::new();
        };
        self.selection
            .nodes
            .iter()
            .map(|&n| scene.node_oid(n))
            .collect()
    }

    /// Child and parent.
    fn selected_edge_commits(&self) -> Option<(Oid, Oid)> {
        let scene = self.scene.as_ref()?;
        let edge = scene.graph.edges[self.selected_edge?];
        let oid = |node: u32| scene.node_oid(node as usize);
        Some((oid(edge.child), oid(edge.parent)))
    }

    fn export_dialog(
        &mut self,
        format: Format,
        frame: &eframe::Frame,
    ) -> Option<rfd::AsyncFileDialog> {
        let (Some(repo), Some(_)) = (&self.repo, &self.scene) else {
            self.status = Some(Note::Error("Nothing to export yet".into()));
            return None;
        };
        let ext = format.extension();
        let kind = format.name();
        let mut dialog = rfd::AsyncFileDialog::new()
            .set_title(format!("Export the graph as {kind}"))
            .set_parent(frame)
            .add_filter(format!("{kind} image"), &[ext])
            .set_file_name(format!("{}.{ext}", repo.display_name()));
        if let Some(dir) = self.export_dir.as_deref().or_else(|| repo.path.parent()) {
            dialog = dialog.set_directory(dir);
        }
        Some(dialog)
    }

    /// Bitmaps are drawn at the current zoom, as in TortoiseGit, and with the display's pixels
    /// per point, so the graph looks as on screen.
    fn export(&mut self, format: Format, mut path: PathBuf, ctx: &egui::Context) {
        usage::action(usage::Action::Export);
        let Some(scene) = &self.scene else { return };
        if path.extension().is_none() || Format::from_path(&path) != Some(format) {
            let mut name = path.file_name().unwrap_or_default().to_owned();
            name.push(format!(".{}", format.extension()));
            path.set_file_name(name);
        }
        self.export_dir = path.parent().map(Path::to_owned);
        let zoom = match format {
            Format::Svg => 1.0,
            Format::Png | Format::WebP => self.view.zoom,
        };
        let palette = self.palette(ctx);
        // Not the text size's zoomed pixels per point: the graph's size leaves it out.
        let ppp = ctx.native_pixels_per_point().unwrap_or(1.0);
        self.status = Some(
            match export::write(&path, format, scene, &self.settings, &palette, zoom, ppp) {
                Ok(what) => Note::Info(format!("Saved {} ({what})", path.display())),
                Err(e) => Note::Error(format!("Could not save {}: {e}", path.display())),
            },
        );
    }

    fn open_folder(&mut self, dir: &Path) {
        usage::action(usage::Action::OpenRepository);
        match parterre_core::git::load_repo(dir) {
            Ok(repo) => self.show_opened(repo),
            Err(e) => {
                // A recent folder that is gone, or no longer a repository, leaves the list. One
                // git refuses for now, such as a bare repository, stays.
                if matches!(e, parterre_core::git::GitError::NotARepository(_)) {
                    self.recent.remove(dir);
                }
                self.status = Some(Note::Error(format!(
                    "Could not open {}: {e}",
                    dir.display()
                )));
            }
        }
    }

    fn close_folder(&mut self) {
        usage::action(usage::Action::CloseRepository);
        self.show_repo(None);
    }

    fn show_opened(&mut self, repo: Repo) {
        self.recent.add(&repo.path);
        self.telemetry.opened(&repo);
        self.show_repo(Some(repo));
    }

    /// Forgets everything about the repository shown but its settings.
    fn show_repo(&mut self, repo: Option<Repo>) {
        self.keep_repo_settings();
        let repo_settings = repo
            .as_ref()
            .map_or_else(RepoSettings::default, |repo| self.stored.settings_of(repo));
        repo_settings.apply(&mut self.settings.graph);
        self.repo = repo.map(Arc::new);
        self.scene = None;
        self.requested_options = None;
        self.job = None;
        self.needs_initial_view = true;
        self.forget_pointer_state();
        self.selection = Selection::default();
        self.selected_edge = None;
        self.select_after_layout.clear();
        self.search.hits.clear();
        self.search.current_hit = None;
        self.status = None;
        self.export_requested = None;
        self.details = Details::default();
        self.log.close();
        self.diffs.close_all();
        self.blames.close_all();
        self.compare.close();
        self.marked = None;
    }

    fn folder_dialog(&self, frame: &eframe::Frame) -> rfd::AsyncFileDialog {
        let mut dialog = rfd::AsyncFileDialog::new()
            .set_title("Open a git repository")
            .set_parent(frame);
        let near = self
            .repo
            .as_ref()
            .map(|r| r.path.as_path())
            .or_else(|| self.recent.iter().next());
        if let Some(dir) = near.and_then(Path::parent) {
            dialog = dialog.set_directory(dir);
        } else if cfg!(target_os = "macos")
            && let Some(home) = std::env::home_dir()
        {
            // macOS's panel would start in Documents, where repositories seldom are, and
            // its sidebar lacks the home folder (#338).
            dialog = dialog.set_directory(home);
        }
        dialog
    }

    /// One is open at a time: requests made meanwhile are dropped.
    fn file_dialogs(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let folder = std::mem::take(&mut self.folder_picker_requested);
        let export = self.export_requested.take();
        let settings_file = self.settings_file_requested.take();
        if let Some(pending) = &self.file_dialog {
            let Some(answer) = pending.answer() else {
                return;
            };
            let what = pending.what;
            self.file_dialog = None;
            match (what, answer) {
                (PickFor::Folder, Some(dir)) => self.open_folder(&dir),
                (PickFor::Export(format), Some(path)) => self.export(format, path, ctx),
                (PickFor::ExportSettings, Some(path)) => self.export_settings(path),
                (PickFor::ImportSettings, Some(path)) => self.import_settings(&path),
                (_, None) => {}
            }
            return;
        }
        if folder {
            let dialog = self.folder_dialog(frame).pick_folder();
            self.file_dialog = Some(Pending::start(PickFor::Folder, dialog, ctx));
        } else if let Some(format) = export
            && let Some(dialog) = self.export_dialog(format, frame)
        {
            let what = PickFor::Export(format);
            self.file_dialog = Some(Pending::start(what, dialog.save_file(), ctx));
        } else if let Some(what) = settings_file {
            let dialog = self.settings_dialog(what, frame);
            self.file_dialog = Some(match what {
                PickFor::ImportSettings => Pending::start(what, dialog.pick_file(), ctx),
                _ => Pending::start(what, dialog.save_file(), ctx),
            });
        }
    }

    fn opening_name(&self) -> Option<String> {
        self.opening
            .as_ref()
            .and_then(Opening::name)
            .or_else(|| self.repo.as_ref().map(|repo| repo.display_name()))
    }

    fn finish_opening(&mut self, ctx: &egui::Context) {
        let Some(opening) = &self.opening else {
            return;
        };
        let Some(result) = opening.poll() else {
            // Nothing else may wake the window meanwhile.
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
            return;
        };
        let opening = self.opening.take().expect("polled");
        // The user has opened something else meanwhile.
        if self.repo.is_some() {
            return;
        }
        match result {
            Ok(repo) => {
                let corrupt = self
                    .status
                    .as_ref()
                    .is_some_and(|note| note.text() == CORRUPT_SETTINGS_STATUS);
                self.show_opened(repo);
                if corrupt {
                    self.status = Some(Note::Error(CORRUPT_SETTINGS_STATUS.to_owned()));
                }
            }
            Err(e) if opening.named => {
                let message = format!("Could not open {}: {e}", opening.path.display());
                eprintln!("parterre: {message}");
                self.status = Some(Note::Error(message));
            }
            // The current directory, tried for want of a name, seldom is a repository.
            Err(_) => self.status = None,
        }
    }

    fn reload(&mut self) {
        let Some(path) = self.repo.as_ref().map(|r| r.path.clone()) else {
            return;
        };
        self.branches.look_at_once();
        match parterre_core::git::load_repo(&path) {
            Ok(repo) => self.show_reloaded(repo),
            Err(e) => self.status = Some(Note::Error(format!("Reload failed: {e}"))),
        }
    }

    fn show_reloaded(&mut self, repo: Repo) {
        self.install_reloaded(repo, "Reloaded");
        // Anything the watcher has loaded meanwhile may be older than this.
        self.watcher = None;
    }

    /// On a worker thread, finished by [`Self::finish_reload`]. Asked again meanwhile, the load
    /// running will do.
    fn reload_by_hand(&mut self, ctx: &egui::Context) {
        let Some(path) = self.repo.as_ref().map(|r| r.path.clone()) else {
            return;
        };
        if self.reloading.is_some() {
            return;
        }
        usage::action(usage::Action::Reload);
        let dir = path.clone();
        let rx = worker::spawn(ctx, move || {
            parterre_core::git::load_repo(&dir).map_err(|e| e.to_string())
        });
        self.reloading = Some((path, rx));
    }

    fn fetch_blocked(&self) -> Option<&'static str> {
        match &self.repo {
            None => Some("Open a repository first"),
            Some(_) => self.branches.fetch_blocked(),
        }
    }

    fn fetch(&mut self, ctx: &egui::Context, opener: egui::ViewportId) {
        if self.fetch_blocked().is_none() {
            let request = branches::Request::Run(parterre_core::branches::Action::Fetch);
            self.branches.request(ctx, request, opener);
        }
    }

    fn finish_reload(&mut self, ctx: &egui::Context) {
        if self.drag.is_some() {
            return;
        }
        let Some((path, rx)) = &self.reloading else {
            return;
        };
        let Some(result) = worker::received(rx, || Err("it stopped unexpectedly".into())) else {
            return;
        };
        let path = path.clone();
        self.reloading = None;
        // Another repository has been opened meanwhile.
        if self.repo.as_ref().is_none_or(|r| r.path != path) {
            return;
        }
        match result {
            Ok(repo) => {
                self.branches.look_at_once();
                self.show_reloaded(repo);
                // A graph that hasn't changed looks the same: say it was reloaded.
                self.branches.inform(ctx, path, "Reloaded");
            }
            Err(e) => self.status = Some(Note::Error(format!("Reload failed: {e}"))),
        }
    }

    /// A script's `open` step (`docs/automation.md`). `Ok(false)` while the branch tool's
    /// dialogs wait for its information.
    fn open_named(&mut self, ctx: &egui::Context, what: &str) -> Result<bool, String> {
        match what {
            "filter" | "zoom" | "drag" => egui::Popup::open_id(ctx, popup_id(what)),
            "about" => self.about.open(ctx),
            "new-release" => {
                self.release_dialog = Some(self.newer_release().ok_or("no --newer-release")?);
            }
            "shortcuts" => self.show_shortcuts = true,
            "legend" => self.show_legend = true,
            "first-run" => self.open_first_run_prompt(),
            "settings" => self.open_settings(self.settings_page),
            "clone" => self.clone_repository(ctx),
            "fetch" | "pull" => {
                let repo = self.repo.clone().ok_or("no repository is open")?;
                return self.open_branch_dialog(ctx, &repo, what, "");
            }
            _ => {
                let (kind, arg) = what
                    .split_once(':')
                    .ok_or("unknown; see docs/automation.md")?;
                if kind == "settings" {
                    let page = SettingsPage::named(arg).ok_or("no such settings page")?;
                    self.open_settings(page);
                    return Ok(true);
                }
                if kind == "menu" {
                    egui::Popup::open_id(ctx, menu_bar::bar::popup_id(arg));
                    return Ok(true);
                }
                if kind == "export-settings" || kind == "import-settings" {
                    self.open_settings(SettingsPage::Manage);
                    if kind == "export-settings" {
                        self.export_settings(PathBuf::from(arg));
                    } else {
                        self.import_settings(Path::new(arg));
                    }
                    return match &self.settings_note {
                        Some(Note::Error(note)) => Err(note.clone()),
                        _ => Ok(true),
                    };
                }
                let repo = self.repo.clone().ok_or("no repository is open")?;
                let oid = |name: &str| resolve_oid(&repo, name);
                match kind {
                    "log" => {
                        let commits = arg
                            .split("..")
                            .map(|name| repo.resolve(name).ok_or(format!("no commit named {name}")))
                            .collect::<Result<Vec<_>, _>>()?;
                        self.open_log(repo.clone(), &commits);
                    }
                    "compare" => match arg.split_once("..").ok_or("expected REF..REF")? {
                        (a, "WORKING_TREE") => {
                            self.compare_request(CompareRequest::WorkingTree(oid(a)?));
                        }
                        (a, b) => self.compare(oid(a)?, oid(b)?),
                    },
                    "diff" => self.open_named_diff(&repo, arg, ctx)?,
                    "blame" => self.open_named_blame(&repo, arg, ctx)?,
                    _ => return self.open_branch_dialog(ctx, &repo, kind, arg),
                }
            }
        }
        Ok(true)
    }

    /// The `open` steps of `docs/automation.md` that ask the branch tool.
    fn open_branch_dialog(
        &mut self,
        ctx: &egui::Context,
        repo: &Repo,
        kind: &str,
        name: &str,
    ) -> Result<bool, String> {
        let Some(catalog) = self.branches.catalog.clone() else {
            return Ok(false);
        };
        let oid = |name: &str| resolve_oid(repo, name);
        let request = match kind {
            "delete-worktree" => {
                let paths = name
                    .split(',')
                    .map(|name| {
                        catalog
                            .worktrees
                            .iter()
                            .find(|w| w.name() == name)
                            .map(|w| w.path.clone())
                            .ok_or_else(|| format!("no worktree named {name}"))
                    })
                    .collect::<Result<_, _>>()?;
                branches::Request::Run(parterre_core::branches::Action::DeleteWorktrees {
                    paths,
                    branches: false,
                })
            }
            "merge" => branches::Request::Merge {
                theirs: oid(name)?,
                target: name.to_owned(),
            },
            "merge-into" => {
                if !catalog.locals.iter().any(|b| b.name == name) {
                    return Err(format!("no local branch named {name}"));
                }
                branches::Request::MergeInto {
                    into: name.to_owned(),
                }
            }
            "cherry-pick" => branches::Request::CherryPick {
                picks: parterre_core::cherry_pick::Picks::Lacking(oid(name)?),
                name: Some(name.to_owned()),
            },
            "rebase" => branches::Request::Rebase {
                onto: oid(name)?,
                target: name.to_owned(),
            },
            "revert" => branches::Request::Revert { commit: oid(name)? },
            "reset" => {
                let (rev, mode) = match name.rsplit_once(':') {
                    Some((rev, mode)) => {
                        let mode = parterre_core::reset::Mode::ALL
                            .into_iter()
                            .find(|m| m.name().eq_ignore_ascii_case(mode))
                            .ok_or_else(|| format!("no reset mode {mode}"))?;
                        (rev, Some(mode))
                    }
                    None => (name, None),
                };
                branches::Request::Reset {
                    target: oid(rev)?,
                    mode,
                }
            }
            "add-worktree" => branches::Request::AddWorktree { start: oid(name)? },
            "fetch" => branches::Request::Run(parterre_core::branches::Action::Fetch),
            "pull" => {
                let head = catalog.head.ok_or("HEAD has no commit")?;
                let (_, pull) = remote::pull_at(&catalog, head)
                    .ok_or("the current branch has no upstream to pull")?;
                pull
            }
            "push" => {
                let (branch, remote) = match name.split_once(':') {
                    Some((branch, remote)) => (branch, Some(remote)),
                    None => (name, None),
                };
                let local = catalog
                    .locals
                    .iter()
                    .find(|b| b.name == branch)
                    .ok_or_else(|| format!("no local branch named {branch}"))?;
                // As `git push`: the upstream's remote, else the first.
                let remote = remote
                    .or(local.upstream_remote.as_ref().map(|(r, _)| r.as_str()))
                    .or(catalog.remote_names.iter().min().map(String::as_str))
                    .ok_or("the repository has no remote")?;
                branches::Request::Run(parterre_core::branches::Action::Push(Box::new(
                    parterre_core::remote::Push::new(local, remote),
                )))
            }
            "delete-remote-branch" => {
                let branches = name
                    .split(',')
                    .map(|name| {
                        let tip = catalog
                            .remotes
                            .iter()
                            .find(|r| r.name == name)
                            .ok_or_else(|| format!("no remote-tracking branch named {name}"))?
                            .tip;
                        let (remote, branch) = catalog
                            .tracking_parts(name)
                            .ok_or_else(|| format!("no remote in {name}"))?;
                        Ok(parterre_core::remote::RemoteBranchTip {
                            remote: remote.to_owned(),
                            branch: branch.to_owned(),
                            tip,
                        })
                    })
                    .collect::<Result<_, String>>()?;
                branches::Request::Run(parterre_core::branches::Action::DeleteRemoteBranches(
                    branches,
                ))
            }
            "set-upstream" => {
                if !catalog.locals.iter().any(|b| b.name == name) {
                    return Err(format!("no local branch named {name}"));
                }
                branches::Request::SetUpstream {
                    branch: name.to_owned(),
                }
            }
            "create-branch" => branches::Request::Create {
                start: oid(name)?,
                track: None,
                switch: false,
            },
            _ => return Err("unknown; see docs/automation.md".into()),
        };
        self.branches.request(ctx, request, egui::ViewportId::ROOT);
        Ok(true)
    }

    /// The same history: the layout, the view, moved nodes and the other windows stay.
    fn go_to_worktree(&mut self, path: &Path) {
        usage::action(usage::Action::GoToWorktree);
        match parterre_core::git::load_repo(path) {
            Ok(repo) => {
                self.record_moves();
                let offsets = self.rest_offsets();
                self.recent.add(&repo.path);
                let name = repo.display_name();
                self.install_reloaded(repo, &format!("Went to worktree {name}"));
                // Moved nodes are remembered by folder: they go along.
                if self.settings.remember_moves {
                    self.remember_offsets(offsets);
                }
                // The watcher follows the open worktree's own git folder.
                self.watcher = None;
            }
            Err(e) => {
                self.status = Some(Note::Error(format!(
                    "Could not go to {}: {e}",
                    path.display()
                )));
            }
        }
    }

    /// A newer snapshot of the same repository.
    fn install_reloaded(&mut self, repo: Repo, status: &str) {
        self.select_after_layout = self.selected_commits();
        let origin_moved = self
            .repo
            .as_ref()
            .is_none_or(|shown| parterre_forge::origin_moved(shown, &repo));
        let repo = Arc::new(repo);
        self.pull_requests.refs_changed(origin_moved);
        self.log.reload(&repo);
        self.compare.reload(&repo);
        self.repo = Some(repo);
        if let Some((oid, _)) = self.marked {
            self.marked = Some((oid, self.commit_label(oid)));
        }
        self.requested_options = None;
        self.status = Some(Note::Info(status.into()));
    }

    fn auto_reload(&mut self, ctx: &egui::Context) {
        let Some(shown) = self.repo.as_ref().filter(|_| self.settings.auto_reload) else {
            self.watcher = None;
            return;
        };
        if self.watcher.as_ref().is_none_or(|w| w.path() != shown.path) {
            self.watcher = Some(auto_reload::Watcher::start(&shown.path, ctx));
        }
        if self.drag.is_some() {
            return;
        }
        let Some(repo) = self.watcher.as_ref().and_then(auto_reload::Watcher::take) else {
            return;
        };
        if !shown.same_refs(&repo) {
            self.install_reloaded(repo, "Reloaded: the refs changed");
        }
    }

    fn update_pull_requests(&mut self, ctx: &egui::Context) {
        let loader = &mut self.pull_requests;
        loader.follow(self.repo.as_ref().map(|r| r.path.as_path()));
        // Turned on in the settings or the menu: the toolbar asks by itself.
        let on = self.settings.graph.show_pull_requests;
        if on && !self.pull_requests_were_on {
            loader.ask();
        }
        self.pull_requests_were_on = on;
        let wanted = on && loader.origin().is_some();
        match loader.update(wanted, ctx) {
            Some(pull_requests::Loaded::Found { count, asked }) => {
                // Those whose head isn't here (another fork's, or pushed since the last fetch)
                // can't be shown.
                let here = match (&self.repo, loader.list()) {
                    (Some(repo), Some(list)) => list.heads(repo).len(),
                    _ => 0,
                };
                if asked && let Some(origin) = loader.origin() {
                    let status = pull_requests::loaded_status(origin.forge(), count, here);
                    self.status = Some(Note::Info(status));
                }
                self.requested_options = None;
            }
            // Canned for a screenshot: a mistake in the file fails the run.
            Some(pull_requests::Loaded::Failed { error, .. }) if loader.is_canned() => {
                eprintln!("parterre: {error}");
                crate::automation::fail();
            }
            // Loads parterre makes by itself fail quietly: the button's tooltip says why.
            Some(pull_requests::Loaded::Failed { error, asked: true }) => {
                self.pull_requests_error = Some(error);
            }
            Some(pull_requests::Loaded::Failed { asked: false, .. }) | None => {}
        }
    }

    pub(super) fn pull_requests_active(&self) -> bool {
        self.settings.graph.show_pull_requests
            && self.pull_requests.origin().is_some()
            && !self.pull_requests.needs_sign_in()
    }

    pub(super) fn toggle_pull_requests(&mut self) {
        if self.pull_requests_active() {
            self.settings.graph.show_pull_requests = false;
        } else {
            self.settings.graph.show_pull_requests = true;
            self.pull_requests.ask();
        }
    }

    /// The repositories opened before name the Azure DevOps organisations the dialog lists.
    pub(super) fn clone_repository(&mut self, ctx: &egui::Context) {
        let known = self.recent.iter().map(Path::to_owned).collect();
        self.branches
            .clone_repository(ctx, egui::ViewportId::ROOT, known);
    }

    fn pull_requests_dialog(&mut self, ctx: &egui::Context) {
        let Some(error) = &self.pull_requests_error else {
            return;
        };
        match pull_requests::dialog(ctx, error) {
            pull_requests::DialogAnswer::Open => {}
            pull_requests::DialogAnswer::Close => self.pull_requests_error = None,
            pull_requests::DialogAnswer::Install => {
                if let Err(e) = crate::browser::open(GH_INSTALL_URL) {
                    self.status = Some(Note::Error(e));
                }
            }
        }
    }

    fn update_search(&mut self) {
        self.search.hits.clear();
        self.search.current_hit = None;
        let query = self.search.query.trim().to_lowercase();
        let Some(scene) = &self.scene else { return };
        if query.is_empty() {
            return;
        }
        for (i, node) in scene.graph.nodes.iter().enumerate() {
            let commit = scene.repo.commit(node.commit);
            let matches = commit.oid.to_hex().starts_with(&query)
                || node
                    .refs
                    .iter()
                    .any(|&r| scene.repo.refs[r].name.to_lowercase().contains(&query))
                || commit.subject.to_lowercase().contains(&query)
                || commit.author_name.to_lowercase().contains(&query);
            if matches {
                self.search.hits.push(i);
            }
        }
    }

    fn find_next(&mut self) {
        self.go_to_search_hit(true);
    }

    fn find_previous(&mut self) {
        self.go_to_search_hit(false);
    }

    fn go_to_search_hit(&mut self, forward: bool) {
        let n = self.search.hits.len();
        if n == 0 {
            return;
        }
        let next = match self.search.current_hit {
            None => 0,
            Some(c) if forward => (c + 1) % n,
            Some(c) => (c + n - 1) % n,
        };
        self.search.current_hit = Some(next);
        let node = self.search.hits[next];
        self.selection.set(Some(node));
        self.center_on(node);
    }

    fn center_on(&mut self, node: usize) {
        if let Some(scene) = &self.scene {
            self.view
                .show_at(self.canvas, scene.node_center(node), vec2(0.5, 0.4));
        }
    }

    fn go_to_head(&mut self) {
        usage::action(usage::Action::GoToHead);
        self.scroll_to_head();
    }

    fn scroll_to_head(&mut self) {
        let Some((scene, head)) = self.scene.as_ref().and_then(|s| Some((s, s.head_node()?)))
        else {
            self.fit();
            return;
        };
        self.view.zoom = self.view.zoom.max(HEAD_MIN_ZOOM);
        // As TortoiseGit scrolls HEAD to the top of the window after loading.
        let fraction = match self.settings.layout.direction {
            Direction::NewestTop => vec2(0.5, 0.12),
            Direction::NewestBottom => vec2(0.5, 0.88),
            Direction::NewestLeft => vec2(0.12, 0.5),
            Direction::NewestRight => vec2(0.88, 0.5),
        };
        self.view
            .show_at(self.canvas, scene.node_center(head), fraction);
    }

    fn zoom_to_fit(&mut self) {
        usage::action(usage::Action::Fit);
        self.fit();
    }

    fn fit(&mut self) {
        if let Some(scene) = &self.scene {
            self.view.fit(self.canvas, scene.bounds(), 1.0);
        }
    }

    /// Remembers the result. Not while nodes are being dragged.
    fn rearrange(&mut self, change: impl FnOnce(&mut parterre_core::physics::Net)) {
        if matches!(self.drag, Some(Drag::Node { .. })) {
            return;
        }
        if let Some(scene) = &mut self.scene {
            change(&mut scene.net);
        }
        self.record_moves();
    }

    fn reset_positions(&mut self) {
        usage::action(usage::Action::ResetPositions);
        self.rearrange(|net| net.reset());
    }

    fn undo(&mut self) {
        usage::action(usage::Action::Undo);
        self.rearrange(|net| {
            net.undo();
        });
    }

    fn redo(&mut self) {
        usage::action(usage::Action::Redo);
        self.rearrange(|net| {
            net.redo();
        });
    }

    fn return_to_layout(&mut self, nodes: &[usize]) {
        self.rearrange(|net| net.return_to_layout(nodes));
    }

    fn select_subtree(&mut self, roots: &[usize]) {
        if let Some(scene) = &self.scene {
            self.selection.extend(scene.graph.subtree(roots));
        }
    }

    fn set_drag_model(&mut self, model: DragModel) {
        self.settings.net.model = model;
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // Even while a text field has the keyboard.
        let (reload, fetch) = ctx.input_mut(keys::reload_and_fetch);
        if reload {
            self.reload_by_hand(ctx);
        }
        if fetch {
            self.fetch(ctx, egui::ViewportId::ROOT);
        }
        if ctx.egui_wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("search")));
            }
            return;
        }
        let pressed = |s: keys::Shortcut| ctx.input_mut(|i| s.consume(i));
        if pressed(keys::OPEN) {
            self.folder_picker_requested = true;
        }
        if pressed(keys::CLOSE) {
            self.close_folder();
        }
        // The Shift-variants first: egui takes ⌘G for ⇧⌘G too.
        if pressed(keys::find_previous()) {
            self.find_previous();
        }
        if pressed(keys::find_next()) {
            self.find_next();
        }
        if pressed(keys::FIND) {
            self.search.request_focus = true;
        }
        if pressed(keys::SETTINGS) {
            self.open_settings(self.settings_page);
        }
        // File › Quit on Linux; macOS's menu bar takes ⌘Q, Windows has Alt+F4.
        if keys::Platform::CURRENT == keys::Platform::Linux && pressed(keys::QUIT) {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
        }
        if pressed(keys::COPY) {
            self.copy_selected_hash(ctx);
        }
        // Most specific first: Ctrl+Z also matches Ctrl+Shift+Z.
        if pressed(keys::redo()) {
            self.redo();
        }
        if pressed(keys::UNDO) {
            self.undo();
        }
        for (shortcut, model) in keys::DRAG.into_iter().zip(DragModel::ALL) {
            if pressed(shortcut) {
                self.set_drag_model(model);
            }
        }
        if pressed(keys::ZOOM_IN) || pressed(keys::ZOOM_IN_UNSHIFTED) {
            self.zoom_in();
        }
        if pressed(keys::ZOOM_OUT) {
            self.zoom_out();
        }
        if pressed(keys::ACTUAL_SIZE) {
            self.zoom_to_actual_size();
        }
        if pressed(keys::SHOW_LOG) {
            self.show_log_of_selection();
        }
        if pressed(keys::ZOOM_TO_FIT) {
            self.zoom_to_fit();
        }
        if pressed(keys::GO_TO_HEAD) {
            self.go_to_head();
        }
        if pressed(keys::Shortcut::plain(Key::Escape)) {
            self.selection.set(None);
            self.selected_edge = None;
        }
        if pressed(keys::CONTEXT_MENU) {
            self.context_menu_requested = true;
        }
        let step = ARROW_KEY_PAN;
        let page = self.canvas.height() * 0.8;
        let pan = [
            (Key::ArrowLeft, vec2(step, 0.0)),
            (Key::ArrowRight, vec2(-step, 0.0)),
            (Key::ArrowUp, vec2(0.0, step)),
            (Key::ArrowDown, vec2(0.0, -step)),
            (Key::PageUp, vec2(0.0, page)),
            (Key::PageDown, vec2(0.0, -page)),
        ];
        for (key, delta) in pan {
            if pressed(keys::Shortcut::plain(key)) {
                self.view.pan_screen(delta);
            }
        }
    }

    fn copy_selected_hash(&self, ctx: &egui::Context) {
        if let Some(oid) = self.selected_commit() {
            usage::action(usage::Action::Copy);
            ctx.copy_text(oid.to_hex());
        }
    }

    fn show_log_of_selection(&mut self) {
        let nodes = self.selection.nodes.clone();
        self.show_log(&nodes);
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if self.job.is_some() {
                ui.spinner();
                ui.label("Laying out…");
                ui.separator();
            }
            if self.reloading.is_some() {
                ui.spinner();
                ui.label("Reloading…");
                ui.separator();
            }
            if self.pull_requests.is_loading() {
                ui.spinner();
                ui.label("Loading pull requests…");
                ui.separator();
            }
            if let Some(scene) = &self.scene {
                if self.settings.graph.show_upstreams {
                    let selected = self.selection.current();
                    crate::upstreams::status_ui(ui, scene, self.hovered, selected);
                }
                if self.selection.len() > 1 {
                    ui.label(format!("{} nodes selected ·", self.selection.len()));
                }
                if let Some(sel) = self.selection.current() {
                    let commit = scene.repo.commit(scene.graph.nodes[sel].commit);
                    ui.monospace(commit.oid.short(scene.repo.abbrev_len));
                    ui.label(format!(
                        "{} — {}, {}",
                        commit.subject, commit.author_name, commit.author_date
                    ));
                } else if let Some(e) = self.selected_edge {
                    let edge = scene.graph.edges[e];
                    let hidden = match edge.hidden {
                        0 => String::new(),
                        n => format!(", {n} commits collapsed"),
                    };
                    ui.label(format!("{}{hidden}", edge_summary(scene, edge)));
                } else {
                    ui.label(scene.repo.path.display().to_string());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{:.0}%", self.view.zoom * 100.0));
                    ui.separator();
                    let hidden = match scene.graph.hidden_branches {
                        0 => String::new(),
                        1 => " · 1 branch hidden".to_owned(),
                        n => format!(" · {n} branches hidden"),
                    };
                    ui.label(format!(
                        "{} nodes · {} commits{hidden}",
                        scene.node_count(),
                        scene.graph.visible_commits,
                    ));
                    if let Some(note) = &self.status {
                        ui.separator();
                        status_label(ui, note);
                    }
                });
            } else if let Some(note) = &self.status {
                status_label(ui, note);
            }
        });
    }

    fn welcome(&mut self, ui: &mut Ui) {
        // Centred by last frame's height; the first frame is measured, then drawn again.
        let id = egui::Id::new("welcome-height");
        let height: Option<f32> = ui.data(|d| d.get_temp(id));
        if height.is_none() {
            ui.ctx().request_discard("measuring the welcome text");
        }
        ui.add_space(((ui.available_height() - height.unwrap_or(0.0)) / 2.0).max(0.0));
        let start = ui.cursor().top();
        let mut open = None;
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("No repository open").size(20.0).strong());
            ui.add_space(6.0);
            ui.label(
                RichText::new("Select a git repository to see its revision graph.")
                    .color(ui.visuals().weak_text_color()),
            );
            ui.add_space(16.0);
            let button_size = vec2(140.0, 30.0);
            let width = 2.0 * button_size.x + ui.spacing().item_spacing.x;
            ui.allocate_ui(vec2(width, button_size.y), |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new("Open folder…").min_size(button_size))
                        .on_hover_text(format!(
                            "Any folder inside the repository ({})",
                            keys::OPEN.label()
                        ))
                        .clicked()
                    {
                        self.folder_picker_requested = true;
                    }
                    if ui
                        .add(egui::Button::new("Clone repository…").min_size(button_size))
                        .on_hover_text("From GitHub, Azure DevOps or any URL git takes")
                        .clicked()
                    {
                        self.clone_repository(ui.ctx());
                    }
                });
            });
            if let Some(note @ Note::Error(_)) = &self.status {
                ui.add_space(10.0);
                status_label(ui, note);
            }
            if !self.recent.is_empty() {
                ui.add_space(24.0);
                ui.label(RichText::new("Recent").color(ui.visuals().weak_text_color()));
                ui.add_space(2.0);
                let (link, weak) = (ui.visuals().hyperlink_color, ui.visuals().weak_text_color());
                for path in self.recent.iter().take(WELCOME_RECENT_COUNT) {
                    let (name, place) = name_and_place(path);
                    let mut text = egui::text::LayoutJob::default();
                    let font = egui::TextStyle::Body.resolve(ui.style());
                    text.append(&name, 0.0, egui::TextFormat::simple(font.clone(), link));
                    text.append(&place, 10.0, egui::TextFormat::simple(font, weak));
                    let response = ui.add(egui::Button::new(text).frame(false));
                    if response
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        open = Some(path.to_owned());
                    }
                }
            }
        });
        let measured = ui.cursor().top() - start;
        if height != Some(measured) {
            ui.data_mut(|d| d.insert_temp(id, measured));
            if height.is_some_and(|h| (h - measured).abs() > 0.5) {
                ui.ctx().request_repaint();
            }
        }
        if let Some(path) = open {
            self.open_folder(&path);
        }
    }

    fn canvas(&mut self, ui: &mut Ui) {
        let (canvas, response) =
            ui.allocate_exact_size(ui.available_size(), crate::focus::pointer_only());
        self.canvas = canvas;
        // The overview map, drawn over the graph, counts as the graph.
        self.graph_hovered = ui.rect_contains_pointer(canvas);
        if self.needs_initial_view && canvas.is_positive() && self.scene.is_some() {
            self.needs_initial_view = false;
            if self.automation.start_fitted {
                self.fit();
            } else {
                self.view.zoom = 1.0;
                self.scroll_to_head();
            }
        }
        let Some(scene) = &mut self.scene else { return };

        let pointer = response.hover_pos();
        self.hovered = pointer.and_then(|p| scene.node_at(self.view.to_world(canvas, p)));
        let hovered_pull_request = match (pointer, self.drag) {
            (Some(p), None) => scene.pull_request_at(self.view.to_world(canvas, p)),
            _ => None,
        };
        if hovered_pull_request.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let view = self.view;
        self.hovered_edge = match (pointer, self.hovered, self.drag) {
            (Some(p), None, None) => render::edge_at(
                scene,
                self.settings.edge_style,
                |w| view.to_screen(canvas, w),
                p,
                view.fixed(5.0),
            ),
            _ => None,
        };

        let modifiers = ui.input(|i| i.modifiers);
        let extend = modifiers.shift || modifiers.command;
        if response.drag_started() {
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or_default();
            let world = self.view.to_world(canvas, origin);
            let node = scene.node_at(world);
            let middle = response.dragged_by(PointerButton::Middle);
            self.drag = match node {
                Some(n) if !middle => {
                    if !self.selection.contains(n) {
                        if extend {
                            self.selection.add(n);
                        } else {
                            self.selection.set(Some(n));
                        }
                    }
                    let model = self.settings.net.model;
                    let nodes = self.selection.group_of(n);
                    let carried = scene.carried_nodes(&nodes, model);
                    scene.net.grab(n, &nodes, &carried, model);
                    Some(Drag::Node {
                        grab_offset: world - scene.node_center(n),
                    })
                }
                None if extend && !middle => Some(Drag::Select {
                    start: world,
                    end: world,
                }),
                _ => Some(Drag::Pan),
            };
        }
        if response.dragged() {
            let pointer = response
                .interact_pointer_pos()
                .map(|p| self.view.to_world(canvas, p));
            match &mut self.drag {
                Some(Drag::Node { grab_offset }) => {
                    if let Some(p) = pointer {
                        scene.net.drag_to(to_point(p - *grab_offset));
                    }
                }
                Some(Drag::Select { end, .. }) => *end = pointer.unwrap_or(*end),
                Some(Drag::Pan) | None => self.view.pan_screen(response.drag_delta()),
            }
        }
        let band = match self.drag {
            Some(Drag::Select { start, end }) => Some(Rect::from_two_pos(start, end)),
            _ => None,
        };
        let mut moved = false;
        if response.drag_stopped() {
            match self.drag {
                Some(Drag::Node { .. }) => {
                    scene.net.release(&self.settings.net);
                    moved = true;
                    usage::action(usage::Action::DragNode);
                }
                Some(Drag::Select { start, end }) => {
                    self.selection
                        .extend(scene.nodes_in(Rect::from_two_pos(start, end)));
                }
                Some(Drag::Pan) | None => {}
            }
            self.drag = None;
        }

        let mut action = None;
        if response.clicked() {
            // Once for a double-click.
            if let Some(i) = hovered_pull_request
                && !extend
                && !response.double_clicked()
            {
                action = Some(MenuAction::OpenPullRequest(
                    scene.pull_requests[i].url.clone(),
                ));
            }
            match self.hovered {
                Some(n) if modifiers.command => self.selection.toggle(n),
                Some(n) if modifiers.shift => self.selection.add(n),
                Some(n) => self.selection.set(Some(n)),
                None if extend => {}
                None => self.selection.set(None),
            }
            self.selected_edge = match (self.hovered, self.hovered_edge) {
                (None, Some(e)) if self.selected_edge != Some(e) => Some(e),
                _ => None,
            };
        }
        if response.secondary_clicked() {
            self.context_node = self.hovered;
            if let Some(n) = self.hovered
                && !self.selection.contains(n)
            {
                self.selection.set(Some(n));
            }
        }
        if response.double_clicked() {
            match self.hovered {
                Some(n) => {
                    self.selection.set(Some(n));
                    if hovered_pull_request.is_none() {
                        action = Some(MenuAction::ShowLog(vec![n]));
                    }
                }
                None => self.view.fit(canvas, scene.bounds(), 1.0),
            }
        }

        if response.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
            if zoom != 1.0 {
                self.view
                    .zoom_around(canvas, pointer.unwrap_or(canvas.center()), zoom);
            } else if scroll != Vec2::ZERO {
                self.view.pan_screen(scroll);
            }
        }
        // However the view moved this frame, before drawing: drawn beyond the limit and pulled
        // back on the next frame, the graph would jitter.
        self.view
            .keep_in_sight(canvas, scene.bounds(), self.view_at_frame_start);

        let dt = self
            .automation
            .fixed_dt()
            .unwrap_or_else(|| ui.input(|i| i.stable_dt));
        if scene.net.step(dt, &self.settings.net) {
            ui.ctx().request_repaint();
        }

        let palette = palette_for(ui, &self.settings);
        let node_count = scene.node_count();
        let mut hits = vec![false; node_count];
        for &h in &self.search.hits {
            hits[h] = true;
        }
        let mut selected = self.selection.mask(node_count);
        if let Some(band) = band {
            for node in scene.nodes_in(band) {
                selected[node] = true;
            }
        }
        let mut preview = vec![false; node_count];
        match (self.hovered, self.drag, self.settings.net.model) {
            (Some(n), None, DragModel::Subtree) => {
                // As a drag would: Shift or Ctrl adds the node to the selection.
                let mut roots = self.selection.group_of(n);
                if extend && !self.selection.contains(n) {
                    roots = self.selection.nodes.clone();
                    roots.push(n);
                }
                if self
                    .subtree_preview
                    .as_ref()
                    .is_none_or(|(r, _)| *r != roots)
                {
                    let nodes = scene.carried_nodes(&roots, DragModel::Subtree);
                    self.subtree_preview = Some((roots, nodes));
                }
                if let Some((_, nodes)) = &self.subtree_preview {
                    for &node in nodes {
                        preview[node] = true;
                    }
                }
            }
            _ => self.subtree_preview = None,
        }
        let marked = self.marked.as_ref().and_then(|(oid, _)| {
            let commit = scene.repo.lookup(oid)?;
            scene.graph.node_of(commit).map(|n| n as usize)
        });
        let marks = Marks {
            marked,
            hovered: self.hovered,
            hovered_pull_request,
            hovered_edge: self.hovered_edge,
            selected,
            preview,
            selected_edge: self.selected_edge,
            search_hits: hits,
            show_upstreams: self.settings.graph.show_upstreams,
            rebasing: rebasing_marks(&scene.repo, self.branches.catalog.as_deref()),
            merging: merging_marks(&scene.repo, self.branches.catalog.as_deref()),
        };
        let painter = ui.painter_at(canvas);
        render::paint_scene(
            &painter,
            canvas,
            &self.view,
            scene,
            &palette,
            &self.settings,
            &marks,
        );

        if let Some(band) = band {
            painter.rect(
                self.view.rect_to_screen(canvas, band),
                0.0,
                palette.selection.gamma_multiply(0.12),
                egui::Stroke::new(1.0, palette.selection),
                egui::StrokeKind::Inside,
            );
        }

        if scene.node_count() == 0 {
            painter.text(
                canvas.center(),
                egui::Align2::CENTER_CENTER,
                "No graph available",
                FontId::proportional(16.0),
                palette.edge,
            );
        }

        if let Some(i) = hovered_pull_request {
            let pull_request = &scene.pull_requests[i];
            let forge = self
                .pull_requests
                .origin()
                .map_or(parterre_forge::Forge::Github, parterre_forge::Origin::forge);
            response
                .clone()
                .on_hover_ui_at_pointer(|ui| pull_request_tooltip(ui, pull_request, forge));
        } else if let (Some(node), None) = (self.hovered, self.drag) {
            let details = &mut self.details;
            response
                .clone()
                .on_hover_ui_at_pointer(|ui| node_tooltip(ui, scene, node, details));
        }
        if let (Some(edge), None) = (self.hovered_edge, self.drag) {
            response
                .clone()
                .on_hover_ui_at_pointer(|ui| edge_tooltip(ui, scene, edge));
        }

        // The key opens the selected node's menu at the node, else the graph's in the middle.
        let key_pressed_at = std::mem::take(&mut self.context_menu_requested).then(|| {
            self.context_node = self.selection.current();
            match (self.context_node, &self.scene) {
                (Some(node), Some(scene)) => self.view.to_screen(canvas, scene.node_center(node)),
                _ => canvas.center(),
            }
        });
        let popup = menu::context_menu(&response, key_pressed_at);
        // Built only while open: it asks the Git menu.
        let open = key_pressed_at.is_some()
            || response.secondary_clicked()
            || egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
        let menu = match self.context_node {
            _ if !open => Vec::new(),
            Some(node) => self.node_menu(node),
            None => self.canvas_menu(),
        };
        let kind = match self.context_node {
            Some(_) => usage::Menu::Node,
            None => usage::Menu::Canvas,
        };
        let mut chosen = None;
        popup.style(menu::style).show(|ui| {
            menu::fit_window(ui, |ui| {
                ui.set_min_width(menu::MIN_WIDTH);
                usage::menu(ui.ctx(), kind);
                chosen = menu_bar::bar::context(ui, &menu);
            });
        });
        if let Some(command) = chosen {
            self.run_command(&response.ctx, command);
        }
        match action {
            Some(MenuAction::ShowLog(nodes)) => self.show_log(&nodes),
            Some(MenuAction::OpenPullRequest(url)) => self.open_pull_request(&url),
            None => {}
        }

        if moved {
            self.record_moves();
        }
        if self.settings.show_overview {
            self.overview(ui, canvas);
        }
    }

    fn overview(&mut self, ui: &mut Ui, canvas: Rect) {
        let Some(scene) = &self.scene else { return };
        // As TortoiseGit's.
        let size = vec2(
            (canvas.width() / 4.0).max(100.0),
            (canvas.height() / 4.0).max(200.0),
        );
        let rect = Rect::from_min_size(canvas.max - size - vec2(12.0, 12.0), size);
        let palette = palette_for(ui, &self.settings);
        let painter = ui.painter_at(rect.expand(2.0));
        let (world, scale) = render::paint_overview(
            &painter,
            rect,
            canvas,
            &self.view,
            scene,
            &palette,
            self.selected_edge,
        );
        let resp = ui.interact(
            rect,
            egui::Id::new("overview"),
            crate::focus::pointer_only(),
        );
        if (resp.clicked() || resp.dragged())
            && let Some(p) = resp.interact_pointer_pos()
        {
            let target = world.center() + (p - rect.center()) / scale;
            self.view.show_at(canvas, target, vec2(0.5, 0.5));
        }
    }

    fn legend_window(&mut self, ctx: &egui::Context) {
        if !self.show_legend {
            return;
        }
        let palette = self.palette(ctx);
        let mut open_colours = false;
        let rules = &self.settings.branch_colors;
        let shown = crate::dialogs::Dialog::new("legend", "Legend")
            .screen(crate::usage::Screen::Legend)
            .width(520.0)
            .resizable()
            .show(ctx, |ui| {
                open_colours = crate::dialogs::fields(ui, |ui| legend(ui, &palette, rules));
                ui.separator();
                crate::dialogs::close_only(ui)
            });
        if shown.answer() == crate::dialogs::Answer::Cancel {
            self.show_legend = false;
        }
        if open_colours {
            self.open_settings(SettingsPage::BranchColours);
        }
    }

    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        if !self.show_shortcuts {
            return;
        }
        let shown = crate::dialogs::Dialog::new("shortcuts", "Keyboard and mouse")
            .screen(crate::usage::Screen::Shortcuts)
            .width(760.0)
            .resizable()
            .show(ctx, |ui| {
                crate::dialogs::fields(ui, shortcuts);
                ui.separator();
                crate::dialogs::close_only(ui)
            });
        if shown.answer() == crate::dialogs::Answer::Cancel {
            self.show_shortcuts = false;
        }
    }
}

fn pull_request_tooltip(
    ui: &mut Ui,
    pull_request: &parterre_forge::PullRequest,
    forge: parterre_forge::Forge,
) {
    let pr = pull_request;
    ui.label(RichText::new(format!("#{} {}", pr.number, pr.title)).strong());
    // The author is missing if their account was deleted.
    let draft = pr.draft.then(|| "Draft".to_owned());
    let author = (!pr.author.is_empty()).then(|| format!("by {}", pr.author));
    let about: Vec<String> = draft.into_iter().chain(author).collect();
    if !about.is_empty() {
        ui.label(about.join(" · "));
    }
    ui.label(RichText::new(format!("{} into {}", pr.head_label(), pr.base_branch)).monospace());
    ui.add_space(4.0);
    ui.label(RichText::new(format!("Click to open it on {forge}")).weak());
}

fn node_tooltip(ui: &mut Ui, scene: &Scene, node: usize, details: &mut Details) {
    let n = &scene.graph.nodes[node];
    let commit = scene.repo.commit(n.commit);
    ui.monospace(commit.oid.to_hex());
    ui.label(format!(
        "{} <{}>  {}",
        commit.author_name, commit.author_email, commit.author_date
    ));
    ui.add_space(4.0);
    ui.label(RichText::new(&commit.subject).strong());
    match details.message(&scene.repo.path, commit.oid, ui.ctx()) {
        Some(message) => {
            let body = message.split_once('\n').map_or("", |(_, b)| b.trim());
            if !body.is_empty() {
                let body = body
                    .lines()
                    .take(MESSAGE_TIP_MAX_LINES)
                    .collect::<Vec<_>>()
                    .join("\n");
                ui.label(body.chars().take(MESSAGE_TIP_MAX_CHARS).collect::<String>());
            }
        }
        None => {
            ui.spinner();
        }
    }
    let refs: Vec<&str> = n
        .refs
        .iter()
        .map(|&r| scene.repo.refs[r].full_name.as_str())
        .collect();
    if !refs.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new(refs.join("\n")).weak());
    }
    let merged: Vec<String> = n
        .refs
        .iter()
        .filter_map(|&r| {
            let m = scene.merged.of(&scene.repo, r)?;
            let name = &scene.repo.refs[r].name;
            Some(format!("{name} was {}", m.describe(&scene.repo)))
        })
        .collect();
    if !merged.is_empty() {
        ui.add_space(4.0);
        ui.label(merged.join("\n"));
    }
    let worktrees: Vec<String> = scene
        .worktrees_on(node)
        .into_iter()
        .map(|k| {
            let w = &scene.repo.worktrees[k];
            let state = if w.missing {
                " (the folder is gone)"
            } else if w.locked {
                " (locked)"
            } else {
                ""
            };
            format!("Worktree {}{state}", w.path.display())
        })
        .collect();
    if !worktrees.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new(worktrees.join("\n")).weak());
    }
    // The node's first edge: its first-parent one, or where the forks mode hangs a branch off
    // its latest merge of its base, the one holding its own commits.
    let hidden = scene
        .graph
        .edges
        .iter()
        .find(|e| e.child as usize == node)
        .map_or(0, |e| e.hidden);
    if hidden > 0 {
        ui.label(RichText::new(format!("{hidden} commits collapsed below")).weak());
    }
}

fn edge_tooltip(ui: &mut Ui, scene: &Scene, edge_index: usize) {
    let edge = scene.graph.edges[edge_index];
    ui.label(edge_summary(scene, edge));
    if edge.hidden == 0 {
        ui.label(RichText::new("direct parent").weak());
        return;
    }
    ui.label(RichText::new(format!("{} commits collapsed:", edge.hidden)).strong());
    let collapsed = scene.graph.collapsed(edge_index);
    let listed = &collapsed[..collapsed.len().min(EDGE_TOOLTIP_MAX_COMMITS)];
    for &c in listed {
        let commit = scene.repo.commit(c);
        ui.horizontal(|ui| {
            ui.monospace(commit.oid.short(scene.repo.abbrev_len));
            ui.label(&commit.subject);
        });
    }
    let unlisted = (edge.hidden as usize).saturating_sub(listed.len());
    if unlisted > 0 {
        ui.label(RichText::new(format!("… and {unlisted} more")).weak());
    }
}

/// Returns whether *Branch colours…* was clicked.
fn legend(ui: &mut Ui, palette: &Palette, rules: &[crate::theme::BranchColor]) -> bool {
    let label_swatch = |ui: &mut Ui, fill: Color32, text_color: Color32, text: &str, what: &str| {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(LEGEND_SWATCH_SIZE, Sense::hover());
            ui.painter().rect_filled(rect, 4.0, fill);
            ui.painter().text(
                rect.left_center() + vec2(8.0, 0.0),
                egui::Align2::LEFT_CENTER,
                text,
                FontId::monospace(12.0),
                text_color,
            );
            ui.label(what);
        });
    };
    let swatch = |ui: &mut Ui, fill: Color32, text: &str, what: &str| {
        label_swatch(ui, fill, crate::theme::text_on(fill), text, what);
    };
    swatch(
        ui,
        palette.current_branch,
        "main",
        "Current branch (HEAD), or a detached HEAD",
    );
    swatch(ui, palette.local_branch, "feature/x", "Local branch");
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(LEGEND_SWATCH_SIZE, Sense::hover());
        let fill = palette.local_branch;
        let corners = egui::CornerRadius::same(4);
        ui.painter().rect_filled(rect, corners, fill);
        let period = crate::merged_branches::period_for(rect.height());
        crate::merged_branches::paint_stripes(ui.painter(), rect, corners, period, fill);
        ui.painter().text(
            rect.left_center() + vec2(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "feature/z",
            FontId::monospace(12.0),
            crate::theme::text_on(fill),
        );
        ui.label(
            "Local branch squash- or rebase-merged; a green dashed line goes to where it \
             landed",
        );
    });
    swatch(
        ui,
        palette.remote_branch,
        "origin/feature/x",
        "Remote-tracking branch",
    );
    swatch(ui, palette.tag, "v1.2.0", "Tag");
    swatch(ui, palette.stash, "stash", "Stash");
    swatch(ui, palette.other_ref, "pull/12/head", "Other ref");
    for (fill, what) in [
        (
            palette.pull_request,
            "Open pull request on GitHub or Azure DevOps (click to open)",
        ),
        (palette.draft_pull_request, "Draft pull request"),
    ] {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(LEGEND_SWATCH_SIZE, Sense::hover());
            let text = crate::theme::text_on(fill);
            ui.painter().rect_filled(rect, 4.0, fill);
            // As in the graph: the number right-aligned, after the glyph.
            let (end, _) = render::pull_request_label(rect, 0.0, 1.0);
            let number = ui.painter().text(
                end,
                egui::Align2::RIGHT_CENTER,
                "12",
                FontId::monospace(12.0),
                text,
            );
            let (_, icon) = render::pull_request_label(rect, number.width(), 1.0);
            crate::widgets::paint_glyph(
                ui.painter(),
                icon,
                parterre_core::glyphs::PULL_REQUEST,
                text,
            );
            ui.label(what);
        });
    }
    for (fill, name, italics, glyph, what) in [
        (
            palette.worktree,
            "feature/y",
            false,
            parterre_core::glyphs::FOLDER,
            "Branch checked out in another worktree",
        ),
        (
            palette.worktree,
            "wt-fix",
            true,
            parterre_core::glyphs::FOLDER,
            "Another worktree's detached HEAD",
        ),
        (
            palette.missing_worktree,
            "wt-old",
            true,
            parterre_core::glyphs::FOLDER_GONE,
            "Worktree whose folder is gone",
        ),
    ] {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(LEGEND_SWATCH_SIZE, Sense::hover());
            let text = crate::theme::text_on(fill);
            ui.painter().rect_filled(rect, 4.0, fill);
            let icon =
                Rect::from_center_size(rect.left_center() + vec2(14.0, 0.0), Vec2::splat(12.0));
            crate::widgets::paint_glyph(ui.painter(), icon, glyph, text);
            let format = egui::TextFormat {
                font_id: FontId::monospace(12.0),
                color: text,
                italics,
                ..egui::TextFormat::default()
            };
            let job = egui::text::LayoutJob::single_section(name.to_owned(), format);
            let galley = ui.painter().layout_job(job);
            let at = rect.left_center() + vec2(24.0, -galley.size().y / 2.0);
            ui.painter().galley(at, galley, text);
            ui.label(what);
        });
    }
    label_swatch(
        ui,
        palette.plain_fill,
        palette.plain_text,
        "1a2b3c4d",
        "Commit without refs (branch point or merge)",
    );
    for rule in rules {
        swatch(ui, rule.color, &rule.patterns, "Branches matching");
    }
    let open_colours = ui.link("Branch colours…").clicked();
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(
            "Arrows point from a commit to its parents. Edges may stand for many \
                         hidden commits; hover an edge to list them, click it to keep it \
                         highlighted.",
        )
        .wrap(),
    );
    open_colours
}

fn shortcuts(ui: &mut Ui) {
    let rows = shortcut_rows();
    egui::Grid::new("shortcuts")
        .striped(true)
        .num_columns(2)
        .min_col_width(220.0)
        .spacing(vec2(12.0, 6.0))
        .show(ui, |ui| {
            for (keys, what) in rows {
                ui.strong(keys);
                ui.add(egui::Label::new(what).wrap());
                ui.end_row();
            }
        });
}

fn shortcut_rows() -> Vec<(String, &'static str)> {
    use keys::{with_command, with_shift};
    let either = |a: keys::Shortcut, b: keys::Shortcut| format!("{} / {}", a.label(), b.label());
    vec![
        (
            "Drag a node".into(),
            "Move it, with the rest of the selection it belongs to",
        ),
        (
            "Click an edge".into(),
            "Keep it highlighted; click it again to let go",
        ),
        (
            "1 / 2 / 3".into(),
            "Drag mode Adapt (the graph gives way) / Free (nothing else moves) / Subtree (take \
             along what grows out of it)",
        ),
        ("Click a node".into(), "Select it"),
        (
            "Click a pull request's number".into(),
            "Open the pull request in the browser",
        ),
        (
            format!("{}, double-click a node", keys::SHOW_LOG.label()),
            "Show log: of the node, or of the range between two selected nodes \
             (first..second)",
        ),
        (
            format!("{} / {}", with_command("click"), with_shift("click")),
            "Toggle it in / add it to the selection",
        ),
        (
            format!("{} the background", with_shift("drag")),
            "Select the nodes in a rectangle",
        ),
        ("Esc".into(), "Clear the selection"),
        (either(keys::UNDO, keys::redo()), "Undo / redo a move"),
        ("Drag the background".into(), "Pan"),
        (
            format!("Wheel / {}", with_shift("wheel")),
            "Scroll vertically / horizontally",
        ),
        (
            format!("{}, pinch", with_command("wheel")),
            "Zoom around the pointer",
        ),
        (
            format!(
                "{} / {} / {}",
                keys::ZOOM_IN.label(),
                keys::ZOOM_OUT.label(),
                keys::ACTUAL_SIZE.label()
            ),
            "Zoom in / out / 100%; in the log, compare, diff, blame and settings windows, their \
             text size",
        ),
        (
            format!("{} off the graph", with_command("wheel")),
            "Text size of every window (Settings, Appearance)",
        ),
        (
            format!("{}, double-click background", keys::ZOOM_TO_FIT.label()),
            "Zoom to fit the whole graph",
        ),
        (keys::GO_TO_HEAD.label(), "Go to HEAD"),
        (
            keys::FIND.label(),
            "Find; Enter / Shift+Enter for next / previous",
        ),
        (
            either(keys::find_next(), keys::find_previous()),
            "Next / previous search hit",
        ),
        (keys::COPY.label(), "Copy the selected commit's hash"),
        (
            format!("{}, {}", keys::RELOAD.label(), keys::RELOAD_F5.label()),
            "Reload the repository",
        ),
        (keys::fetch().label(), "Fetch every remote, in every window"),
        (either(keys::OPEN, keys::CLOSE), "Open / close a folder"),
        (keys::SETTINGS.label(), "Settings"),
        match keys::Platform::CURRENT {
            keys::Platform::Mac => (
                "⌃F2, ⇧⌘/".into(),
                "The menu bar from the keyboard; Help's search finds any command by name",
            ),
            _ => (
                "Alt+letter, F10, Alt".into(),
                "The menu bar: Alt with a menu's underlined letter opens it, a letter in it \
                 chooses; F10 or Alt alone reaches the bar, the arrow keys move",
            ),
        },
        (
            keys::close_window().label(),
            "Close the log, compare, diff, blame or settings window",
        ),
        (
            "Right-click a node".into(),
            "Show log, compare (with HEAD, the working tree, two nodes, or the commit marked \
             for comparison), open its pull requests, copy hash or refs, select its subtree, \
             return it to the layout",
        ),
        (
            if keys::Platform::CURRENT == keys::Platform::Windows {
                format!("{}, the Menu key", keys::CONTEXT_MENU.label())
            } else {
                keys::CONTEXT_MENU.label()
            },
            "The right-click menu of what has the keyboard: the selected node, or a list's \
             selected row",
        ),
    ]
}

/// The repository named at start, loading while the window comes up.
pub struct Opening {
    path: PathBuf,
    /// Else the current directory is tried, and failing is no error.
    named: bool,
    rx: mpsc::Receiver<Result<Repo, String>>,
}

impl Opening {
    pub fn start(path: Option<PathBuf>) -> Opening {
        let named = path.is_some();
        let path = path.unwrap_or_else(|| PathBuf::from("."));
        let (tx, rx) = mpsc::channel();
        let dir = path.clone();
        std::thread::spawn(move || {
            let loaded = parterre_core::git::load_repo(&dir).map_err(|e| e.to_string());
            crate::startup::mark("repository loaded");
            let _ = tx.send(loaded);
        });
        Opening { path, named, rx }
    }

    /// `None` when the unnamed current directory isn't a repository.
    pub fn wait(self) -> Result<Option<Repo>, String> {
        match self.rx.recv() {
            Ok(Ok(repo)) => Ok(Some(repo)),
            Ok(Err(e)) if self.named => Err(e),
            Err(_) if self.named => Err("loading stopped unexpectedly".into()),
            _ => Ok(None),
        }
    }

    fn poll(&self) -> Option<Result<Repo, String>> {
        worker::received(&self.rx, || Err("loading stopped unexpectedly".into()))
    }

    fn status(&self) -> Option<Note> {
        self.named
            .then(|| Note::Info(format!("Opening {}…", self.path.display())))
    }

    /// The one the repository will have, so that it doesn't change when the repository is in.
    pub fn title(&self) -> String {
        match self.name() {
            Some(name) => graph_window_title(&name),
            None => window_title(None),
        }
    }

    fn name(&self) -> Option<String> {
        self.named.then(|| name_and_place(&self.path).0)
    }
}

fn opening_panel(ui: &mut Ui, name: &str) {
    ui.add_space((ui.available_height() / 2.0 - 10.0).max(0.0));
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(format!("Opening {name}…")).color(ui.visuals().weak_text_color()));
    });
}

pub fn window_title(repo: Option<&Repo>) -> String {
    match repo {
        Some(repo) => graph_window_title(&repo.display_name()),
        None => "parterre".to_owned(),
    }
}

fn graph_window_title(repo_name: &str) -> String {
    format!("{repo_name} – Revision Graph")
}

/// A repository's folder name, and the folder it is in: `("Apps", "C:\src")`.
pub fn name_and_place(path: &Path) -> (String, String) {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let place = path
        .parent()
        .map_or_else(String::new, |p| p.display().to_string());
    (name, place)
}

/// "From main to its merged parent feature/x": the default fonts have no arrow.
fn edge_summary(scene: &Scene, edge: RevEdge) -> String {
    format!(
        "From {} to its {}parent {}",
        node_name(scene, edge.child),
        if edge.first_parent { "" } else { "merged " },
        node_name(scene, edge.parent)
    )
}

/// Its first ref, or its short hash.
fn node_name(scene: &Scene, node: u32) -> String {
    let node = &scene.graph.nodes[node as usize];
    match node.refs.first() {
        Some(&r) => scene.repo.refs[r].name.clone(),
        None => scene
            .repo
            .commit(node.commit)
            .oid
            .short(scene.repo.abbrev_len),
    }
}

fn resolve_oid(repo: &Repo, name: &str) -> Result<Oid, String> {
    repo.resolve(name)
        .map(|c| repo.commit(c).oid)
        .ok_or_else(|| format!("no commit named {name}"))
}

fn status_label(ui: &mut Ui, note: &Note) {
    let color = if note.is_error() {
        Color32::RED
    } else {
        ui.visuals().weak_text_color()
    };
    ui.colored_label(color, note.text());
}

impl ParterreApp {
    /// Of egui and of the window's title bar.
    fn apply_theme(&mut self, ctx: &egui::Context) {
        use egui::{SystemTheme as Window, Theme, ThemePreference as Egui};
        let (egui_theme, window_theme) = match self.settings.theme {
            // Where winit can't tell the system theme, egui would pick dark, and the title bar
            // would stay as it started.
            ThemeChoice::System => match self.system_theme.get() {
                Some(Theme::Light) => (Egui::Light, Window::Light),
                Some(Theme::Dark) => (Egui::Dark, Window::Dark),
                None => (Egui::System, Window::SystemDefault),
            },
            ThemeChoice::Light => (Egui::Light, Window::Light),
            ThemeChoice::Dark => (Egui::Dark, Window::Dark),
        };
        ctx.set_theme(egui_theme);
        if self.window_theme != Some(window_theme) {
            self.window_theme = Some(window_theme);
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(window_theme));
        }
    }
}

const GH_INSTALL_URL: &str = "https://github.com/cli/cli#installation";
const WELCOME_RECENT_COUNT: usize = 5;
const HEAD_MIN_ZOOM: f32 = 0.6;
const ARROW_KEY_PAN: f32 = 60.0;
const MESSAGE_TIP_MAX_LINES: usize = 40;
const MESSAGE_TIP_MAX_CHARS: usize = 4000;
const EDGE_TOOLTIP_MAX_COMMITS: usize = 12;
const LEGEND_SWATCH_SIZE: Vec2 = vec2(150.0, 20.0);

/// Each worktree with a rebase in progress: its HEAD, and the ref of the branch it rebases.
fn rebasing_marks(
    repo: &Repo,
    catalog: Option<&parterre_core::branches::Catalog>,
) -> Vec<(parterre_core::CommitIx, usize)> {
    catalog
        .iter()
        .flat_map(|catalog| &catalog.worktrees)
        .filter_map(|w| {
            let branch = w.rebasing.as_ref()?.branch.as_deref()?;
            let head = repo.lookup(&w.head?)?;
            let full = format!("refs/heads/{branch}");
            let r = repo.refs.iter().position(|r| r.full_name == full)?;
            Some((head, r))
        })
        .collect()
}

/// Each worktree with a merge or a cherry-pick in progress: its HEAD, and the commit being
/// merged or picked.
fn merging_marks(
    repo: &Repo,
    catalog: Option<&parterre_core::branches::Catalog>,
) -> Vec<(parterre_core::CommitIx, parterre_core::CommitIx)> {
    catalog
        .iter()
        .flat_map(|catalog| &catalog.worktrees)
        .filter_map(|w| {
            let theirs = w.merging.or(w.picking.as_ref().map(|p| p.commit))?;
            Some((repo.lookup(&w.head?)?, repo.lookup(&theirs)?))
        })
        .collect()
}

fn palette_for(ui: &Ui, settings: &Settings) -> Palette {
    Palette::new(ui.visuals().dark_mode, &settings.branch_colors)
}

enum MenuAction {
    ShowLog(Vec<usize>),
    OpenPullRequest(String),
}

#[derive(Clone, Copy, Debug)]
enum Opener {
    FileManager,
    Terminal,
}

impl ParterreApp {
    fn palette(&self, ctx: &egui::Context) -> Palette {
        Palette::new(
            ctx.global_style().visuals.dark_mode,
            &self.settings.branch_colors,
        )
    }

    fn open_pull_request(&mut self, url: &str) {
        usage::action(usage::Action::OpenPullRequest);
        if let Err(e) = crate::browser::open(url) {
            self.status = Some(Note::Error(e));
        }
    }

    fn open_in(&mut self, opener: Opener, dir: &Path) {
        let opened = match opener {
            Opener::FileManager => {
                usage::action(usage::Action::OpenFileManager);
                crate::file_manager::open(dir)
            }
            Opener::Terminal => {
                usage::action(usage::Action::OpenTerminal);
                crate::file_manager::open_terminal(dir)
            }
        };
        if let Err(e) = opened {
            self.status = Some(Note::Error(e));
        }
    }

    fn compare_window_requests(&mut self, ctx: &egui::Context) {
        let opener = compare_window::viewport_id();
        for request in std::mem::take(&mut self.compare.requests) {
            match request {
                compare_window::FileRequest::MergeTool { root, path } => {
                    self.merge_tools.open(ctx, root, path, opener)
                }
                compare_window::FileRequest::Resolve(resolve) => self.branches.request(
                    ctx,
                    branches::Request::Run(parterre_core::branches::Action::Resolve(Box::new(
                        resolve,
                    ))),
                    opener,
                ),
                compare_window::FileRequest::Open(how, dir) => self.open_in(how, &dir),
            }
        }
    }
}

impl eframe::App for ParterreApp {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        crate::startup::mark("first frame");
        // One pass paints the main window and every immediate viewport, so this paces them all.
        if let Some(limiter) = &mut self.frame_limiter {
            limiter.wait();
        }
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        // Uncloaked (Windows) once a frame at the final size has been painted (#309).
        if self.reveal.as_mut().is_some_and(|r| r.finish(&ctx)) {
            self.reveal = None;
        }
        crate::dialogs::set_look(
            &ctx,
            crate::dialogs::Look {
                icon: self.window_icon.clone(),
                theme: self.window_theme,
            },
        );
        self.view.text_size = ctx.zoom_factor();
        self.graph_hovered = false;
        let title = self
            .opening
            .as_ref()
            .map_or_else(|| window_title(self.repo.as_deref()), Opening::title);
        if self.title_shown != title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title_shown = title;
        }
        let shown = self.repo.as_ref().map(|r| r.path.as_path());
        if self.persist && self.restart_on.as_ref().map(Option::as_deref) != Some(shown) {
            crate::restart::register(shown);
            self.restart_on = Some(shown.map(Path::to_path_buf));
        }
        self.finish_opening(&ctx);
        // Dropped on the Dock icon, opened with parterre, or its service (#336, #337).
        #[cfg(target_os = "macos")]
        if let Some(path) = crate::macos::take_opened(&ctx).into_iter().next() {
            self.open_folder(&path);
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        #[cfg(target_os = "macos")]
        self.input_method
            .update(frame, ctx.egui_wants_keyboard_input());
        self.auto_reload(&ctx);
        self.finish_reload(&ctx);
        self.branches.update(&ctx, self.repo.as_ref());
        if std::mem::take(&mut self.branches.conflicts_changed) {
            self.compare.relist_working_tree();
        }
        if let Some(path) = self.branches.reload.take()
            && self.repo.as_ref().is_some_and(|r| r.path == path)
        {
            self.reload();
        }
        // A revert says it's done by the log moving to the new commit.
        if let Some(reverted) = self.branches.reverted.take() {
            let here = self.repo.as_ref().is_some_and(|r| r.path == reverted.path);
            if !(here && self.log.follow(reverted.from, reverted.to)) {
                self.branches.reverted_unseen(&ctx, reverted);
            }
        }
        if let Some(path) = self.branches.go_to.take() {
            self.go_to_worktree(&path);
        }
        if let Some(path) = self.branches.cloned.take() {
            self.open_folder(&path);
        }
        if let Some(what) = self.automation.pending_open.take() {
            match self.open_named(&ctx, &what) {
                Ok(true) => {}
                Ok(false) => self.automation.pending_open = Some(what),
                Err(e) => self
                    .automation
                    .fail_step(&ctx, &format!("open {what}: {e}")),
            }
        }
        self.update_pull_requests(&ctx);
        if self.merged.update(self.repo.as_ref(), &ctx) {
            self.requested_options = None;
        }
        self.usage_statistics(&ctx);
        self.update_check(&ctx);
        self.ensure_scene(&ctx);
        self.view_at_frame_start = self.view;
        let menus = self.menu_bar_keys(&ctx);
        // The window's keys may change what the menus show.
        let window_keys = ctx.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::Key { pressed: true, .. }))
        });
        self.handle_keys(&ctx);

        self.menu_bar(ui, menus.filter(|_| !window_keys));
        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::side_top_panel(&ctx.global_style())
                    .inner_margin(egui::Margin::symmetric(8, 6)),
            )
            .show(ui, |ui| self.toolbar(ui));
        if self.settings.show_status_bar {
            egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        }
        let banner = self.branches.banner_shown();
        if let (Some(repo), Some(catalog)) = (&self.repo, &self.branches.catalog)
            && banner
            && let Some(head) = rebase::banner(ui, repo, catalog)
        {
            self.compare_request(CompareRequest::WorkingTree(head));
        }
        if self.scene.is_some() {
            egui::CentralPanel::no_frame().show(ui, |ui| self.canvas(ui));
        } else if let Some(name) = self.opening_name() {
            egui::CentralPanel::default().show(ui, |ui| opening_panel(ui, &name));
        } else {
            egui::CentralPanel::default().show(ui, |ui| self.welcome(ui));
        }
        if !self.graph_hovered {
            text_size::read_input(ui, &mut self.settings.text_size, false);
        }
        self.shortcuts_window(&ctx);
        self.pull_requests_dialog(&ctx);
        self.legend_window(&ctx);
        self.settings_window(&ctx);
        self.import_settings_dialog(&ctx);
        self.reset_settings_dialog(&ctx);
        self.log_window(&ctx);
        self.compare_window(&ctx);
        self.compare_window_requests(&ctx);
        self.diff_windows(&ctx);
        self.blame_windows(&ctx);
        if let Some(error) = self.about.show(&ctx, self.heard_ring()) {
            self.status = Some(Note::Error(error));
        }
        self.release_dialog(&ctx);
        self.file_drop(&ctx);
        // Over everything else.
        self.first_run_prompt(&ctx, frame);
        let palette = self.palette(&ctx);
        let merged = self.merged.shown();
        self.branches
            .show(&ctx, &palette, &self.settings.graph, merged);
        self.merge_tools.show(&ctx);
        if let Some(e) = self.merge_tools.error.take() {
            self.status = Some(Note::Error(e));
        }
        for (repo, spec) in std::mem::take(&mut self.branches.diff_requests) {
            self.diffs
                .open(repo, spec, &self.settings.diff_window, &ctx);
        }
        if let Some((repo, commits, exact)) = self.branches.log_request.take() {
            if exact {
                self.open_loss_log(repo, &commits);
            } else {
                let indices: Vec<_> = commits.iter().filter_map(|oid| repo.lookup(oid)).collect();
                self.open_log(repo, &indices);
            }
        }

        // Scripted runs wait for the graph, if any, and for the diffs, the blames and the pull
        // requests, with their layout, to load.
        if self.scene.is_some() || self.repo.is_none() {
            let pulling = self.pull_requests.is_loading()
                || self.pull_requests_active()
                    && self.pull_requests.list().is_some()
                    && self.job.is_some();
            let merging =
                self.merged.is_loading() || !self.merged.shown().is_empty() && self.job.is_some();
            self.automation.waiting = self.diffs.is_loading()
                || self.blames.is_loading()
                || pulling
                || merging
                || self.reloading.is_some();
            self.automation
                .drive(&ctx, self.scene.as_ref(), &mut self.view, self.canvas);
        }

        self.file_dialogs(&ctx, frame);
        // Takes effect from the next frame on, in every window.
        if ctx.zoom_factor() != self.settings.text_size {
            ctx.set_zoom_factor(self.settings.text_size);
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.automation.inject_input(raw_input);
        // What the menu bar did in this window (macOS: Copy in the find field, and the like).
        raw_input
            .events
            .extend(commands::take_injected(ctx, egui::ViewportId::ROOT));
        // Before eframe sees a close request: the main window stays while a modal dialog is up.
        crate::dialogs::ModalLock::filter(ctx, raw_input);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.persist {
            self.keep_repo_settings();
            storage.set_string(
                settings_file::STORAGE_KEY,
                self.stored.write(&self.settings),
            );
            eframe::set_value(storage, MOVES_KEY, &self.moves);
            eframe::set_value(storage, merged::MERGED_KEY, self.merged.remembered());
            eframe::set_value(storage, RECENT_KEY, &self.recent);
            eframe::set_value(storage, CLONE_PARENT_KEY, &self.branches.clone_parent);
            eframe::set_value(storage, PRIVACY_KEY, &self.telemetry.privacy);
            eframe::set_value(storage, updates::RELEASE_TOLD_KEY, &self.release_told);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.telemetry.close();
    }
}

/// Syntax colouring runs in a child process, this program again, so that a grammar's crash
/// costs the colours and not the window; without the program's path, not at all.
fn highlight_engine() -> parterre_highlight::Engine {
    use parterre_highlight::Engine;
    std::env::current_exe().map_or(Engine::Off, Engine::Child)
}

/// In this order: the context-menu key, handed in first, passes the rest as a pressed Shift+F10
/// would, and a click on a locked window counts as user input before the lock drops it.
fn add_input_plugins(ctx: &egui::Context) {
    crate::context_key::install(ctx);
    ctx.add_plugin(privacy::UserInput::default());
    ctx.add_plugin(crate::dialogs::ModalLock::default());
    ctx.add_plugin(crate::focus::Cues::default());
    ctx.add_plugin(menu::ContextMenus);
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, Rect};

    use crate::focus::tests::Screen;

    #[test]
    fn the_context_menu_key_shows_what_has_the_keyboard_as_shift_f10_does() {
        let ctx = egui::Context::default();
        super::add_input_plugins(&ctx);
        let mut s = Screen::of(ctx);
        let button = std::cell::Cell::new((egui::Id::NULL, Rect::NOTHING));
        let show = |ui: &mut egui::Ui| {
            let response = ui.button("Item");
            button.set((response.id, response.rect));
        };
        s.frame(show);
        let (id, rect) = button.get();
        s.ctx.memory_mut(|m| m.request_focus(id));
        s.frame(show);
        assert!(!s.ringed(rect), "not before the keyboard is used");
        let _turn = crate::context_key::tests::press();
        s.frame(show);
        assert!(s.ringed(rect));
    }
}
