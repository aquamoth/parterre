//! The eframe application: menus, toolbar, canvas interaction and status bar.

use std::path::{Path, PathBuf};

use eframe::egui::{
    self, Color32, FontId, Key, Modifiers, PointerButton, Pos2, Rect, RichText, Sense, Ui, Vec2,
    vec2,
};
use parterre_core::git::CommitDetails;
use parterre_core::layout::{Direction, LayoutOptions};
use parterre_core::physics::DragModel;
use parterre_core::revgraph::{GraphOptions, RevEdge};
use std::sync::Arc;

use parterre_core::recent::Recent;
use parterre_core::{Oid, Repo};

mod auto_reload;
mod blame_window;
mod branches;
mod column_borders;
mod commit_table;
mod compare_window;
mod diff_window;
mod file_table;
mod log_window;
mod pull_requests;
mod settings_window;
mod toolbar;

pub use toolbar::popup_id;

use settings_window::SettingsPage;

use crate::automation::Automation;
use crate::export::{self, Format};
use crate::file_dialog::Pending;
use crate::frame_pacing::FrameLimiter;
use crate::menu;
use crate::render::{self, Marks};
use crate::scene::{FONT_SIZE, Scene, to_point};
use crate::settings::{MOVES_KEY, RECENT_KEY, RememberedMoves, STORAGE_KEY, Settings, load_moves};
use crate::system_theme::SystemTheme;
use crate::text_size;
use crate::theme::{Palette, ThemeChoice};
use crate::view::View;
use compare_window::CompareRequest;

/// What a file dialog is picking for.
#[derive(Clone, Copy, Debug)]
enum Picked {
    Folder,
    Export(Format),
}

#[derive(Clone, Copy, Debug)]
enum Drag {
    /// Dragging nodes; `grab` is the pointer's offset from the grabbed node's centre (world
    /// units).
    Node {
        grab: Vec2,
    },
    Pan,
    /// Selecting the nodes in the rectangle between `start` and `end` (world coordinates).
    Select {
        start: Pos2,
        end: Pos2,
    },
}

/// Selected nodes, in the order they were selected. The last one is the current node, whose
/// details the status bar shows.
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

    /// Selects `node` alone, or nothing.
    fn set(&mut self, node: Option<usize>) {
        self.nodes.clear();
        self.nodes.extend(node);
    }

    /// Adds `node`, making it the current node.
    fn add(&mut self, node: usize) {
        self.nodes.retain(|&n| n != node);
        self.nodes.push(node);
    }

    /// Adds `nodes` that are not selected yet, keeping the current node current.
    fn extend(&mut self, nodes: impl IntoIterator<Item = usize>) {
        let mut seen: std::collections::HashSet<usize> = self.nodes.iter().copied().collect();
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

    /// Per node of a scene with `count` nodes: selected.
    fn mask(&self, count: usize) -> Vec<bool> {
        let mut mask = vec![false; count];
        for &n in &self.nodes {
            if n < count {
                mask[n] = true;
            }
        }
        mask
    }
}

/// The nodes dragged along with `anchor`: the selection it belongs to, or `anchor` alone.
fn dragged_with(selection: &Selection, anchor: usize) -> Vec<usize> {
    if selection.contains(anchor) {
        selection.nodes.clone()
    } else {
        vec![anchor]
    }
}

/// Full commit messages, notes and committers for tooltips and the log (and for each blame
/// window's history pane, which has its own), fetched from git on a worker thread when first
/// needed. A failure is kept as its message.
#[derive(Debug, Default)]
struct Details {
    /// `None` while loading.
    cache: std::collections::HashMap<parterre_core::Oid, Option<Result<CommitDetails, String>>>,
    rx: Option<std::sync::mpsc::Receiver<(parterre_core::Oid, Result<CommitDetails, String>)>>,
    tx: Option<std::sync::mpsc::Sender<parterre_core::Oid>>,
}

impl Details {
    /// The details of `oid` if already loaded; otherwise requests them.
    fn get(
        &mut self,
        repo_path: &std::path::Path,
        oid: parterre_core::Oid,
        ctx: &egui::Context,
    ) -> Option<&Result<CommitDetails, String>> {
        while let Some(Ok((oid, details))) = self.rx.as_ref().map(|rx| rx.try_recv()) {
            self.cache.insert(oid, Some(details));
        }
        if let std::collections::hash_map::Entry::Vacant(slot) = self.cache.entry(oid) {
            slot.insert(None);
            let tx = self.tx.get_or_insert_with(|| {
                let (req_tx, req_rx) = std::sync::mpsc::channel::<parterre_core::Oid>();
                let (res_tx, res_rx) = std::sync::mpsc::channel();
                let git = parterre_core::git::Git::new(repo_path);
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    for oid in req_rx {
                        let details = git.details(&oid).map_err(|e| e.to_string());
                        if res_tx.send((oid, details)).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                });
                self.rx = Some(res_rx);
                req_tx
            });
            let _ = tx.send(oid);
        }
        self.cache.get(&oid).and_then(Option::as_ref)
    }

    /// The full message of `oid` (or why it could not be read) if already loaded.
    fn message(
        &mut self,
        repo_path: &std::path::Path,
        oid: parterre_core::Oid,
        ctx: &egui::Context,
    ) -> Option<String> {
        self.get(repo_path, oid, ctx).map(|d| match d {
            Ok(d) => d.message.clone(),
            Err(e) => format!("({e})"),
        })
    }
}

/// A layout running on a worker thread.
#[derive(Debug)]
struct LayoutJob {
    rx: std::sync::mpsc::Receiver<Scene>,
    /// Commit near the view centre and its screen position, to keep the view steady.
    anchor: Option<(Oid, Pos2)>,
    selected_commits: Vec<Oid>,
    /// Child and parent commit of the selected edge.
    selected_edge: Option<(Oid, Oid)>,
}

#[derive(Debug, Default)]
struct Search {
    query: String,
    hits: Vec<usize>,
    current: Option<usize>,
    request_focus: bool,
}

pub struct ParterreApp {
    /// The most recently loaded snapshot, used for new layouts. The scene on screen keeps its
    /// own snapshot until a new layout replaces it. `None` until a repository is opened.
    repo: Option<Arc<Repo>>,
    branches: branches::Tool,
    /// Repositories opened before, newest first.
    recent: Recent,
    /// Show the folder picker at the end of this frame.
    pick_folder: bool,
    /// The file dialog that is open, if any.
    file_dialog: Option<Pending<Picked>>,
    /// The window title last set.
    title: String,
    settings: Settings,
    /// False for automated runs, so they don't overwrite the user's settings.
    persist: bool,
    scene: Option<Scene>,
    /// Options of the most recently requested layout.
    requested: Option<(GraphOptions, LayoutOptions)>,
    job: Option<LayoutJob>,
    view: View,
    /// The view at the start of the frame, before any panning or zooming.
    view_before: View,
    needs_initial_view: bool,
    canvas: Rect,
    /// The pointer is over the graph (or its overview map), where Ctrl+wheel is the graph's.
    graph_hovered: bool,
    hovered: Option<usize>,
    hovered_edge: Option<usize>,
    selection: Selection,
    /// Edge kept highlighted after a click, independent of the selected nodes.
    selected_edge: Option<usize>,
    /// Nodes that would move if the hovered node were dragged in Subtree mode, cached for
    /// the roots they were computed from.
    preview: Option<(Vec<usize>, Vec<usize>)>,
    context_node: Option<usize>,
    /// Commits to select once the scene has been rebuilt (after a reload).
    pending_select: Vec<Oid>,
    drag: Option<Drag>,
    search: Search,
    status: Option<(String, bool)>,
    show_shortcuts: bool,
    show_legend: bool,
    show_settings: bool,
    settings_page: SettingsPage,
    show_about: bool,
    /// Show the save dialog for exporting in this format at the end of this frame.
    export: Option<Format>,
    /// The folder exported to last, where the save dialog starts next time.
    export_dir: Option<PathBuf>,
    details: Details,
    /// The log window (Show log), and what it keeps while closed.
    log: log_window::LogWindow,
    /// Raise the log window in the next frame (Show log while it is open).
    focus_log: bool,
    /// The open diff windows, one file diff each.
    diffs: diff_window::DiffWindows,
    /// The open blame windows, one file each.
    blames: blame_window::BlameWindows,
    /// The compare window (Compare revisions and the like).
    compare: compare_window::CompareWindow,
    /// Raise the compare window in the next frame.
    focus_compare: bool,
    /// The commit marked for comparison, and its name when it was marked (or last reloaded).
    marked: Option<(Oid, String)>,
    /// Dragged nodes of every repository, kept when `remember_moves` is on.
    moves: RememberedMoves,
    /// Moved nodes to put back in the next scene: after a reload, when `remember_moves` is
    /// off.
    carried_moves: Option<std::collections::HashMap<String, (f32, f32, bool)>>,
    /// Reloads when the refs change, if `settings.auto_reload` is on.
    watcher: Option<auto_reload::Watcher>,
    /// Open pull requests from GitHub, while they are shown.
    pull_requests: pull_requests::PullRequestLoader,
    /// Whether pull requests were turned on in the last frame, to see the user turn them on.
    pull_requests_setting: bool,
    /// Why pull requests the user asked for couldn't be loaded, shown in a dialog.
    pull_requests_error: Option<parterre_core::forge::ForgeError>,
    /// Load the pull requests again (F5).
    refresh_pull_requests: bool,
    system_theme: SystemTheme,
    /// The theme last given to the window (its title bar), if any.
    window_theme: Option<egui::SystemTheme>,
    /// The same for the settings window, while it is open.
    settings_window_theme: Option<egui::SystemTheme>,
    /// The text size the settings window was last shown at, while it is open.
    settings_window_text_size: Option<f32>,
    window_icon: Arc<egui::IconData>,
    /// What is typed into the zoom level, while it has the focus.
    zoom_text: String,
    automation: Automation,
    /// Caps the frame rate where vsync is off (Wayland, see `frame_pacing`).
    frame_limiter: Option<FrameLimiter>,
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
        open_error: Option<String>,
        overrides: impl FnOnce(&mut Settings),
        automation: Automation,
        vsync: bool,
    ) -> ParterreApp {
        let persist = !automation.is_active();
        let mut settings: Settings = cc
            .storage
            .filter(|_| persist)
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();
        overrides(&mut settings);
        // Settings edited by hand or saved by another version may put a divider out of reach.
        settings.log_window.dividers = settings.log_window.dividers.clamped();
        settings.text_size = parterre_core::text_size::sanitize(settings.text_size);
        cc.egui_ctx.set_zoom_factor(settings.text_size);
        let moves: RememberedMoves = cc
            .storage
            .filter(|_| persist)
            .map(load_moves)
            .unwrap_or_default();
        let mut recent: Recent = cc
            .storage
            .filter(|_| persist)
            .and_then(|s| eframe::get_value(s, RECENT_KEY))
            .unwrap_or_default();
        if let Some(repo) = &repo {
            recent.add(&repo.path);
        }
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        // One screenshot shows everything: the settings in the main window.
        cc.egui_ctx.set_embed_viewports(automation.is_active());
        let demo_settings = automation
            .demo_open
            .as_deref()
            .and_then(|o| o.strip_prefix("settings"))
            .map(|page| SettingsPage::named(page.trim_start_matches(':')).unwrap_or_default());
        // Automated runs are short and show one window (viewports are embedded), so they neither
        // freeze nor need their frame rate capped: they run as fast as they can.
        let frame_limiter = (!vsync && !automation.is_active()).then(FrameLimiter::default);
        let demo_log: Option<Vec<parterre_core::CommitIx>> = automation
            .demo_log
            .as_deref()
            .zip(repo.as_ref())
            .map(|(spec, repo)| {
                spec.split("..")
                    .filter_map(|name| {
                        let commit = repo.resolve(name);
                        if commit.is_none() {
                            eprintln!("--demo-log: no commit named {name}");
                        }
                        commit
                    })
                    .collect()
            });
        // Pull requests on at startup (the default) were not turned on by the user: whatever
        // comes of loading them, nothing is said.
        let pull_requests_setting = settings.graph.show_pull_requests;
        let mut app = ParterreApp {
            title: window_title(repo.as_ref()),
            repo: repo.map(Arc::new),
            recent,
            pick_folder: false,
            file_dialog: None,
            settings,
            persist,
            scene: None,
            requested: None,
            job: None,
            view: View::default(),
            view_before: View::default(),
            needs_initial_view: true,
            canvas: Rect::NOTHING,
            graph_hovered: false,
            hovered: None,
            hovered_edge: None,
            selection: Selection::default(),
            selected_edge: None,
            preview: None,
            branches: branches::Tool::default(),
            context_node: None,
            pending_select: Vec::new(),
            drag: None,
            search: Search::default(),
            status: open_error.map(|e| (e, true)),
            show_shortcuts: false,
            show_legend: false,
            show_settings: demo_settings.is_some(),
            settings_page: demo_settings.unwrap_or_default(),
            show_about: false,
            export: None,
            export_dir: None,
            details: Details::default(),
            log: log_window::LogWindow::default(),
            focus_log: false,
            diffs: diff_window::DiffWindows::default(),
            blames: blame_window::BlameWindows::default(),
            compare: compare_window::CompareWindow::default(),
            focus_compare: false,
            marked: None,
            moves,
            carried_moves: None,
            watcher: None,
            pull_requests: pull_requests::PullRequestLoader::default(),
            pull_requests_setting,
            pull_requests_error: None,
            refresh_pull_requests: false,
            system_theme: SystemTheme::watch(&cc.egui_ctx),
            window_theme: None,
            settings_window_theme: None,
            settings_window_text_size: None,
            window_icon: Arc::new(crate::icon::icon()),
            zoom_text: String::new(),
            automation,
            frame_limiter,
        };
        if let (Some(commits), Some(repo)) = (demo_log, app.repo.clone()) {
            app.open_log(repo, &commits);
        }
        let demo_oid = |repo: &Repo, name: &str, flag: &str| {
            let oid = repo.resolve(name).map(|c| repo.commit(c).oid);
            if oid.is_none() {
                eprintln!("{flag}: no commit named {name}");
            }
            oid
        };
        if let (Some(name), Some(repo)) = (app.automation.demo_mark.clone(), app.repo.clone())
            && let Some(oid) = demo_oid(&repo, &name, "--demo-mark")
        {
            app.compare_request(CompareRequest::Mark(Some(oid)));
        }
        if let (Some(spec), Some(repo)) = (app.automation.demo_compare.clone(), app.repo.clone()) {
            match spec.split_once("..") {
                Some((a, "WORKING_TREE")) => {
                    if let Some(a) = demo_oid(&repo, a, "--demo-compare") {
                        app.compare_request(CompareRequest::WorkingTree(a));
                    }
                }
                Some((a, b)) => {
                    if let (Some(a), Some(b)) = (
                        demo_oid(&repo, a, "--demo-compare"),
                        demo_oid(&repo, b, "--demo-compare"),
                    ) {
                        app.compare(a, b);
                    }
                }
                None => eprintln!("--demo-compare: expected <ref>..<ref>"),
            }
        }
        if let Some(spec) = app.automation.demo_diff.clone() {
            app.open_demo_diff(&spec, &cc.egui_ctx);
        }
        if let Some(spec) = app.automation.demo_blame.clone() {
            app.open_demo_blame(&spec, &cc.egui_ctx);
        }
        app
    }

    /// Starts a new layout when the graph or layout options changed, and installs finished
    /// layouts. Layout runs on a worker thread; the previous scene stays visible meanwhile.
    fn ensure_scene(&mut self, ctx: &egui::Context) {
        let Some(repo) = &self.repo else { return };
        let key = (self.settings.graph.clone(), self.settings.layout.clone());
        if self.requested.as_ref() != Some(&key) {
            self.requested = Some(key);
            let font = FontId::monospace(FONT_SIZE);
            let text_height = ctx.fonts_mut(|f| f.row_height(&font));
            let input = ctx.fonts_mut(|f| {
                let mut width = |s: &str| {
                    f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE)
                        .size()
                        .x
                };
                let pull_requests = self.pull_requests.list().map(|p| &**p);
                Scene::prepare(repo, &self.settings, pull_requests, &mut width, text_height)
            });
            let (tx, rx) = std::sync::mpsc::channel();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                // The receiver is gone if a newer layout superseded this one.
                let _ = tx.send(input.lay_out());
                repaint.request_repaint();
            });
            let pending = std::mem::take(&mut self.pending_select);
            self.job = Some(LayoutJob {
                rx,
                anchor: self.view_anchor(),
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
        self.scene = Some(scene);
        self.hovered = None;
        self.hovered_edge = None;
        self.context_node = None;
        self.drag = None;
        self.preview = None;
        let selected: Vec<usize> = job
            .selected_commits
            .iter()
            .filter_map(|oid| self.node_for(oid))
            .collect();
        self.selection.set(None);
        self.selection.extend(selected);
        self.selected_edge = job.selected_edge.and_then(|(c, p)| self.edge_for(&c, &p));
        self.update_search();
        self.restore_moves();
        if let (Some((oid, screen)), Some(scene)) = (job.anchor, &self.scene)
            && let Some(node) = self.node_for(&oid)
            && self.canvas.is_positive()
        {
            let world = scene.node_center(node);
            let fraction = (screen - self.canvas.min) / self.canvas.size();
            self.view.show_at(self.canvas, world, fraction);
        }
    }

    /// The edge between the nodes shown for these commits, if there is one.
    fn edge_for(&self, child: &Oid, parent: &Oid) -> Option<usize> {
        let (child, parent) = (self.node_for(child)?, self.node_for(parent)?);
        let edges = &self.scene.as_ref()?.graph.edges;
        edges
            .iter()
            .position(|e| e.child as usize == child && e.parent as usize == parent)
    }

    /// The node that shows commit `oid` in the current scene: the commit itself, or the node
    /// it is collapsed into.
    fn node_for(&self, oid: &Oid) -> Option<usize> {
        let scene = self.scene.as_ref()?;
        let commit = scene.repo.lookup(oid)?;
        scene.graph.represented_by(commit).map(|n| n as usize)
    }

    fn repo_key(&self) -> Option<String> {
        Some(self.repo.as_ref()?.path.display().to_string())
    }

    /// Puts remembered nodes, or those carried over a reload, back where they were in a
    /// freshly laid-out scene.
    fn restore_moves(&mut self) {
        let carried = self.carried_moves.take();
        let moves = if self.settings.remember_moves {
            self.repo_key()
                .and_then(|key| self.moves.get(&key).cloned())
        } else {
            carried
        };
        let Some(moves) = moves else { return };
        let Some(scene) = &mut self.scene else { return };
        let saved: Vec<_> = moves
            .iter()
            .filter_map(|(hex, &(dx, dy, by_hand))| {
                let node = Oid::from_hex(hex)
                    .and_then(|oid| scene.repo.lookup(&oid))
                    .and_then(|c| scene.graph.node_of(c))?;
                Some((
                    node as usize,
                    parterre_core::layout::Point::new(dx, dy),
                    by_hand,
                ))
            })
            .collect();
        scene.net.restore(saved);
    }

    /// Where the current scene's moved nodes rest, by commit (see
    /// [`parterre_core::physics::Net::rest_offsets`]).
    fn rest_offsets(&self) -> Option<std::collections::HashMap<String, (f32, f32, bool)>> {
        let scene = self.scene.as_ref()?;
        let offsets = scene
            .net
            .rest_offsets()
            .map(|(node, d, by_hand)| {
                (
                    scene
                        .repo
                        .commit(scene.graph.nodes[node].commit)
                        .oid
                        .to_hex(),
                    (d.x, d.y, by_hand),
                )
            })
            .collect();
        Some(offsets)
    }

    /// Records where the current scene's nodes rest, for this repository.
    fn record_moves(&mut self) {
        if !self.settings.remember_moves {
            return;
        }
        let Some(offsets) = self.rest_offsets() else {
            return;
        };
        let Some(key) = self.repo_key() else { return };
        if offsets.is_empty() {
            self.moves.remove(&key);
        } else {
            self.moves.insert(key, offsets);
        }
    }

    pub fn is_laying_out(&self) -> bool {
        self.job.is_some()
    }

    /// A commit near the middle of the view, with its screen position, for keeping the view
    /// stable across rebuilds.
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
        let oid = scene.repo.commit(scene.graph.nodes[node].commit).oid;
        Some((
            oid,
            self.view.to_screen(self.canvas, scene.node_center(node)),
        ))
    }

    fn selected_commit(&self) -> Option<Oid> {
        let scene = self.scene.as_ref()?;
        Some(
            scene
                .repo
                .commit(scene.graph.nodes[self.selection.current()?].commit)
                .oid,
        )
    }

    /// Commits of the selected nodes, the current node last.
    fn selected_commits(&self) -> Vec<Oid> {
        let Some(scene) = &self.scene else {
            return Vec::new();
        };
        self.selection
            .nodes
            .iter()
            .map(|&n| scene.repo.commit(scene.graph.nodes[n].commit).oid)
            .collect()
    }

    /// Child and parent commit of the selected edge.
    fn selected_edge_commits(&self) -> Option<(Oid, Oid)> {
        let scene = self.scene.as_ref()?;
        let edge = scene.graph.edges[self.selected_edge?];
        let oid = |node: u32| {
            scene
                .repo
                .commit(scene.graph.nodes[node as usize].commit)
                .oid
        };
        Some((oid(edge.child), oid(edge.parent)))
    }

    /// The save dialog for exporting the whole graph in `format`, as TortoiseGit's "Save graph
    /// as" does; `None` if there is nothing to export yet.
    fn export_dialog(
        &mut self,
        format: Format,
        frame: &eframe::Frame,
    ) -> Option<rfd::AsyncFileDialog> {
        let (Some(repo), Some(_)) = (&self.repo, &self.scene) else {
            self.status = Some(("Nothing to export yet".into(), true));
            return None;
        };
        let ext = format.extension();
        let kind = format.name();
        let mut dialog = rfd::AsyncFileDialog::new()
            .set_title(format!("Export the graph as {kind}"))
            .set_parent(frame)
            .add_filter(format!("{kind} image"), &[ext])
            .set_file_name(format!("{}.{ext}", repo.display_name()));
        // Start where the last export went, or next to the repository.
        if let Some(dir) = self.export_dir.as_deref().or_else(|| repo.path.parent()) {
            dialog = dialog.set_directory(dir);
        }
        Some(dialog)
    }

    /// Writes the whole graph to `path` in `format`. PNG is drawn at the current zoom, as in
    /// TortoiseGit, and with the display's pixels per point, so it looks as on screen.
    fn export(&mut self, format: Format, mut path: PathBuf, ctx: &egui::Context) {
        let Some(scene) = &self.scene else { return };
        // A name typed without the extension, or with another one, gets it added.
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
        let palette = Palette::new(
            ctx.global_style().visuals.dark_mode,
            &self.settings.branch_colors,
        );
        // The graph's size on screen leaves the text size out.
        let ppp = ctx.native_pixels_per_point().unwrap_or(1.0);
        self.status = Some(
            match export::write(&path, scene, &self.settings, &palette, zoom, ppp) {
                Ok(what) => (format!("Saved {} ({what})", path.display()), false),
                Err(e) => (format!("Could not save {}: {e}", path.display()), true),
            },
        );
    }

    /// Opens the repository containing `dir` in place of the one shown. On failure the one
    /// shown stays, and the status bar says why.
    fn open_folder(&mut self, dir: &Path) {
        match parterre_core::git::load_repo(dir) {
            Ok(repo) => {
                self.recent.add(&repo.path);
                self.show_repo(Some(repo));
            }
            Err(e) => {
                // A recent folder that is gone, or no longer a repository, leaves the list.
                self.recent.remove(dir);
                self.status = Some((format!("Could not open {}: {e}", dir.display()), true));
            }
        }
    }

    fn close_folder(&mut self) {
        self.show_repo(None);
    }

    /// Replaces the repository shown, and forgets everything about the old one.
    fn show_repo(&mut self, repo: Option<Repo>) {
        self.repo = repo.map(Arc::new);
        self.scene = None;
        self.requested = None;
        self.job = None;
        self.needs_initial_view = true;
        self.hovered = None;
        self.hovered_edge = None;
        self.selection = Selection::default();
        self.selected_edge = None;
        self.preview = None;
        self.context_node = None;
        self.pending_select.clear();
        self.drag = None;
        self.search.hits.clear();
        self.search.current = None;
        self.status = None;
        self.export = None;
        // Its worker thread asks the old repository's git; dropping it ends the thread.
        self.details = Details::default();
        // The log and the diffs show the old repository's history.
        self.log.close();
        self.diffs.close_all();
        self.blames.close_all();
        self.compare.close();
        self.marked = None;
    }

    /// The folder picker for opening a repository.
    fn folder_dialog(&self, frame: &eframe::Frame) -> rfd::AsyncFileDialog {
        let mut dialog = rfd::AsyncFileDialog::new()
            .set_title("Open a git repository")
            .set_parent(frame);
        // Start next to the repository shown, or the one opened last.
        let near = self
            .repo
            .as_ref()
            .map(|r| r.path.as_path())
            .or_else(|| self.recent.iter().next());
        if let Some(dir) = near.and_then(Path::parent) {
            dialog = dialog.set_directory(dir);
        }
        dialog
    }

    /// Opens a requested file dialog, and acts on the answer of the one that was open. Only
    /// one is open at a time; requests made meanwhile are dropped.
    fn file_dialogs(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let (folder, export) = (std::mem::take(&mut self.pick_folder), self.export.take());
        if let Some(pending) = &self.file_dialog {
            let Some(answer) = pending.answer() else {
                return;
            };
            let what = pending.what;
            self.file_dialog = None;
            match (what, answer) {
                (Picked::Folder, Some(dir)) => self.open_folder(&dir),
                (Picked::Export(format), Some(path)) => self.export(format, path, ctx),
                (_, None) => {}
            }
            return;
        }
        if folder {
            let dialog = self.folder_dialog(frame).pick_folder();
            self.file_dialog = Some(Pending::start(Picked::Folder, dialog, ctx));
        } else if let Some(format) = export
            && let Some(dialog) = self.export_dialog(format, frame)
        {
            let what = Picked::Export(format);
            self.file_dialog = Some(Pending::start(what, dialog.save_file(), ctx));
        }
    }

    fn reload(&mut self) {
        let Some(path) = self.repo.as_ref().map(|r| r.path.clone()) else {
            return;
        };
        self.refresh_pull_requests = true;
        match parterre_core::git::load_repo(&path) {
            Ok(repo) => {
                self.install_reloaded(repo, "Reloaded");
                // Anything the watcher has loaded meanwhile may be older than this.
                self.watcher = None;
            }
            Err(e) => self.status = Some((format!("Reload failed: {e}"), true)),
        }
    }

    /// Shows a newer snapshot of the same repository. The scene on screen keeps its own
    /// snapshot until the new layout replaces it; the selection and moved nodes are carried
    /// over by commit id.
    fn install_reloaded(&mut self, repo: Repo, status: &str) {
        self.pending_select = self.selected_commits();
        if !self.settings.remember_moves {
            self.carried_moves = self.rest_offsets();
        }
        let repo = Arc::new(repo);
        self.pull_requests.refs_changed();
        self.log.reload(&repo);
        self.compare.reload(&repo);
        self.repo = Some(repo);
        if let Some((oid, _)) = self.marked {
            self.marked = Some((oid, self.commit_label(oid)));
        }
        self.requested = None;
        self.status = Some((status.into(), false));
    }

    /// Starts or stops watching the repository shown, and shows what the watcher loaded once
    /// no drag is going on.
    fn auto_reload(&mut self, ctx: &egui::Context) {
        let path = match &self.repo {
            Some(repo) if self.settings.auto_reload => repo.path.clone(),
            _ => {
                self.watcher = None;
                return;
            }
        };
        if self.watcher.as_ref().is_none_or(|w| w.path() != path) {
            self.watcher = Some(auto_reload::Watcher::start(&path, ctx));
        }
        if self.drag.is_some() {
            return;
        }
        let Some(repo) = self.watcher.as_ref().and_then(auto_reload::Watcher::take) else {
            return;
        };
        if !self
            .repo
            .as_ref()
            .is_some_and(|shown| shown.same_refs(&repo))
        {
            self.install_reloaded(repo, "Reloaded: the refs changed");
        }
    }

    /// Loads the open pull requests while they are shown, and shows them once they are in.
    fn update_pull_requests(&mut self, ctx: &egui::Context) {
        let loader = &mut self.pull_requests;
        loader.follow(self.repo.as_ref().map(|r| r.path.as_path()));
        // Turned on in the settings or the menu (the toolbar asks by itself). On by default
        // they are loaded without a word, whatever comes of it.
        let setting = self.settings.graph.show_pull_requests;
        if setting && !self.pull_requests_setting {
            loader.ask();
        }
        self.pull_requests_setting = setting;
        if std::mem::take(&mut self.refresh_pull_requests) {
            loader.refresh();
        }
        let wanted = setting && loader.origin().is_some();
        match loader.update(wanted, ctx) {
            Some(pull_requests::Loaded::Found { count, asked }) => {
                // Those whose head isn't here (another fork's, or pushed since the last fetch)
                // can't be shown.
                let here = match (&self.repo, loader.list()) {
                    (Some(repo), Some(list)) => list.heads(repo).len(),
                    _ => 0,
                };
                if asked {
                    self.status = Some((pull_requests::loaded_status(count, here), false));
                }
                // Lay out again, with them.
                self.requested = None;
            }
            // Only the user's own request gets an answer: loads parterre makes by itself fail
            // quietly, and the button's tooltip says why.
            Some(pull_requests::Loaded::Failed { error, asked: true }) => {
                self.pull_requests_error = Some(error);
            }
            Some(pull_requests::Loaded::Failed { asked: false, .. }) | None => {}
        }
    }

    /// Pull requests are on, `origin` is on GitHub, and `gh` isn't known to be missing or
    /// signed out: what the toolbar shows as on.
    pub(super) fn pull_requests_active(&self) -> bool {
        self.settings.graph.show_pull_requests
            && self.pull_requests.origin().is_some()
            && !self.pull_requests.needs_sign_in()
    }

    /// Turns pull requests off if they are active, else on, asking GitHub now.
    pub(super) fn toggle_pull_requests(&mut self) {
        if self.pull_requests_active() {
            self.settings.graph.show_pull_requests = false;
        } else {
            self.settings.graph.show_pull_requests = true;
            self.pull_requests.ask();
        }
    }

    /// Why pull requests the user asked for couldn't be loaded, and what to do about it.
    fn pull_requests_dialog(&mut self, ctx: &egui::Context) {
        let Some(error) = &self.pull_requests_error else {
            return;
        };
        match pull_requests::dialog(ctx, error) {
            pull_requests::DialogAnswer::Open => {}
            pull_requests::DialogAnswer::Close => self.pull_requests_error = None,
            pull_requests::DialogAnswer::Install => {
                if let Err(e) = crate::browser::open(GH_INSTALL) {
                    self.status = Some((e, true));
                }
            }
        }
    }

    fn update_search(&mut self) {
        self.search.hits.clear();
        self.search.current = None;
        let q = self.search.query.trim().to_lowercase();
        let Some(scene) = &self.scene else { return };
        if q.is_empty() {
            return;
        }
        for (i, node) in scene.graph.nodes.iter().enumerate() {
            let commit = scene.repo.commit(node.commit);
            let matches = commit.oid.to_hex().starts_with(&q)
                || node
                    .refs
                    .iter()
                    .any(|&r| scene.repo.refs[r].name.to_lowercase().contains(&q))
                || commit.subject.to_lowercase().contains(&q)
                || commit.author_name.to_lowercase().contains(&q);
            if matches {
                self.search.hits.push(i);
            }
        }
    }

    fn goto_search_hit(&mut self, forward: bool) {
        let n = self.search.hits.len();
        if n == 0 {
            return;
        }
        let next = match self.search.current {
            None => 0,
            Some(c) if forward => (c + 1) % n,
            Some(c) => (c + n - 1) % n,
        };
        self.search.current = Some(next);
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
        if let Some(head) = self.scene.as_ref().and_then(Scene::head_node) {
            self.view.zoom = self.view.zoom.max(0.6);
            // TortoiseGit scrolls HEAD to the top of the window after loading.
            let (world, fraction) = {
                let scene = self.scene.as_ref().unwrap();
                let fraction = match self.settings.layout.direction {
                    Direction::NewestTop => vec2(0.5, 0.12),
                    Direction::NewestBottom => vec2(0.5, 0.88),
                    Direction::NewestLeft => vec2(0.12, 0.5),
                    Direction::NewestRight => vec2(0.88, 0.5),
                };
                (scene.node_center(head), fraction)
            };
            self.view.show_at(self.canvas, world, fraction);
        } else {
            self.fit();
        }
    }

    fn fit(&mut self) {
        if let Some(scene) = &self.scene {
            self.view.fit(self.canvas, scene.bounds(), 1.0);
        }
    }

    /// Changes the arrangement of the nodes (reset, undo, …) and remembers the result. Not
    /// while nodes are being dragged.
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
        self.rearrange(|net| net.reset());
    }

    fn undo(&mut self) {
        self.rearrange(|net| {
            net.undo();
        });
    }

    fn redo(&mut self) {
        self.rearrange(|net| {
            net.redo();
        });
    }

    fn return_to_layout(&mut self, nodes: &[usize]) {
        self.rearrange(|net| net.return_to_layout(nodes));
    }

    /// Selects the nodes growing out of `roots` (see [`DragModel::Subtree`]).
    fn select_subtree(&mut self, roots: &[usize]) {
        if let Some(scene) = &self.scene {
            self.selection.extend(scene.graph.subtree(roots));
        }
    }

    fn set_drag_model(&mut self, model: DragModel) {
        self.settings.net.model = model;
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("search")));
            }
            return;
        }
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k) && !i.modifiers.command);
        let command = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, k));
        if command(Key::O) {
            self.pick_folder = true;
        }
        if command(Key::W) {
            self.close_folder();
        }
        if command(Key::F) {
            self.search.request_focus = true;
        }
        if command(Key::Comma) {
            self.open_settings(self.settings_page);
        }
        if command(Key::C) {
            self.copy_selected_hash(ctx);
        }
        // Most specific first: Ctrl+Z also matches Ctrl+Shift+Z.
        let redo = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z));
        if redo || command(Key::Y) {
            self.redo();
        }
        if command(Key::Z) {
            self.undo();
        }
        for (key, model) in [Key::Num1, Key::Num2, Key::Num3]
            .into_iter()
            .zip(DragModel::ALL)
        {
            if pressed(key) {
                self.set_drag_model(model);
            }
        }
        if command(Key::Num0) || pressed(Key::Num0) {
            self.view
                .zoom_around(self.canvas, self.canvas.center(), 1.0 / self.view.zoom);
        }
        if pressed(Key::F5) {
            self.reload();
        }
        // One node gives its log, two the range between them; three or more nothing.
        if pressed(Key::L) {
            let nodes = self.selection.nodes.clone();
            self.show_log(&nodes);
        }
        if pressed(Key::F) {
            self.fit();
        }
        if pressed(Key::Home) || pressed(Key::H) {
            self.go_to_head();
        }
        if pressed(Key::R) {
            self.reset_positions();
        }
        if pressed(Key::Plus) || pressed(Key::Equals) || command(Key::Plus) || command(Key::Equals)
        {
            self.view
                .zoom_around(self.canvas, self.canvas.center(), 1.0 / 0.8);
        }
        if pressed(Key::Minus) || command(Key::Minus) {
            self.view
                .zoom_around(self.canvas, self.canvas.center(), 0.8);
        }
        if pressed(Key::Escape) {
            self.selection.set(None);
            self.selected_edge = None;
        }
        if pressed(Key::F3) || pressed(Key::N) {
            let back = ctx.input(|i| i.modifiers.shift);
            self.goto_search_hit(!back);
        }
        let step = 60.0;
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
            if pressed(key) {
                self.view.pan_screen(delta);
            }
        }
    }

    fn copy_selected_hash(&self, ctx: &egui::Context) {
        if let Some(oid) = self.selected_commit() {
            ctx.copy_text(oid.to_hex());
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if self.is_laying_out() {
                ui.spinner();
                ui.label("Laying out…");
                ui.separator();
            }
            if self.pull_requests.is_loading() {
                ui.spinner();
                ui.label("Loading pull requests…");
                ui.separator();
            }
            if let Some(scene) = &self.scene {
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
                    if let Some((msg, error)) = &self.status {
                        ui.separator();
                        let color = if *error {
                            Color32::RED
                        } else {
                            ui.visuals().weak_text_color()
                        };
                        ui.colored_label(color, msg);
                    }
                });
            } else if let Some((msg, error)) = &self.status {
                // No repository open: only the message, such as why one could not be opened.
                let color = if *error {
                    Color32::RED
                } else {
                    ui.visuals().weak_text_color()
                };
                ui.colored_label(color, msg);
            }
        });
    }

    /// In place of the graph while no repository is open: what to do, centred, with the
    /// recent folders as a shortcut.
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
            if ui
                .add(egui::Button::new("Open folder…").min_size(vec2(140.0, 30.0)))
                .on_hover_text("Any folder inside the repository (Ctrl+O)")
                .clicked()
            {
                self.pick_folder = true;
            }
            if let Some((msg, true)) = &self.status {
                ui.add_space(10.0);
                ui.colored_label(Color32::RED, msg);
            }
            if !self.recent.is_empty() {
                ui.add_space(24.0);
                ui.label(RichText::new("Recent").color(ui.visuals().weak_text_color()));
                ui.add_space(2.0);
                let (link, weak) = (ui.visuals().hyperlink_color, ui.visuals().weak_text_color());
                for path in self.recent.iter().take(WELCOME_RECENT) {
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
            ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.canvas = canvas;
        // The overview map, drawn over the graph, counts as the graph.
        self.graph_hovered = ui.rect_contains_pointer(canvas);
        if self.needs_initial_view && canvas.is_positive() && self.scene.is_some() {
            self.needs_initial_view = false;
            if self.automation.fit {
                self.fit();
            } else {
                self.view.zoom = 1.0;
                self.go_to_head();
            }
        }
        let Some(scene) = &mut self.scene else { return };

        // Hover.
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

        // Dragging: nodes (with their selection) follow the pointer, the background pans, and
        // Shift- or Ctrl-dragging the background selects.
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
                    let nodes = dragged_with(&self.selection, n);
                    let carried = scene.carried_nodes(&nodes, model);
                    scene.net.grab(n, &nodes, &carried, model.adapts());
                    Some(Drag::Node {
                        grab: world - scene.node_center(n),
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
                Some(Drag::Node { grab }) => {
                    if let Some(p) = pointer {
                        scene.net.drag_to(to_point(p - *grab));
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
        // Clicks: select a node; Ctrl toggles it, Shift adds it. A plain click on a pull
        // request's number also opens it, once for a double-click.
        if response.clicked() {
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
            // Clicking an edge keeps it highlighted; clicking it again, or anything else, lets
            // go of it.
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
        // Double-clicking a node opens its log and selects it alone; the background fits.
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

        // Wheel: scroll; Ctrl+wheel or pinch: zoom around the pointer.
        if response.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
            if zoom != 1.0 {
                self.view
                    .zoom_around(canvas, pointer.unwrap_or(canvas.center()), zoom);
            } else if scroll != Vec2::ZERO {
                self.view.pan_screen(scroll);
            }
        }
        // However the view was panned or zoomed this frame (keys, toolbar, pointer), keep the
        // graph in sight, before drawing it: drawn beyond the limit and pulled back on the
        // next frame, it would jitter.
        self.view
            .keep_in_sight(canvas, scene.bounds(), self.view_before);

        // Physics.
        let dt = self
            .automation
            .fixed_dt()
            .unwrap_or_else(|| ui.input(|i| i.stable_dt));
        if scene.net.step(dt, &self.settings.net) {
            ui.ctx().request_repaint();
        }

        let palette = palette_for(ui, &self.settings);
        let count = scene.node_count();
        let mut hits = vec![false; count];
        for &h in &self.search.hits {
            hits[h] = true;
        }
        let mut selected = self.selection.mask(count);
        if let Some(band) = band {
            for node in scene.nodes_in(band) {
                selected[node] = true;
            }
        }
        // In Subtree mode, show what a drag would move.
        let mut preview = vec![false; count];
        match (self.hovered, self.drag, self.settings.net.model) {
            (Some(n), None, DragModel::Subtree) => {
                // As a drag would: Shift or Ctrl adds the node to the selection.
                let mut roots = dragged_with(&self.selection, n);
                if extend && !self.selection.contains(n) {
                    roots = self.selection.nodes.clone();
                    roots.push(n);
                }
                if self.preview.as_ref().is_none_or(|(r, _)| *r != roots) {
                    let nodes = scene.carried_nodes(&roots, DragModel::Subtree);
                    self.preview = Some((roots, nodes));
                }
                if let Some((_, nodes)) = &self.preview {
                    for &node in nodes {
                        preview[node] = true;
                    }
                }
            }
            _ => self.preview = None,
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

        // Tooltip for the hovered pull request, or node.
        if let Some(i) = hovered_pull_request {
            let pr = &scene.pull_requests[i];
            response.clone().on_hover_ui_at_pointer(|ui| {
                ui.label(RichText::new(format!("#{} {}", pr.number, pr.title)).strong());
                // The author is missing if their account was deleted.
                let draft = pr.draft.then(|| "Draft".to_owned());
                let author = (!pr.author.is_empty()).then(|| format!("by {}", pr.author));
                let about: Vec<String> = draft.into_iter().chain(author).collect();
                if !about.is_empty() {
                    ui.label(about.join(" · "));
                }
                ui.label(
                    RichText::new(format!("{} into {}", pr.head_label(), pr.base_branch))
                        .monospace(),
                );
                ui.add_space(4.0);
                ui.label(RichText::new("Click to open it on GitHub").weak());
            });
        } else if let (Some(node), None) = (self.hovered, self.drag) {
            let n = &scene.graph.nodes[node];
            let commit = scene.repo.commit(n.commit);
            let hidden: u32 = scene
                .graph
                .edges
                .iter()
                .filter(|e| e.child as usize == node && e.first_parent)
                .map(|e| e.hidden)
                .sum();
            let refs: Vec<&str> = n
                .refs
                .iter()
                .map(|&r| scene.repo.refs[r].full_name.as_str())
                .collect();
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
            let details = &mut self.details;
            let repo_path = &scene.repo.path;
            let ctx = ui.ctx().clone();
            response.clone().on_hover_ui_at_pointer(|ui| {
                ui.monospace(commit.oid.to_hex());
                ui.label(format!(
                    "{} <{}>  {}",
                    commit.author_name, commit.author_email, commit.author_date
                ));
                ui.add_space(4.0);
                ui.label(RichText::new(&commit.subject).strong());
                match details.message(repo_path, commit.oid, &ctx) {
                    Some(message) => {
                        let body = message.split_once('\n').map_or("", |(_, b)| b.trim());
                        if !body.is_empty() {
                            // TortoiseGit truncates at 8000 characters.
                            let body: String = body.lines().take(40).collect::<Vec<_>>().join("\n");
                            ui.label(body.chars().take(4000).collect::<String>());
                        }
                    }
                    None => {
                        ui.spinner();
                    }
                }
                if !refs.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(refs.join("\n")).weak());
                }
                if !worktrees.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(worktrees.join("\n")).weak());
                }
                if hidden > 0 {
                    ui.label(RichText::new(format!("{hidden} commits collapsed below")).weak());
                }
            });
        }

        // Tooltip for the hovered edge: the commits collapsed into it.
        if let (Some(e), None) = (self.hovered_edge, self.drag) {
            let edge = scene.graph.edges[e];
            let hidden = scene.graph.collapsed_commits(&scene.repo, edge, 12);
            response.clone().on_hover_ui_at_pointer(|ui| {
                ui.label(edge_summary(scene, edge));
                if edge.hidden == 0 {
                    ui.label(RichText::new("direct parent").weak());
                    return;
                }
                ui.label(RichText::new(format!("{} commits collapsed:", edge.hidden)).strong());
                for c in &hidden {
                    let commit = scene.repo.commit(*c);
                    ui.horizontal(|ui| {
                        ui.monospace(commit.oid.short(scene.repo.abbrev_len));
                        ui.label(&commit.subject);
                    });
                }
                if (edge.hidden as usize) > hidden.len() {
                    ui.label(
                        RichText::new(format!(
                            "… and {} more",
                            edge.hidden as usize - hidden.len()
                        ))
                        .weak(),
                    );
                }
            });
        }

        // Context menu.
        let context_node = self.context_node;
        // A node's menu acts on the selection it belongs to.
        let group: Vec<usize> = match context_node {
            Some(n) if self.selection.contains(n) => self.selection.nodes.clone(),
            Some(n) => vec![n],
            None => Vec::new(),
        };
        let item = |text: &str, shortcut: &str| egui::Button::new(text).shortcut_text(shortcut);
        let marked = self.marked.clone();
        // The scene is borrowed: `pull_requests_active`, field by field.
        let pull_requests_shown = self.settings.graph.show_pull_requests
            && self.pull_requests.origin().is_some()
            && !self.pull_requests.needs_sign_in()
            && self.pull_requests.list().is_some();
        let worktrees_shown = self.settings.graph.show_worktrees;
        egui::Popup::context_menu(&response)
            .style(menu::style)
            .show(|ui| {
                menu::fit_window(ui, |ui| {
                    ui.set_min_width(menu::MIN_WIDTH);
                    let Some(node) = context_node else {
                        if ui.add(item("Fit graph", "F")).clicked() {
                            action = Some(MenuAction::Fit);
                            ui.close();
                        }
                        if ui.add(item("Return all nodes to layout", "R")).clicked() {
                            action = Some(MenuAction::ResetAll);
                            ui.close();
                        }
                        return;
                    };
                    // Greyed out rather than left out, so the menu keeps its shape.
                    let show_log = ui
                        .add_enabled(group.len() <= 2, item("Show log", "L"))
                        .on_disabled_hover_text("Select one or two nodes");
                    if show_log.clicked() {
                        action = Some(MenuAction::ShowLog(group.clone()));
                        ui.close();
                    }
                    let n = &scene.graph.nodes[node];
                    let oid_of = |n: usize| scene.repo.commit(scene.graph.nodes[n].commit).oid;
                    let oid = oid_of(node);
                    menu::plain_submenu(ui, "Compare", |ui| {
                        // Two nodes: with each other, in selection order; one: with HEAD.
                        let head = scene.repo.head_commit().map(|c| scene.repo.commit(c).oid);
                        let (label, pair) = match *group.as_slice() {
                            [a, b] => ("Selected revisions", Some((oid_of(a), oid_of(b)))),
                            [_] => ("HEAD", head.filter(|&h| h != oid).map(|h| (oid, h))),
                            _ => ("Selected revisions", None),
                        };
                        let why = if group.len() > 2 {
                            "Select one or two nodes"
                        } else {
                            "This is HEAD"
                        };
                        let compare = ui
                            .add_enabled(pair.is_some(), egui::Button::new(label))
                            .on_disabled_hover_text(why);
                        if compare.clicked()
                            && let Some((a, b)) = pair
                        {
                            action = Some(MenuAction::Compare(CompareRequest::Compare(a, b)));
                            ui.close();
                        }
                        let working_tree = scene.repo.has_working_tree;
                        let why = if !working_tree {
                            "A bare repository has no working tree"
                        } else {
                            "Select one node"
                        };
                        let with_working_tree = ui
                            .add_enabled(
                                group.len() == 1 && working_tree,
                                egui::Button::new("Working tree"),
                            )
                            .on_disabled_hover_text(why);
                        if with_working_tree.clicked() {
                            action = Some(MenuAction::Compare(CompareRequest::WorkingTree(oid)));
                            ui.close();
                        }
                        menu::separator(ui);
                        let is_marked = marked.as_ref().is_some_and(|(m, _)| *m == oid);
                        let (text, mark) = if is_marked {
                            ("Clear the mark", None)
                        } else {
                            ("Mark for comparison", Some(oid))
                        };
                        let mark_item = ui
                            .add_enabled(group.len() == 1, egui::Button::new(text))
                            .on_disabled_hover_text("Select one node");
                        if mark_item.clicked() {
                            action = Some(MenuAction::Compare(CompareRequest::Mark(mark)));
                            ui.close();
                        }
                        let other = marked
                            .as_ref()
                            .filter(|(m, _)| *m != oid && group.len() == 1);
                        let label = other.map_or("Compare with marked".to_owned(), |(_, name)| {
                            format!("Compare with marked ({name})")
                        });
                        let why = if group.len() > 1 {
                            "Select one node"
                        } else if is_marked {
                            "This is the marked commit"
                        } else {
                            "Mark a commit for comparison first"
                        };
                        let with_marked = ui
                            .add_enabled(other.is_some(), egui::Button::new(label))
                            .on_disabled_hover_text(why);
                        if with_marked.clicked()
                            && let Some(&(m, _)) = other
                        {
                            action = Some(MenuAction::Compare(CompareRequest::Compare(m, oid)));
                            ui.close();
                        }
                    });
                    if let Some(request) = branches::node_menu(
                        ui,
                        &scene.repo,
                        oid,
                        self.branches.catalog.as_deref(),
                        self.branches.busy(),
                    ) {
                        action = Some(MenuAction::Branch(request));
                    }
                    // The worktrees shown on the node, the open one at HEAD among them.
                    let worktrees: Vec<&parterre_core::Worktree> = scene
                        .worktrees_on(node)
                        .into_iter()
                        .map(|k| &scene.repo.worktrees[k])
                        .collect();
                    menu::separator(ui);
                    // Greyed out rather than left out, so the menu keeps its shape.
                    if !(pull_requests_shown || worktrees_shown) {
                        ui.add_enabled(false, egui::Button::new("Open"))
                            .on_disabled_hover_text("Show pull requests or worktrees to open them");
                    } else {
                        menu::plain_submenu(ui, "Open", |ui| {
                            if pull_requests_shown {
                                let mut open_item = |ui: &mut Ui, label: &str, i: usize| {
                                    let pr = &scene.pull_requests[i];
                                    if ui.button(label).on_hover_text(&pr.title).clicked() {
                                        action = Some(MenuAction::OpenPullRequest(pr.url.clone()));
                                        ui.close();
                                    }
                                };
                                let number = |i: usize| scene.pull_requests[i].number;
                                match n.pull_requests.as_slice() {
                                    [] => {
                                        ui.add_enabled(false, egui::Button::new("Pull request"))
                                            .on_disabled_hover_text(
                                                "No open pull request's head is here",
                                            );
                                    }
                                    &[i] => {
                                        open_item(ui, &format!("Pull request #{}", number(i)), i)
                                    }
                                    several => menu::plain_submenu(ui, "Pull request", |ui| {
                                        for &i in several {
                                            open_item(ui, &format!("#{}", number(i)), i);
                                        }
                                    }),
                                }
                            }
                            if !worktrees_shown {
                                return;
                            }
                            for (what, opener) in [
                                ("File system", Opener::FileManager),
                                ("Terminal", Opener::Terminal),
                            ] {
                                // One worktree: the item opens it. Several: a submenu names them.
                                let mut open_item =
                                    |ui: &mut Ui, label: &str, w: &parterre_core::Worktree| {
                                        let path = w.path.display().to_string();
                                        let open = ui
                                            .add_enabled(!w.missing, egui::Button::new(label))
                                            .on_hover_text(&path)
                                            .on_disabled_hover_text(format!(
                                                "The folder is gone: {path}"
                                            ));
                                        if open.clicked() {
                                            action = Some(MenuAction::Open(opener, w.path.clone()));
                                            ui.close();
                                        }
                                    };
                                match worktrees.as_slice() {
                                    [] => {
                                        ui.add_enabled(false, egui::Button::new(what))
                                            .on_disabled_hover_text(
                                                "No worktree is checked out here",
                                            );
                                    }
                                    [w] => open_item(ui, what, w),
                                    several => menu::plain_submenu(ui, what, |ui| {
                                        for w in several {
                                            open_item(ui, &w.name(), w);
                                        }
                                    }),
                                }
                            }
                        });
                    }
                    let commit = scene.repo.commit(n.commit);
                    menu::plain_submenu(ui, "Copy", |ui| {
                        // Right-clicking selects the node, so Ctrl+C would copy the same hash.
                        let copy_hash = if group.len() > 1 { "" } else { "Ctrl+C" };
                        if ui.add(item("Commit hash", copy_hash)).clicked() {
                            ui.ctx().copy_text(commit.oid.to_hex());
                            ui.close();
                        }
                        if ui.button("Ref names").clicked() {
                            let names: Vec<&str> = n
                                .refs
                                .iter()
                                .map(|&r| scene.repo.refs[r].full_name.as_str())
                                .collect();
                            let text = if names.is_empty() {
                                commit.oid.to_hex()
                            } else {
                                names.join("\n")
                            };
                            ui.ctx().copy_text(text);
                            ui.close();
                        }
                        let copy_path = |ui: &mut Ui, label: &str, w: &parterre_core::Worktree| {
                            let path = w.path.display().to_string();
                            if ui.button(label).on_hover_text(&path).clicked() {
                                ui.ctx().copy_text(path);
                                ui.close();
                            }
                        };
                        match worktrees.as_slice() {
                            [] => {}
                            [w] => copy_path(ui, "Folder path", w),
                            several => menu::plain_submenu(ui, "Folder path", |ui| {
                                for w in several {
                                    copy_path(ui, &w.name(), w);
                                }
                            }),
                        }
                    });
                    menu::separator(ui);
                    if ui
                        .button("Select subtree")
                        .on_hover_text(
                            "Select everything that grows out of this (first-parent descendants)",
                        )
                        .clicked()
                    {
                        action = Some(MenuAction::SelectSubtree(group.clone()));
                        ui.close();
                    }
                    let displaced: Vec<usize> = group
                        .iter()
                        .copied()
                        .filter(|&n| scene.net.is_displaced(n))
                        .collect();
                    let label = if group.len() > 1 {
                        "Return selection to layout"
                    } else {
                        "Return node to layout"
                    };
                    // Greyed out rather than left out, so the menu keeps its shape.
                    if ui
                        .add_enabled(!displaced.is_empty(), egui::Button::new(label))
                        .clicked()
                    {
                        action = Some(MenuAction::ReturnToLayout(displaced));
                        ui.close();
                    }
                    if ui.button("Centre view here").clicked() {
                        action = Some(MenuAction::Center(node));
                        ui.close();
                    }
                });
            });
        match action {
            Some(MenuAction::Fit) => self.fit(),
            Some(MenuAction::ResetAll) => self.reset_positions(),
            Some(MenuAction::ReturnToLayout(nodes)) => self.return_to_layout(&nodes),
            Some(MenuAction::SelectSubtree(roots)) => self.select_subtree(&roots),
            Some(MenuAction::Center(node)) => self.center_on(node),
            Some(MenuAction::ShowLog(nodes)) => self.show_log(&nodes),
            Some(MenuAction::Compare(request)) => self.compare_request(request),
            Some(MenuAction::Branch(request)) => self.branches.request(&response.ctx, request),
            Some(MenuAction::OpenPullRequest(url)) => {
                if let Err(e) = crate::browser::open(&url) {
                    self.status = Some((e, true));
                }
            }
            Some(MenuAction::Open(opener, dir)) => {
                let opened = match opener {
                    Opener::FileManager => crate::file_manager::open(&dir),
                    Opener::Terminal => crate::file_manager::open_terminal(&dir),
                };
                if let Err(e) = opened {
                    self.status = Some((e, true));
                }
            }
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
        // TortoiseGit: max(100, w/4) x max(200, h/4) in the bottom-right corner.
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
        let resp = ui.interact(rect, egui::Id::new("overview"), Sense::click_and_drag());
        if (resp.clicked() || resp.dragged())
            && let Some(p) = resp.interact_pointer_pos()
        {
            let target = world.center() + (p - rect.center()) / scale;
            self.view.show_at(canvas, target, vec2(0.5, 0.5));
        }
    }

    fn legend_window(&mut self, ctx: &egui::Context) {
        let palette = Palette::new(
            ctx.global_style().visuals.dark_mode,
            &self.settings.branch_colors,
        );
        let mut open_colours = false;
        egui::Window::new("Legend")
            .open(&mut self.show_legend)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                let swatch = |ui: &mut Ui, fill: Color32, text: &str, what: &str| {
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(vec2(150.0, 20.0), Sense::hover());
                        ui.painter().rect_filled(rect, 4.0, fill);
                        ui.painter().text(
                            rect.left_center() + vec2(8.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            text,
                            FontId::monospace(12.0),
                            crate::theme::text_on(fill),
                        );
                        ui.label(what);
                    });
                };
                swatch(
                    ui,
                    palette.current_branch,
                    "main",
                    "Current branch (HEAD), or a detached HEAD",
                );
                swatch(ui, palette.local_branch, "feature/x", "Local branch");
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
                        "Open pull request on GitHub (click to open)",
                    ),
                    (palette.draft_pull_request, "Draft pull request"),
                ] {
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(vec2(150.0, 20.0), Sense::hover());
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
                        palette.local_branch,
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
                        let (rect, _) = ui.allocate_exact_size(vec2(150.0, 20.0), Sense::hover());
                        let text = crate::theme::text_on(fill);
                        ui.painter().rect_filled(rect, 4.0, fill);
                        // As in the graph: the name after the glyph.
                        let icon = Rect::from_center_size(
                            rect.left_center() + vec2(14.0, 0.0),
                            Vec2::splat(12.0),
                        );
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
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(150.0, 20.0), Sense::hover());
                    ui.painter().rect_filled(rect, 4.0, palette.plain_fill);
                    ui.painter().text(
                        rect.left_center() + vec2(8.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        "1a2b3c4d",
                        FontId::monospace(12.0),
                        palette.plain_text,
                    );
                    ui.label("Commit without refs (branch point or merge)");
                });
                for rule in &self.settings.branch_colors {
                    swatch(ui, rule.color, &rule.patterns, "Branches matching");
                }
                if ui.link("Branch colours…").clicked() {
                    open_colours = true;
                }
                ui.add_space(6.0);
                ui.label(
                    "Arrows point from a commit to its parents. Edges may stand for many hidden",
                );
                ui.label("commits; hover an edge to list them, click it to keep it highlighted.");
            });
        if open_colours {
            self.open_settings(SettingsPage::BranchColours);
        }
    }

    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("Keyboard and mouse")
            .open(&mut self.show_shortcuts)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::Grid::new("shortcuts").striped(true).show(ui, |ui| {
                    for (keys, what) in [
                        (
                            "Drag a node",
                            "Move it, with the rest of the selection it belongs to",
                        ),
                        (
                            "Click an edge",
                            "Keep it highlighted; click it again to let go",
                        ),
                        (
                            "1 / 2 / 3",
                            "Drag mode Adapt (the graph gives way) / Free (nothing else \
                             moves) / Subtree (take along what grows out of it)",
                        ),
                        ("Click a node", "Select it"),
                        (
                            "Click a pull request's number",
                            "Open the pull request on GitHub",
                        ),
                        (
                            "L, double-click a node",
                            "Show log: of the node, or of the range between two selected \
                             nodes (first..second)",
                        ),
                        (
                            "Ctrl+click / Shift+click",
                            "Toggle it in / add it to the selection",
                        ),
                        (
                            "Shift+drag the background",
                            "Select the nodes in a rectangle",
                        ),
                        ("Esc", "Clear the selection"),
                        ("Ctrl+Z / Ctrl+Shift+Z", "Undo / redo a move"),
                        ("R", "Return all nodes to the layout"),
                        ("Drag the background", "Pan"),
                        ("Wheel / Shift+wheel", "Scroll vertically / horizontally"),
                        ("Ctrl+wheel, pinch", "Zoom around the pointer"),
                        ("+ / - / 0", "Zoom in / out / 100%"),
                        (
                            "Ctrl+wheel off the graph",
                            "Text size of every window (Settings → Appearance); Ctrl+plus / \
                             minus / 0 in the log, diff and settings windows",
                        ),
                        ("F, double-click background", "Fit the whole graph"),
                        ("Home, H", "Go to HEAD"),
                        ("Ctrl+F", "Find; Enter / Shift+Enter for next / previous"),
                        ("F3, N", "Next search hit"),
                        ("Ctrl+C", "Copy the selected commit's hash"),
                        ("F5", "Reload the repository"),
                        ("Ctrl+O / Ctrl+W", "Open / close a folder"),
                        ("Ctrl+,", "Settings"),
                        (
                            "Right-click a node",
                            "Show log, compare (with HEAD, the working tree, two nodes, or the \
                             commit marked for comparison), open its pull requests, copy hash or refs, select \
                             its subtree, return it to the layout",
                        ),
                    ] {
                        ui.strong(keys);
                        ui.label(what);
                        ui.end_row();
                    }
                });
            });
    }

    /// The "Appropriate Legal Notices" of GPL-3.0 section 5(d). NOTICE requires works based on
    /// parterre to keep showing them.
    fn about_window(&mut self, ctx: &egui::Context) {
        // Paths chosen by build.rs.
        const NOTICE: &str = include_str!(env!("PARTERRE_NOTICE"));
        const LICENSE: &str = include_str!(env!("PARTERRE_LICENSE"));
        // Room for the title bar and the heading; the texts scroll within the rest.
        let max_height = ctx.content_rect().height() - 140.0;
        egui::Window::new("About parterre")
            .open(&mut self.show_about)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.heading(format!("parterre {}", crate::VERSION));
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(max_height)
                    .show(ui, |ui| {
                        // Both texts are wrapped at 80 columns already.
                        ui.add(egui::Label::new(RichText::new(NOTICE).monospace()).extend());
                        ui.collapsing("GNU General Public License, version 3", |ui| {
                            ui.add(egui::Label::new(RichText::new(LICENSE).monospace()).extend());
                        });
                    });
            });
    }
}

/// "Apps – parterre", or just "parterre" while no repository is open.
pub fn window_title(repo: Option<&Repo>) -> String {
    match repo {
        Some(repo) => format!("{} – parterre", repo.display_name()),
        None => "parterre".to_owned(),
    }
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

/// Which way an edge leads, e.g. "From main to its merged parent feature/x". (The default
/// fonts have no arrow glyph.)
fn edge_summary(scene: &Scene, edge: RevEdge) -> String {
    format!(
        "From {} to its {}parent {}",
        node_name(scene, edge.child),
        if edge.first_parent { "" } else { "merged " },
        node_name(scene, edge.parent)
    )
}

/// A node's first ref, or its short hash if it has none.
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

impl ParterreApp {
    /// Sets the theme of egui and of the window's title bar.
    fn apply_theme(&mut self, ctx: &egui::Context) {
        use egui::{SystemTheme as Window, Theme, ThemePreference as Egui};
        let (egui_theme, window_theme) = match self.settings.theme {
            // Where winit can't tell the system theme, egui would pick dark (and the title bar
            // would stay as it started).
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

/// Where the GitHub CLI's installation is explained.
const GH_INSTALL: &str = "https://github.com/cli/cli#installation";

/// How many recent folders the welcome screen lists; the menu has them all.
const WELCOME_RECENT: usize = 5;

fn palette_for(ui: &Ui, settings: &Settings) -> Palette {
    Palette::new(ui.visuals().dark_mode, &settings.branch_colors)
}

enum MenuAction {
    Fit,
    ResetAll,
    ReturnToLayout(Vec<usize>),
    SelectSubtree(Vec<usize>),
    Center(usize),
    ShowLog(Vec<usize>),
    /// Mark a commit for comparison, or compare two.
    Compare(CompareRequest),
    Branch(branches::Request),
    /// Open a pull request's page in the browser.
    OpenPullRequest(String),
    /// Open a worktree's folder in the file manager or a terminal.
    Open(Opener, std::path::PathBuf),
}

/// What opens a worktree's folder.
#[derive(Clone, Copy, Debug)]
enum Opener {
    FileManager,
    Terminal,
}

impl eframe::App for ParterreApp {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        // One pass paints the main window and every immediate viewport, so this paces them all.
        if let Some(limiter) = &mut self.frame_limiter {
            limiter.wait();
        }
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        self.view.text_size = ctx.zoom_factor();
        self.graph_hovered = false;
        let title = window_title(self.repo.as_deref());
        if self.title != title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
        self.auto_reload(&ctx);
        self.branches.update(&ctx, self.repo.as_ref());
        if let Some(path) = self.branches.reload.take()
            && self.repo.as_ref().is_some_and(|r| r.path == path)
        {
            self.reload();
        }
        self.update_pull_requests(&ctx);
        self.ensure_scene(&ctx);
        self.view_before = self.view;
        self.handle_keys(&ctx);

        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::side_top_panel(&ctx.global_style())
                    .inner_margin(egui::Margin::symmetric(8, 6)),
            )
            .show(ui, |ui| self.toolbar(ui));
        if self.settings.show_status_bar {
            egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        }
        if self.repo.is_some() {
            egui::CentralPanel::no_frame().show(ui, |ui| self.canvas(ui));
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
        self.log_window(&ctx);
        self.compare_window(&ctx);
        self.diff_windows(&ctx);
        self.blame_windows(&ctx);
        self.about_window(&ctx);
        self.branches.show(&ctx);
        if let Some((repo, commits, exact)) = self.branches.log_request.take() {
            if exact {
                self.open_loss_log(repo, &commits);
            } else {
                let indices: Vec<_> = commits.iter().filter_map(|oid| repo.lookup(oid)).collect();
                self.open_log(repo, &indices);
            }
        }

        // Scripted runs wait for the graph, unless there is none to wait for, and for the diffs
        // and the pull requests (and the layout with them) being loaded.
        if self.scene.is_some() || self.repo.is_none() {
            let pulling = self.pull_requests.is_loading()
                || self.pull_requests_active()
                    && self.pull_requests.list().is_some()
                    && self.job.is_some();
            self.automation.waiting =
                self.diffs.is_loading() || self.blames.is_loading() || pulling;
            self.automation.drive(
                &ctx,
                self.scene.as_mut(),
                &mut self.view,
                self.canvas,
                &self.settings.net,
            );
        }

        self.file_dialogs(&ctx, frame);
        // Takes effect from the next frame on, in every window.
        if ctx.zoom_factor() != self.settings.text_size {
            ctx.set_zoom_factor(self.settings.text_size);
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.automation.inject_input(raw_input);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.persist {
            eframe::set_value(storage, STORAGE_KEY, &self.settings);
            eframe::set_value(storage, MOVES_KEY, &self.moves);
            eframe::set_value(storage, RECENT_KEY, &self.recent);
        }
    }
}
