//! Scripted runs for checking the rendering without a human: take a screenshot after a few
//! frames, optionally after dragging a node or opening the context menu, then exit.

use std::path::PathBuf;

use eframe::egui::{self, Pos2, Rect, Vec2, vec2};
use parterre_core::physics::NetParams;

use crate::scene::{Scene, to_point};
use crate::view::View;

#[derive(Debug, Default)]
pub struct Automation {
    /// Save a PNG of the window here, then exit.
    pub screenshot: Option<PathBuf>,
    /// Start with the whole graph fitted instead of at HEAD.
    pub fit: bool,
    /// Drag the node nearest the centre by this much before the screenshot (in the drag mode
    /// of the settings).
    pub demo_drag: Option<Vec2>,
    /// Zoom to apply (around the canvas centre) after the initial view is set up.
    pub zoom: Option<f32>,
    /// Drag this node (a ref name or hash prefix) instead of the one nearest the centre.
    pub demo_node: Option<String>,
    /// Right-click before the screenshot, to show the context menu.
    pub demo_menu: Option<DemoMenu>,
    /// Open the ☰ menu, a toolbar popover (`filter`, `zoom`, `drag`) or the settings
    /// (`settings`, or `settings:<page>`) before the screenshot.
    pub demo_open: Option<String>,
    /// Open the log window on `<ref>` or `<ref>..<ref>` (as if those nodes were selected in
    /// that order) before the screenshot.
    pub demo_log: Option<String>,
    /// Open a diff window on `<commit>:<path>` (against the commit's first parent) before the
    /// screenshot.
    pub demo_diff: Option<String>,
    /// Open a blame window on `<commit>:<path>[:<line>]` before the screenshot.
    pub demo_blame: Option<String>,
    /// Open the compare window on `<ref>..<ref>` before the screenshot.
    pub demo_compare: Option<String>,
    /// Mark `<ref>` for comparison before the screenshot.
    pub demo_mark: Option<String>,
    /// Select the node of `<ref>` before the screenshot.
    pub demo_select: Option<String>,
    /// Something is still loading (a diff, a blame or its history): hold the screenshot.
    pub waiting: bool,
    /// Where the context menu is opened, once chosen.
    menu_at: Option<Pos2>,
    /// When (in egui's clock) the menu or popover was opened.
    opened_at: Option<f64>,
    frame: u32,
    requested: bool,
    frame_times: Vec<std::time::Instant>,
    dragging: Option<(usize, egui::Pos2)>,
}

impl Automation {
    pub fn new(
        screenshot: Option<PathBuf>,
        fit: bool,
        demo_drag: Option<Vec2>,
        zoom: Option<f32>,
    ) -> Automation {
        Automation {
            screenshot,
            fit,
            demo_drag,
            zoom,
            ..Default::default()
        }
    }
}

/// What to right-click for `demo_menu`.
#[derive(Clone, Copy, Debug)]
pub enum DemoMenu {
    /// The node of `demo_node`, or the one nearest the centre.
    Node,
    /// The empty spot farthest from any node.
    Canvas,
}

const DRAG_START: u32 = 5;
const MENU_START: u32 = 5;
const DRAG_FRAMES: u32 = 30;
const SETTLE_FRAMES: u32 = 90;

impl Automation {
    pub fn is_active(&self) -> bool {
        self.screenshot.is_some() || self.demo_drag.is_some()
    }

    /// Feeds the synthetic pointer events of `demo_menu`: move there, right-click, then hover
    /// the second item. Called before each frame, after [`Self::drive`] has counted the last.
    pub fn inject_input(&self, raw: &mut egui::RawInput) {
        let Some(at) = self.menu_at else { return };
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        match self.frame - MENU_START {
            1 => raw.events.push(egui::Event::PointerMoved(at)),
            2 => raw.events.push(button(true)),
            3 => raw.events.push(button(false)),
            6 => raw
                .events
                .push(egui::Event::PointerMoved(at + vec2(60.0, 45.0))),
            _ => {}
        }
    }

    /// Automated runs step the physics at a fixed rate so they are reproducible.
    pub fn fixed_dt(&self) -> Option<f32> {
        self.is_active().then_some(1.0 / 60.0)
    }

    /// Called after each frame. Without a scene (no repository open) only the popups can be
    /// opened before the screenshot.
    pub fn drive(
        &mut self,
        ctx: &egui::Context,
        scene: Option<&mut Scene>,
        view: &mut View,
        canvas: Rect,
        params: &NetParams,
    ) {
        if !self.is_active() {
            return;
        }
        self.frame += 1;
        self.frame_times.push(std::time::Instant::now());
        ctx.request_repaint();
        if self.frame == 2
            && let Some(z) = self.zoom
        {
            view.zoom_around(canvas, canvas.center(), z / view.zoom);
        }

        if self.frame == MENU_START {
            self.opened_at = Some(ctx.input(|i| i.time));
        }
        if self.frame == MENU_START
            && let Some(name) = &self.demo_open
            && matches!(name.as_str(), "menu" | "filter" | "zoom" | "drag")
        {
            egui::Popup::open_id(ctx, crate::app::popup_id(name));
        }
        if self.frame == MENU_START
            && let Some(scene) = scene.as_deref()
        {
            self.menu_at = match self.demo_menu {
                Some(DemoMenu::Node) => self
                    .demo_node(scene, view, canvas)
                    .map(|n| view.to_screen(canvas, scene.node_center(n))),
                Some(DemoMenu::Canvas) => Some(emptiest_spot(scene, view, canvas)),
                None => None,
            };
        }

        if let Some(delta) = self.demo_drag
            && let Some(scene) = scene
        {
            let f = self.frame;
            if f == DRAG_START {
                if let Some(n) = self.demo_node(scene, view, canvas) {
                    let carried = scene.carried_nodes(&[n], params.model);
                    scene.net.grab(n, &[n], &carried, params.model.adapts());
                    self.dragging = Some((n, scene.node_center(n)));
                }
            } else if let Some((_, start)) = self.dragging {
                if f <= DRAG_START + DRAG_FRAMES {
                    let t = (f - DRAG_START) as f32 / DRAG_FRAMES as f32;
                    scene.net.drag_to(to_point(start + delta * t));
                } else if f == DRAG_START + DRAG_FRAMES + 1 {
                    scene.net.release(params);
                }
            }
        }

        let shoot_at = if self.demo_drag.is_some() {
            DRAG_START + DRAG_FRAMES + SETTLE_FRAMES
        } else if self.demo_menu.is_some()
            || self.demo_open.is_some()
            || self.demo_log.is_some()
            || self.demo_diff.is_some()
            || self.demo_blame.is_some()
            || self.demo_compare.is_some()
        {
            MENU_START + 60
        } else {
            8
        };
        // Popups fade in over wall-clock time, which 60 frames of a small window can undercut.
        let faded_in = self.opened_at.is_none_or(|t| {
            ctx.input(|i| i.time) - t > 2.0 * f64::from(ctx.global_style().animation_time)
        });
        if self.frame >= shoot_at && faded_in && !self.waiting && !self.requested {
            self.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let gaps: Vec<f32> = self
                .frame_times
                .windows(2)
                .skip(3)
                .map(|w| (w[1] - w[0]).as_secs_f32() * 1000.0)
                .collect();
            if !gaps.is_empty() {
                let mean = gaps.iter().sum::<f32>() / gaps.len() as f32;
                let max = gaps.iter().copied().fold(0.0, f32::max);
                eprintln!(
                    "frame interval: mean {mean:.1} ms, max {max:.1} ms over {} frames",
                    gaps.len()
                );
            }
            if let Some(path) = &self.screenshot {
                let [w, h] = image.size;
                let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                match image::RgbaImage::from_raw(w as u32, h as u32, bytes)
                    .map(|img| img.save(path))
                {
                    Some(Ok(())) => eprintln!("saved screenshot to {}", path.display()),
                    Some(Err(e)) => eprintln!("could not save screenshot: {e}"),
                    None => eprintln!("screenshot had an unexpected size"),
                }
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl Automation {
    /// The node named by `demo_node` (a ref name or hash prefix), or the one nearest the
    /// centre of the canvas.
    fn demo_node(&self, scene: &Scene, view: &View, canvas: Rect) -> Option<usize> {
        let centre = view.to_world(canvas, canvas.center());
        let named = self.demo_node.as_deref().and_then(|name| {
            (0..scene.node_count()).find(|&i| {
                let node = &scene.graph.nodes[i];
                scene
                    .repo
                    .commit(node.commit)
                    .oid
                    .to_hex()
                    .starts_with(name)
                    || node.refs.iter().any(|&r| scene.repo.refs[r].name == name)
            })
        });
        named.or_else(|| {
            (0..scene.node_count()).min_by(|&a, &b| {
                scene
                    .node_center(a)
                    .distance_sq(centre)
                    .total_cmp(&scene.node_center(b).distance_sq(centre))
            })
        })
    }
}

/// The point of the canvas's upper left quarter (where an opened menu still fits) farthest
/// from any node.
fn emptiest_spot(scene: &Scene, view: &View, canvas: Rect) -> Pos2 {
    let area = Rect::from_min_size(canvas.min, canvas.size() / 2.0).shrink(20.0);
    let nearest = |p: Pos2| {
        (0..scene.node_count())
            .map(|n| view.to_screen(canvas, scene.node_center(n)).distance_sq(p))
            .fold(f32::INFINITY, f32::min)
    };
    (0..=16)
        .flat_map(|i| (0..=16).map(move |j| (i as f32 / 16.0, j as f32 / 16.0)))
        .map(|(u, v)| area.lerp_inside(vec2(u, v)))
        .max_by(|&a, &b| nearest(a).total_cmp(&nearest(b)))
        .unwrap_or(area.center())
}
