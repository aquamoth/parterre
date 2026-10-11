use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{self, FontId, Id, RichText, Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2};
use parterre_core::blame::BlameSpec;
use parterre_core::compare::{Comparison, WorkingTree};
use parterre_core::conflicts::Resolve;
use parterre_core::file_diff::{FileDiffSpec, Rev};
use parterre_core::glyphs;
use parterre_core::log::commit_name;
use parterre_core::revgraph::GraphOptions;
use parterre_core::{Oid, Repo};

use super::file_table::{DiffQueue, FileTable, Lister, Listing, WorkingTreePick, file_folder};
use super::log_window::{Colors, badge, badges, colors, window_input};
use super::{Opener, ParterreApp};
use crate::keys;
use crate::settings::CompareWindowSettings;
use crate::text_size;
use crate::theme::Palette;
use crate::widgets;

const SIDE_ROW_HEIGHT: f32 = 26.0;

pub(super) fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("compare")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareRequest {
    /// `None` clears the mark.
    Mark(Option<Oid>),
    /// In the order they were picked.
    Compare(Oid, Oid),
    WorkingTree(Oid),
}

#[derive(Debug)]
struct Compared {
    /// Where the left side is read from: `None` for unrelated histories' common ancestor, or on
    /// failure.
    base: Option<Rev>,
    files: Listing,
    working_tree: Option<WorkingTree>,
}

#[derive(Clone, Debug)]
pub enum FileRequest {
    MergeTool { root: PathBuf, path: String },
    Resolve(Resolve),
    Open(Opener, PathBuf),
}

#[derive(Debug, Default)]
pub struct CompareWindow {
    view: Option<CompareView>,
    table: FileTable,
    compared: Lister<Comparison, Compared>,
    /// Fixed while the window is open: egui resizes a window whose viewport builder changes.
    size: Vec2,
    title_theme: Option<egui::SystemTheme>,
    diffs: DiffQueue<(Arc<Repo>, FileDiffSpec)>,
    blame_requests: Vec<(Arc<Repo>, BlameSpec)>,
    pub requests: Vec<FileRequest>,
}

#[derive(Debug)]
struct CompareView {
    repo: Arc<Repo>,
    refs_by_commit: Vec<Vec<usize>>,
    comparison: Comparison,
    ancestor_matters: bool,
}

impl CompareView {
    fn new(repo: Arc<Repo>, comparison: Comparison) -> CompareView {
        CompareView {
            refs_by_commit: repo.refs_by_commit(),
            ancestor_matters: comparison.ancestor_matters(&repo),
            repo,
            comparison,
        }
    }
}

struct Env<'a> {
    palette: Palette,
    graph: &'a GraphOptions,
    settings: &'a mut CompareWindowSettings,
}

impl CompareWindow {
    fn open(&mut self, repo: Arc<Repo>, comparison: Comparison, size: Vec2) {
        if self.view.is_none() {
            self.size = size;
            // Column widths last as long as the window is open.
            self.table.widths = Default::default();
        }
        self.view = Some(CompareView::new(repo, comparison));
    }

    pub fn is_open(&self) -> bool {
        self.view.is_some()
    }

    pub fn close(&mut self) {
        self.view = None;
        self.compared = Lister::default();
        self.table.clear_selection();
        self.diffs.cancel();
    }

    /// The commits stay; the working tree is listed again.
    pub fn reload(&mut self, repo: &Arc<Repo>) {
        if let Some(view) = &mut self.view {
            *view = CompareView::new(repo.clone(), view.comparison);
            if view.comparison.reads_working_tree() {
                self.compared = Lister::default();
            }
        }
    }

    /// For a change to the conflicted files, which moves no refs.
    pub fn relist_working_tree(&mut self) {
        if self
            .view
            .as_ref()
            .is_some_and(|v| v.comparison.reads_working_tree())
        {
            self.compared = Lister::default();
        }
    }

    fn title(&self) -> String {
        let name = self
            .view
            .as_ref()
            .map(|v| v.repo.display_name())
            .unwrap_or_default();
        format!("{name} – Compare")
    }

    fn handle_keys(&mut self, ui: &Ui) {
        if !ui.ctx().egui_wants_keyboard_input()
            && ui.input_mut(|i| keys::close_window().consume(i))
        {
            self.view = None;
        }
    }

    fn contents(&mut self, ui: &mut Ui, env: &mut Env) {
        let c = colors(ui);
        self.header(ui, &c, env);
        self.changed_files(ui, env);
        self.diffs.confirm_many(ui, Id::new("compare-many-diffs"));
    }

    fn header(&mut self, ui: &mut Ui, c: &Colors, env: &mut Env) {
        let Some(view) = &mut self.view else { return };
        let height = 2.0 * SIDE_ROW_HEIGHT + 12.0;
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(1.0, c.line),
        );
        let mut tools = ui.new_child(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(8.0, 0.0)))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        // The working tree stays on the right, as in `git diff <commit>`.
        if !view.comparison.reads_working_tree() {
            let swap = widgets::tip_explained(
                widgets::icon_button(&mut tools, glyphs::SWAP, false),
                "Swap sides",
                "",
                "Put the right-hand side on the left, and the left-hand one on the right.",
            );
            if swap.clicked() {
                *view = CompareView::new(view.repo.clone(), view.comparison.swapped());
            }
        }
        let right = tools.min_rect().left() - 12.0;
        let sides = [("From", view.comparison.old), ("To", view.comparison.new)];
        for (row, (label, rev)) in sides.into_iter().enumerate() {
            let top = rect.top() + 6.0 + row as f32 * SIDE_ROW_HEIGHT;
            let y = top + SIDE_ROW_HEIGHT / 2.0;
            side(ui, view, env, label, rev, (rect.left() + 12.0, right), y);
        }
    }

    fn changed_files(&mut self, ui: &mut Ui, env: &mut Env) {
        let Some(view) = &mut self.view else { return };
        view.comparison.since_ancestor = env.settings.since_ancestor;
        let comparison = view.comparison;
        let listed = self
            .compared
            .get(&view.repo.path, comparison, ui.ctx(), |git, c| {
                match c.run(git) {
                    Ok(compared) => Compared {
                        base: compared.base,
                        files: Ok(compared.files),
                        working_tree: compared.working_tree,
                    },
                    Err(e) => Compared {
                        base: None,
                        files: Err(e.to_string()),
                        working_tree: None,
                    },
                }
            });
        let base = listed.and_then(|l| l.files.is_ok().then_some(l.base));
        let tree = listed.and_then(|l| l.working_tree.as_ref());
        let weak = ui.visuals().weak_text_color();
        let abbrev = view.repo.abbrev_len;
        let since = &mut env.settings.since_ancestor;
        let ancestor_matters = view.ancestor_matters;
        let action = self.table.show_working_tree(
            ui,
            "compare",
            Id::new(comparison),
            listed.map(|l| &l.files),
            tree,
            |ui| {
                if ancestor_matters {
                    ui.add_space(8.0);
                    widgets::tip_explained(
                        ui.checkbox(since, "Since common ancestor"),
                        "Since common ancestor",
                        "",
                        "Compare the right-hand side with where the two histories forked, \
                         rather than with the left-hand side: only what the right-hand side \
                         changed since then, as a pull request shows it (git diff A...B). The \
                         working tree forks where HEAD does.",
                    );
                }
                let note = match base {
                    Some(Some(Rev::Commit(base)))
                        if comparison.since_ancestor && ancestor_matters =>
                    {
                        format!("from {}", base.short(abbrev))
                    }
                    Some(None) => "No common ancestor".to_owned(),
                    _ if comparison.reads_working_tree() => {
                        format!("{} lists the files again", keys::reload_label())
                    }
                    _ => String::new(),
                };
                ui.label(RichText::new(note).size(12.0).color(weak));
            },
        );
        if let (Some((file, pick)), Some(tree)) = (action.working_tree, tree) {
            let conflict = tree.conflict(&file.path);
            self.requests.push(match pick {
                WorkingTreePick::MergeTool => FileRequest::MergeTool {
                    root: tree.root.clone(),
                    path: file.path.clone(),
                },
                WorkingTreePick::Answer(answer) => {
                    let Some(conflict) = conflict else { return };
                    FileRequest::Resolve(Resolve {
                        conflict: conflict.clone(),
                        answer,
                        item: conflict.item(&tree.sides, answer),
                    })
                }
                WorkingTreePick::Terminal => {
                    FileRequest::Open(Opener::Terminal, file_folder(&tree.root, &file.path))
                }
                WorkingTreePick::FileSystem => {
                    FileRequest::Open(Opener::FileManager, file_folder(&tree.root, &file.path))
                }
            });
        }
        if let Some(f) = action.blame {
            let spec = BlameSpec {
                rev: comparison.new,
                path: f.path.clone(),
            };
            self.blame_requests.push((view.repo.clone(), spec));
        }
        let Some(Some(base)) = base else { return };
        let open = action
            .open
            .into_iter()
            .map(|f| {
                let spec = FileDiffSpec::between(Some(base), comparison.new, f);
                (view.repo.clone(), spec)
            })
            .collect();
        self.diffs.push(open);
    }
}

fn side(
    ui: &Ui,
    view: &CompareView,
    env: &Env,
    label: &str,
    rev: Rev,
    (left, right): (f32, f32),
    y: f32,
) {
    let painter = ui.painter();
    let weak = ui.visuals().weak_text_color();
    let text = ui.visuals().text_color();
    let g = painter.layout_no_wrap(label.to_owned(), FontId::proportional(12.0), weak);
    painter.galley(pos2(left, y - g.size().y / 2.0), g, weak);
    let mut x = left + 44.0;
    let Rev::Commit(oid) = rev else {
        let mut job = LayoutJob::default();
        let font = FontId::proportional(13.5);
        job.append("Working tree", 0.0, TextFormat::simple(font.clone(), text));
        job.append(
            "files on disk, staged or not",
            10.0,
            TextFormat::simple(font, weak),
        );
        let g = painter.layout_job(job);
        painter.galley(pos2(x, y - g.size().y / 2.0), g, text);
        return;
    };
    // The hash first: it is monospace and always as long, so the two sides' hashes line up.
    let hash = painter.layout_no_wrap(
        oid.short(view.repo.abbrev_len),
        FontId::monospace(12.5),
        weak,
    );
    let hash_width = hash.size().x;
    if x + hash_width > right {
        return;
    }
    painter.galley(pos2(x, y - hash.size().y / 2.0), hash, weak);
    x += hash_width + 10.0;
    let Some(ix) = view.repo.lookup(&oid) else {
        return;
    };
    for b in badges(
        &view.repo,
        &view.refs_by_commit[ix.ix()],
        Some(ix),
        env.graph,
    ) {
        if x >= right {
            break;
        }
        x += badge(ui, &b, &env.palette, pos2(x, y), right - x) + 4.0;
    }
    if x >= right {
        return;
    }
    let mut job = LayoutJob::single_section(
        view.repo.commit(ix).subject.clone(),
        TextFormat::simple(FontId::proportional(13.5), text),
    );
    job.wrap = TextWrapping::truncate_at_width(right - x);
    let g = painter.layout_job(job);
    painter.galley(pos2(x, y - g.size().y / 2.0), g, text);
}

impl ParterreApp {
    pub(super) fn compare_request(&mut self, request: CompareRequest) {
        match request {
            CompareRequest::Mark(oid) => {
                if oid.is_some() {
                    crate::usage::action(crate::usage::Action::MarkForComparison);
                }
                self.marked = oid.map(|oid| (oid, self.commit_label(oid)));
                self.status = Some(match &self.marked {
                    Some((_, name)) => (format!("Marked {name} for comparison"), false),
                    None => ("Mark cleared".to_owned(), false),
                });
            }
            CompareRequest::Compare(first, second) => self.compare(first, second),
            CompareRequest::WorkingTree(commit) => {
                let since = self.settings.compare_window.since_ancestor;
                self.open_compare(Comparison::with_working_tree(commit, since));
            }
        }
    }

    /// Its first shown ref, or its short hash.
    pub(super) fn commit_label(&self, oid: Oid) -> String {
        let Some(repo) = &self.repo else {
            return oid.to_hex();
        };
        match repo.lookup(&oid) {
            Some(ix) => commit_name(repo, &repo.refs_by_commit(), ix, |r| {
                self.settings.graph.shows(r.kind)
            }),
            None => oid.short(repo.abbrev_len),
        }
    }

    /// In the order they were picked.
    pub(super) fn compare(&mut self, first: Oid, second: Oid) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let (Some(a), Some(b)) = (repo.lookup(&first), repo.lookup(&second)) else {
            self.status = Some(("That commit is no longer in the repository".into(), true));
            return;
        };
        let since = self.settings.compare_window.since_ancestor;
        self.open_compare(Comparison::of(&repo, a, b, since));
    }

    fn open_compare(&mut self, comparison: Comparison) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let was_open = self.compare.is_open();
        let [w, h] = self.settings.compare_window.size;
        self.compare.open(repo, comparison, vec2(w, h));
        if was_open {
            self.focus_compare = true;
        }
    }

    /// Screenshot runs embed it in the main window.
    pub(super) fn compare_window(&mut self, ctx: &egui::Context) {
        if !self.compare.is_open() {
            self.compare.title_theme = None;
            return;
        }
        let builder = egui::ViewportBuilder::default()
            .with_title(self.compare.title())
            .with_app_id(crate::settings::APP_ID)
            .with_icon(self.window_icon.clone())
            .with_inner_size(self.compare.size)
            .with_min_inner_size([480.0, 300.0]);
        let id = viewport_id();
        crate::usage::screen(ctx, id.0, crate::usage::Screen::Compare);
        if self.compare.title_theme.is_none() && !ctx.embed_viewports() {
            ctx.request_repaint();
        }
        if std::mem::take(&mut self.focus_compare) && !ctx.embed_viewports() {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        }
        ctx.show_viewport_immediate(id, builder, |ui, class| {
            super::commands::window_begin(ui);
            if class != egui::ViewportClass::EmbeddedWindow {
                let input = window_input(
                    ui,
                    &mut self.compare.title_theme,
                    self.window_theme,
                    &mut self.settings.compare_window.size,
                );
                if input.reload {
                    self.reload();
                }
                if input.fetch {
                    self.fetch(ui.ctx(), id);
                }
                // Keys go to the main window too when the window is embedded in it.
                self.compare.handle_keys(ui);
                text_size::read_input(ui, &mut self.settings.text_size, true);
                if input.close {
                    self.compare.view = None;
                }
            }
            if !self.compare.is_open() {
                return;
            }
            let palette = Palette::new(ui.visuals().dark_mode, &self.settings.branch_colors);
            let mut env = Env {
                palette,
                graph: &self.settings.graph,
                settings: &mut self.settings.compare_window,
            };
            let compare = &mut self.compare;
            egui::CentralPanel::default()
                .frame(egui::Frame::central_panel(&ui.ctx().global_style()).inner_margin(0))
                .show(ui, |ui| compare.contents(ui, &mut env));
        });
        for (repo, spec) in self.compare.diffs.take() {
            self.diffs.open(repo, spec, &self.settings.diff_window, ctx);
        }
        for (repo, spec) in std::mem::take(&mut self.compare.blame_requests) {
            self.open_blame(repo, spec, None, ctx);
        }
    }
}
