use std::sync::Arc;

use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{
    self, Color32, CornerRadius, FontId, Galley, Painter, Pos2, Rect, Sense, Stroke, Ui, Vec2,
    pos2, vec2,
};
use parterre_core::glyphs;
use parterre_core::merged::MergedBranches;
use parterre_core::revgraph::GraphOptions;
use parterre_core::{CommitIx, GitRef, Label, RefKind, Repo, Worktree};

use crate::theme::{Palette, text_on};
use crate::widgets;

pub(super) const HEADING_HEIGHT: f32 = 26.0;
pub(super) const CELL_PAD: f32 = 8.0;
pub(super) const BADGE_HEIGHT: f32 = 17.0;
pub(super) const BADGE_PAD: f32 = 5.0;
pub(super) const BADGE_FONT_SIZE: f32 = 11.5;
const BADGE_FOLDER_SIZE: f32 = 11.0;
const BADGE_FOLDER_ROOM: f32 = BADGE_FOLDER_SIZE + 3.0;

pub(super) struct Colors {
    pub(super) pane: Color32,
    pub(super) stripe: Color32,
    pub(super) hover: Color32,
    pub(super) line: Color32,
    pub(super) selected_bg: Color32,
    pub(super) selected_fg: Color32,
    pub(super) link: Color32,
    pub(super) added: Color32,
    pub(super) removed: Color32,
    pub(super) renamed: Color32,
    pub(super) modified: Color32,
    pub(super) found: Color32,
    pub(super) lanes: [Color32; 8],
}

pub(super) fn colors(ui: &Ui) -> Colors {
    let t = widgets::tones(ui);
    if ui.visuals().dark_mode {
        Colors {
            pane: Color32::from_gray(22),
            stripe: Color32::from_white_alpha(5),
            hover: Color32::from_white_alpha(13),
            line: Color32::from_white_alpha(23),
            selected_bg: t.on_bg,
            selected_fg: Color32::from_rgb(0xcf, 0xe5, 0xff),
            link: t.on_fg,
            added: Color32::from_rgb(0x7b, 0xd8, 0x8f),
            removed: Color32::from_rgb(0xff, 0x8a, 0x80),
            renamed: Color32::from_rgb(0xd1, 0xa5, 0xff),
            modified: Color32::from_rgb(0xe0, 0xa8, 0x40),
            found: Color32::from_rgba_unmultiplied(0xd0, 0x9a, 0x1c, 80),
            lanes: [
                Color32::from_rgb(0xef, 0x53, 0x50),
                Color32::from_rgb(0x42, 0xa5, 0xf5),
                Color32::from_rgb(0x66, 0xbb, 0x6a),
                Color32::from_rgb(0xff, 0xa7, 0x26),
                Color32::from_rgb(0xab, 0x47, 0xbc),
                Color32::from_rgb(0x26, 0xc6, 0xda),
                Color32::from_rgb(0xec, 0x40, 0x7a),
                Color32::from_rgb(0xa1, 0x88, 0x7f),
            ],
        }
    } else {
        Colors {
            pane: Color32::WHITE,
            stripe: Color32::from_black_alpha(5),
            hover: Color32::from_black_alpha(11),
            line: Color32::from_black_alpha(26),
            selected_bg: t.on_bg,
            selected_fg: Color32::from_rgb(0x0b, 0x3d, 0x7a),
            link: t.on_fg,
            added: Color32::from_rgb(0x2e, 0x7d, 0x32),
            removed: Color32::from_rgb(0xc6, 0x28, 0x28),
            renamed: Color32::from_rgb(0x6a, 0x1b, 0x9a),
            modified: Color32::from_rgb(0xa8, 0x6a, 0x00),
            found: Color32::from_rgba_unmultiplied(0xff, 0xcc, 0x33, 130),
            lanes: [
                Color32::from_rgb(0xd3, 0x2f, 0x2f),
                Color32::from_rgb(0x19, 0x76, 0xd2),
                Color32::from_rgb(0x38, 0x8e, 0x3c),
                Color32::from_rgb(0xf5, 0x7c, 0x00),
                Color32::from_rgb(0x7b, 0x1f, 0xa2),
                Color32::from_rgb(0x00, 0x83, 0x8f),
                Color32::from_rgb(0xc2, 0x18, 0x5b),
                Color32::from_rgb(0x5d, 0x40, 0x37),
            ],
        }
    }
}

/// One line, cut with an ellipsis at `width`.
pub(super) fn cell(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping::truncate_at_width(width.max(1.0));
    ui.painter().layout_job(job)
}

/// What decides how a commit's labels are drawn: which refs the graph shows, and which
/// branches were squash- or rebase-merged.
#[derive(Clone, Copy, Debug)]
pub(super) struct Labels<'a> {
    pub graph: &'a GraphOptions,
    pub merged: &'a MergedBranches,
}

/// A label before a commit's subject, as the graph draws them.
#[derive(Clone, Copy, Debug)]
pub(super) enum Badge<'a> {
    Ref {
        git_ref: &'a GitRef,
        worktree: Option<&'a Worktree>,
        ahead_behind: Option<(usize, usize)>,
        /// Squash- or rebase-merged: striped, as in the graph.
        merged: bool,
    },
    DetachedWorktree(&'a Worktree),
}

impl<'a> Badge<'a> {
    fn text(&self) -> String {
        match *self {
            Badge::Ref {
                git_ref,
                worktree: Some(w),
                ..
            } if git_ref.kind == RefKind::DetachedHead => w.name(),
            Badge::Ref { git_ref, .. } => git_ref.name.clone(),
            Badge::DetachedWorktree(w) => w.name(),
        }
    }

    fn is_detached_worktree(&self) -> bool {
        match *self {
            Badge::Ref {
                git_ref, worktree, ..
            } => git_ref.kind == RefKind::DetachedHead && worktree.is_some(),
            Badge::DetachedWorktree(_) => true,
        }
    }

    fn worktree(&self) -> Option<&'a Worktree> {
        match *self {
            Badge::Ref { worktree, .. } => worktree,
            Badge::DetachedWorktree(w) => Some(w),
        }
    }

    fn is_merged(&self) -> bool {
        matches!(*self, Badge::Ref { merged: true, .. })
    }

    fn ahead_behind(&self) -> Option<(usize, usize)> {
        match *self {
            Badge::Ref { ahead_behind, .. } => ahead_behind,
            Badge::DetachedWorktree(_) => None,
        }
    }

    fn fill(&self, palette: &Palette) -> Color32 {
        let worktree_fill = |w: &Worktree| {
            if w.missing {
                palette.missing_worktree
            } else {
                palette.worktree
            }
        };
        match *self {
            Badge::Ref {
                git_ref,
                worktree: Some(w),
                ..
            } if !git_ref.is_head => worktree_fill(w),
            Badge::Ref { git_ref: r, .. } => palette.ref_fill(r.kind, r.is_head, &r.name),
            Badge::DetachedWorktree(w) => worktree_fill(w),
        }
    }

    fn chrome(&self, painter: &Painter) -> Chrome {
        let folder = if self.worktree().is_some() {
            BADGE_FOLDER_ROOM
        } else {
            0.0
        };
        let counts = self
            .ahead_behind()
            .map(|c| (c, crate::upstreams::counts_width(painter, c)));
        Chrome {
            width: 2.0 * BADGE_PAD + folder + counts.map_or(0.0, |(_, width)| width),
            counts,
        }
    }
}

/// What a badge shows besides its name: the padding, a worktree's folder and the ahead and
/// behind counts, with the counts' width.
struct Chrome {
    width: f32,
    counts: Option<((usize, usize), f32)>,
}

/// `refs` index [`Repo::refs`] in display order; `commit` is `None` for one outside the
/// snapshot.
pub(super) fn badges<'a>(
    repo: &'a Repo,
    refs: &[usize],
    commit: Option<CommitIx>,
    graph: &GraphOptions,
    merged: &MergedBranches,
) -> Vec<Badge<'a>> {
    let worktrees_shown = graph.show_worktrees;
    let refs: Vec<usize> = refs
        .iter()
        .copied()
        .filter(|&i| {
            let r = &repo.refs[i];
            graph.shows(r.kind)
                || (worktrees_shown && repo.worktrees_on(&r.full_name).next().is_some())
        })
        .collect();
    let detached: Vec<usize> = match commit {
        Some(c) if worktrees_shown => repo.detached_worktrees_at(c).collect(),
        _ => Vec::new(),
    };
    repo.labels(&refs, &detached, worktrees_shown)
        .into_iter()
        .map(|label| match label {
            Label::Ref { index, worktree } => Badge::Ref {
                git_ref: &repo.refs[index],
                worktree: worktree.map(|k| &repo.worktrees[k]),
                ahead_behind: repo
                    .upstream_of(index)
                    .filter(|_| graph.show_upstreams)
                    .map(|u| (u.ahead(), u.behind()))
                    .filter(|&(ahead, behind)| ahead + behind > 0),
                merged: merged.of(repo, index).is_some(),
            },
            Label::Worktree(k) => Badge::DetachedWorktree(&repo.worktrees[k]),
        })
        .collect()
}

/// At most `max_width` wide. Returns its width.
pub(super) fn badge(
    ui: &Ui,
    badge: &Badge,
    palette: &Palette,
    left_center: Pos2,
    max_width: f32,
) -> f32 {
    let chrome = badge.chrome(ui.painter());
    paint_badge(ui, badge, &chrome, palette, left_center, max_width)
}

fn paint_badge(
    ui: &Ui,
    badge: &Badge,
    chrome: &Chrome,
    palette: &Palette,
    left_center: Pos2,
    max_width: f32,
) -> f32 {
    let fill = badge.fill(palette);
    let color = text_on(fill);
    let painter = ui.painter();
    let mut job = LayoutJob::single_section(
        badge.text(),
        TextFormat {
            font_id: FontId::proportional(BADGE_FONT_SIZE),
            color,
            italics: badge.is_detached_worktree(),
            ..TextFormat::default()
        },
    );
    job.wrap = TextWrapping::truncate_at_width((max_width - chrome.width).max(1.0));
    let g = painter.layout_job(job);
    let size = vec2(g.size().x + chrome.width, BADGE_HEIGHT);
    let rect = Rect::from_min_size(pos2(left_center.x, left_center.y - size.y / 2.0), size);
    painter.rect(
        rect,
        CornerRadius::same(3),
        fill,
        Stroke::new(1.0, Color32::from_black_alpha(64)),
        egui::StrokeKind::Inside,
    );
    if badge.is_merged() {
        let period = crate::merged_branches::period_for(BADGE_HEIGHT);
        let inside = rect.shrink(1.0);
        crate::merged_branches::paint_stripes(painter, inside, CornerRadius::same(2), period, fill);
    }
    let mut text_left = rect.left() + BADGE_PAD;
    if let Some(w) = badge.worktree() {
        let icon = Rect::from_center_size(
            pos2(text_left + BADGE_FOLDER_SIZE / 2.0, rect.center().y),
            Vec2::splat(BADGE_FOLDER_SIZE),
        );
        let folder = if w.missing {
            glyphs::FOLDER_GONE
        } else {
            glyphs::FOLDER
        };
        widgets::paint_glyph(painter, icon, folder, color);
        text_left += BADGE_FOLDER_ROOM;
    }
    painter.galley(
        pos2(text_left, rect.center().y - g.size().y / 2.0),
        g,
        color,
    );
    if let Some((counts, width)) = chrome.counts {
        let at = pos2(rect.right() - BADGE_PAD - width, rect.center().y);
        crate::upstreams::paint_counts(painter, at, counts, color);
    }
    size.x
}

pub(super) fn badge_widget(ui: &mut Ui, b: &Badge, palette: &Palette) {
    let chrome = b.chrome(ui.painter());
    let name = ui.painter().layout_no_wrap(
        b.text(),
        FontId::proportional(BADGE_FONT_SIZE),
        Color32::PLACEHOLDER,
    );
    let width = name.size().x + 1.0 + chrome.width;
    let (rect, response) = ui.allocate_exact_size(vec2(width, BADGE_HEIGHT), Sense::hover());
    paint_badge(ui, b, &chrome, palette, rect.left_center(), rect.width());
    if let Some(w) = b.worktree() {
        response.on_hover_text(format!("Worktree {}", w.path.display()));
    }
}

pub(super) fn heading_background(ui: &Ui, rect: Rect, c: &Colors) {
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, ui.visuals().panel_fill);
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, c.line),
    );
}
