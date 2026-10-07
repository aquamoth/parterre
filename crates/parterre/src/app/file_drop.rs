//! Opening a repository by dropping its folder on the window (#335), from Finder, Explorer or
//! a file manager: any folder inside the repository, or a file (its folder), as *Open folder…*
//! takes. While something is dragged over the window, the window says what a drop opens. Of
//! several, the first. A modal dialog clears both (`dialogs::ModalLock`): no drop then.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, FontId, Stroke, StrokeKind};

use super::ParterreApp;
use crate::widgets;

impl ParterreApp {
    /// Opens what was dropped on the window this frame, or shows what a drop would open.
    pub(super) fn file_drop(&mut self, ctx: &egui::Context) {
        let (hovered, dropped) = ctx.input(|i| {
            (
                i.raw.hovered_files.first().map(|f| f.path.clone()),
                i.raw.dropped_files.first().map(|f| f.path().to_owned()),
            )
        });
        if let Some(path) = dropped {
            self.open_folder(&dropped_folder(&path));
        } else if let Some(path) = hovered {
            // Some platforms don't say what is dragged until it is dropped.
            let name = path.map(|p| super::name_and_place(&dropped_folder(&p)).0);
            paint_drop_target(ctx, name.as_deref());
        }
    }
}

/// The folder a dropped path opens: a folder itself, a file the folder it is in.
fn dropped_folder(path: &Path) -> PathBuf {
    if path.is_file()
        && let Some(parent) = path.parent()
    {
        return parent.to_owned();
    }
    path.to_owned()
}

/// Over the whole window: an outline in the accent colour and what a drop opens.
fn paint_drop_target(ctx: &egui::Context, name: Option<&str>) {
    let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("file-drop"));
    let painter = ctx.layer_painter(layer);
    let screen = ctx.content_rect();
    let dark = ctx.global_style().visuals.dark_mode;
    let accent = widgets::tones_of(dark).accent;
    let veil = if dark {
        Color32::from_black_alpha(190)
    } else {
        Color32::from_white_alpha(225)
    };
    painter.rect_filled(screen, 0.0, veil);
    painter.rect_stroke(
        screen.shrink(10.0),
        10.0,
        Stroke::new(2.0, accent),
        StrokeKind::Inside,
    );
    let text = match name {
        Some(name) => format!("Open {name}"),
        None => "Open folder".to_owned(),
    };
    let galley = painter.layout_no_wrap(text, FontId::proportional(20.0), accent);
    let label =
        egui::Rect::from_center_size(screen.center(), galley.size() + egui::vec2(36.0, 20.0));
    let fill = ctx.global_style().visuals.window_fill;
    painter.rect(
        label,
        10.0,
        fill,
        Stroke::new(1.0, accent),
        StrokeKind::Inside,
    );
    painter.galley(label.center() - galley.size() / 2.0, galley, accent);
}

#[cfg(test)]
mod tests {
    use super::dropped_folder;

    #[test]
    fn a_file_opens_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("README.md");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(dropped_folder(&file), dir.path());
        assert_eq!(dropped_folder(dir.path()), dir.path());
        // Gone by now, or never there: as dropped, for opening to say why it can't.
        let gone = dir.path().join("gone");
        assert_eq!(dropped_folder(&gone), gone);
    }
}
