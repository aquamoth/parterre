use std::sync::Arc;

use eframe::egui::{self, Ui, Vec2, ViewportBuilder, ViewportClass, ViewportId};

use crate::usage;

/// A window in a viewport of its own, which screenshot runs embed in the main window.
#[derive(Debug, Default)]
pub(super) struct Viewport {
    /// The theme its title bar was last given; `None` until its first frame.
    title_theme: Option<egui::SystemTheme>,
    pub(super) bring_to_front: bool,
}

/// What a window in a viewport of its own reads of it, each frame.
pub(super) struct ViewportInput {
    pub(super) close_requested: bool,
    /// The inner size, once the window has one.
    pub(super) size: Option<Vec2>,
}

impl ViewportInput {
    pub(super) fn keep_size(&self, size: &mut [f32; 2]) {
        if let Some(inner) = self.size {
            *size = [inner.x, inner.y];
        }
    }
}

pub(super) fn builder(
    title: impl Into<String>,
    size: Vec2,
    min_size: impl Into<Vec2>,
    icon: &Arc<egui::IconData>,
) -> ViewportBuilder {
    ViewportBuilder::default()
        .with_title(title)
        .with_app_id(crate::settings::APP_ID)
        .with_icon(icon.clone())
        .with_inner_size(size)
        .with_min_inner_size(min_size)
}

/// Edge to edge: the window draws its own toolbar and panes.
pub(super) fn central_panel(ui: &Ui) -> egui::CentralPanel {
    egui::CentralPanel::default()
        .frame(egui::Frame::central_panel(&ui.ctx().global_style()).inner_margin(0))
}

impl Viewport {
    /// Every frame the window is open, before its viewport `id` is shown.
    pub(super) fn before_show(
        &mut self,
        ctx: &egui::Context,
        id: ViewportId,
        screen: usage::Screen,
    ) {
        usage::screen(ctx, id.0, screen);
        // A new window is created after this frame: paint it without waiting for input.
        if self.title_theme.is_none() && !ctx.embed_viewports() {
            ctx.request_repaint();
        }
        if std::mem::take(&mut self.bring_to_front) && !ctx.embed_viewports() {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        }
    }

    /// First in the window's pass. `None` when embedded: its keys are the main window's then.
    pub(super) fn begin(
        &mut self,
        ui: &Ui,
        class: ViewportClass,
        theme: Option<egui::SystemTheme>,
    ) -> Option<ViewportInput> {
        super::commands::window_begin(ui);
        if class == ViewportClass::EmbeddedWindow {
            return None;
        }
        if self.title_theme != theme {
            self.title_theme = theme;
            if let Some(theme) = theme {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::SetTheme(theme));
            }
        }
        let (close_requested, size) = ui.input(|i| {
            let viewport = i.viewport();
            (
                viewport.close_requested(),
                viewport.inner_rect.map(|r| r.size()),
            )
        });
        Some(ViewportInput {
            close_requested,
            size: size.filter(|s| s.x > 0.0 && s.y > 0.0),
        })
    }

    /// The window's next viewport is a new one, whose title bar has no theme yet.
    pub(super) fn closed(&mut self) {
        self.title_theme = None;
    }
}
