use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2, Rect, Vec2, vec2};

use crate::record::{self, Recorder};
use crate::scene::Scene;
use crate::script::{self, Crop, Step, Target};
use crate::view::View;

static FAILED: AtomicBool = AtomicBool::new(false);

pub fn fail() {
    FAILED.store(true, Ordering::Relaxed);
}

pub fn failed() -> bool {
    FAILED.load(Ordering::Relaxed)
}

const SCRIPT_FPS: u32 = 60;
const SETTLE_FRAMES: u32 = 8;
/// `--zoom` applies once the initial view is set.
const START_ZOOM_FRAME: u32 = 2;
const WARM_UP_FRAME_GAPS: usize = 3;
const TARGET_TIMEOUT: Duration = Duration::from_secs(15);
const PAUSE_FRAMES: usize = 12;
/// A moment on the end, for a recording to show it.
const LINGER_AT_END_FRAMES: usize = 15;
const MAX_TEXTS_LISTED: usize = 40;
const CROP_SHADOW_MARGIN: f32 = 16.0;

#[derive(Debug, Default)]
pub struct Automation {
    pub start_fitted: bool,
    pub start_zoom: Option<f32>,
    pub canned_pull_requests: Option<String>,
    pub canned_github_repositories: Option<String>,
    pub canned_newer_release: Option<parterre_telemetry::Update>,
    pub waiting: bool,
    pub script: Option<Runner>,
    pub record: Option<Recorder>,
    pub pending_open: Option<String>,
    pub texts: Texts,
    next_frame_is_screenshot: bool,
    recording_since: Option<Instant>,
    frame: u32,
    frame_times: Vec<Instant>,
}

#[derive(Debug)]
struct DroppedPath(PathBuf);

impl egui::DroppedFile for DroppedPath {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

#[derive(Debug)]
enum Shot {
    ScriptStep { path: PathBuf, crop: Option<Rect> },
    RecordingFrame { frame_time: u64 },
}

impl Automation {
    pub fn is_scripted(&self) -> bool {
        self.script.is_some()
    }

    /// Keeps time by frames, so a recording plays at the speed a person would see however
    /// slowly its frames were captured.
    pub fn inject_input(&mut self, raw: &mut egui::RawInput) {
        let Some(runner) = &mut self.script else {
            return;
        };
        raw.time = Some(f64::from(self.frame) / f64::from(SCRIPT_FPS));
        raw.predicted_dt = 1.0 / SCRIPT_FPS as f32;
        raw.events
            .extend(runner.coming_frames.pop_front().unwrap_or_default());
        if let Some(path) = &runner.dragged_file {
            raw.hovered_files = vec![egui::HoveredFile {
                path: Some(path.clone()),
                ..Default::default()
            }];
        }
        if let Some(path) = runner.dropped_file.take() {
            raw.dropped_files = vec![Arc::new(DroppedPath(path))];
        }
    }

    /// Scripted runs step the physics at a fixed rate, to be reproducible.
    pub fn fixed_dt(&self) -> Option<f32> {
        self.is_scripted().then_some(1.0 / SCRIPT_FPS as f32)
    }

    pub fn fail_step(&mut self, ctx: &egui::Context, message: &str) {
        let line = self.script.as_mut().map_or(0, |runner| {
            runner.closing = true;
            runner.last_line_taken()
        });
        stop_with_failure(ctx, &format!("line {line}: {message}"));
    }

    pub fn drive(
        &mut self,
        ctx: &egui::Context,
        scene: Option<&Scene>,
        view: &mut View,
        canvas: Rect,
    ) {
        self.take_shots(ctx);
        if !self.is_scripted() {
            self.record_frame(ctx, false);
            return;
        }
        self.frame += 1;
        self.frame_times.push(Instant::now());
        ctx.request_repaint();
        if self.frame == START_ZOOM_FRAME
            && let Some(zoom) = self.start_zoom
        {
            view.zoom_around(canvas, canvas.center(), zoom / view.zoom);
        }
        self.run_script(ctx, scene, view, canvas);
    }

    fn take_shots(&mut self, ctx: &egui::Context) {
        let shots: Vec<_> = ctx.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| match e {
                    Event::Screenshot {
                        image, user_data, ..
                    } => Some((image.clone(), user_data.data.clone())),
                    _ => None,
                })
                .collect()
        });
        for (image, data) in shots {
            match data.as_deref().and_then(|d| d.downcast_ref::<Shot>()) {
                Some(Shot::RecordingFrame { frame_time }) => {
                    if let Some(record) = &mut self.record {
                        record.push(&image, *frame_time);
                    }
                }
                Some(Shot::ScriptStep { path, crop }) => {
                    match crop {
                        Some(rect) => {
                            let cropped = image.region(rect, Some(ctx.pixels_per_point()));
                            save_screenshot(&cropped, path);
                        }
                        None => save_screenshot(&image, path),
                    }
                    if let Some(runner) = &mut self.script {
                        runner.pending_screenshots -= 1;
                    }
                }
                None => {}
            }
        }
    }

    fn report_frame_times(&self) {
        let gaps: Vec<f32> = self
            .frame_times
            .windows(2)
            .skip(WARM_UP_FRAME_GAPS)
            .map(|w| (w[1] - w[0]).as_secs_f32() * 1000.0)
            .collect();
        if gaps.is_empty() {
            return;
        }
        let mean = gaps.iter().sum::<f32>() / gaps.len() as f32;
        let max = gaps.iter().copied().fold(0.0, f32::max);
        eprintln!(
            "frame interval: mean {mean:.1} ms, max {max:.1} ms over {} frames",
            gaps.len()
        );
    }

    fn run_script(
        &mut self,
        ctx: &egui::Context,
        scene: Option<&Scene>,
        view: &View,
        canvas: Rect,
    ) {
        let Some(runner) = &mut self.script else {
            return;
        };
        if self.frame < SETTLE_FRAMES || self.waiting || self.pending_open.is_some() {
            self.record_frame(ctx, false);
            return;
        }
        let action = {
            let texts = self.texts.lock().unwrap_or_else(PoisonError::into_inner);
            let find = |target: &Target| -> Option<Pos2> {
                match target {
                    Target::Text(text) => find_text(&texts, text),
                    Target::Node(name) => {
                        let scene = scene?;
                        let node = named_node(scene, name)?;
                        let at = view.to_screen(canvas, scene.node_center(node));
                        canvas.contains(at).then_some(at)
                    }
                    Target::Canvas => scene.map(|scene| emptiest_spot(scene, view, canvas)),
                    Target::Toolbar(name) => ctx
                        .read_response(crate::app::toolbar_button_id(name))
                        .map(|r| r.rect.center()),
                    Target::Point(p) => Some(p.to_pos2()),
                }
            };
            runner.advance(find, &texts)
        };
        let mut screenshot_requested = false;
        match action {
            Action::Busy => {}
            Action::Open(what) => self.pending_open = Some(what),
            Action::Screenshot(path, crop) => {
                let rect = crop_rect(ctx, crop);
                if crop != Crop::Full && rect.is_none() {
                    eprintln!("{}: no {crop:?} on screen to crop to", path.display());
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
                    Shot::ScriptStep { path, crop: rect },
                )));
                screenshot_requested = true;
            }
            Action::Done => {
                self.report_frame_times();
                if let Some(scene) = scene {
                    report_tidiness(scene);
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Action::Failed(message) => stop_with_failure(ctx, &message),
        }
        self.record_frame(ctx, screenshot_requested);
    }

    /// A screenshot shows the frame after the one asking for it. That frame of a script's
    /// screenshot goes without the pointer and unrecorded: the one before shows a moment longer.
    fn record_frame(&mut self, ctx: &egui::Context, screenshot_requested: bool) {
        let Some(record) = &mut self.record else {
            return;
        };
        if std::mem::take(&mut self.next_frame_is_screenshot) {
            return;
        }
        record::paint_pointer(ctx);
        if screenshot_requested {
            self.next_frame_is_screenshot = true;
            return;
        }
        let frame_time = if self.script.is_some() {
            u64::from(self.frame) * u64::from(record::FPS) / u64::from(SCRIPT_FPS)
        } else {
            let since = *self.recording_since.get_or_insert_with(Instant::now);
            ctx.request_repaint_after(Duration::from_secs(1) / record::FPS);
            (since.elapsed().as_secs_f64() * f64::from(record::FPS)) as u64
        };
        if record
            .last_requested_frame_time
            .is_none_or(|last| frame_time > last)
        {
            record.last_requested_frame_time = Some(frame_time);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
                Shot::RecordingFrame { frame_time },
            )));
        }
    }
}

fn stop_with_failure(ctx: &egui::Context, message: &str) {
    eprintln!("parterre: script {message}");
    fail();
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
}

fn save_screenshot(image: &egui::ColorImage, path: &Path) {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(dir);
    }
    match record::to_rgba(image).map(|rgba| rgba.save(path)) {
        Some(Ok(())) => eprintln!("saved screenshot to {}", path.display()),
        Some(Err(e)) => {
            eprintln!("could not save screenshot: {e}");
            fail();
        }
        None => eprintln!("screenshot had an unexpected size"),
    }
}

fn crop_rect(ctx: &egui::Context, crop: Crop) -> Option<Rect> {
    let rect = ctx.memory(|m| {
        let areas = m.areas();
        let visible = m.layer_ids().filter(|l| areas.is_visible(l));
        match crop {
            Crop::Full => None,
            Crop::Window => visible
                .filter(|l| l.order == egui::Order::Middle && areas.parent_layer(*l).is_none())
                .last()
                .and_then(|l| m.area_rect(l.id)),
            Crop::Popup => visible
                .filter(|l| l.order == egui::Order::Foreground)
                .filter_map(|l| m.area_rect(l.id))
                // Not empty areas, such as that of notifications when there are none.
                .filter(|r| r.width() > 1.0 && r.height() > 1.0)
                .reduce(Rect::union),
        }
    })?;
    Some(
        rect.expand(CROP_SHADOW_MARGIN)
            .intersect(ctx.content_rect()),
    )
}

/// `texts` are in paint order: the last match is the topmost.
fn find_text(texts: &[(String, Rect)], text: &str) -> Option<Pos2> {
    texts
        .iter()
        .rev()
        .find(|(t, _)| t == text)
        .or_else(|| texts.iter().rev().find(|(t, _)| t.contains(text)))
        .map(|(_, r)| r.center())
}

pub type Texts = Arc<Mutex<Vec<(String, Rect)>>>;

pub struct TextCollector(pub Texts);

impl egui::Plugin for TextCollector {
    fn debug_name(&self) -> &'static str {
        "TextCollector"
    }

    fn output_hook(&mut self, _ctx: &egui::Context, output: &mut egui::FullOutput) {
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_texts(&clipped.shape, &mut texts);
        }
        if let Ok(mut shared) = self.0.lock() {
            *shared = texts;
        }
    }
}

fn collect_texts(shape: &egui::Shape, texts: &mut Vec<(String, Rect)>) {
    match shape {
        egui::Shape::Text(t) => {
            let rect = t
                .galley
                .rows
                .iter()
                .filter(|r| !r.glyphs.is_empty())
                .map(|r| r.rect_without_leading_space())
                .reduce(Rect::union)
                .unwrap_or(t.galley.rect);
            texts.push((t.galley.text().to_owned(), rect.translate(t.pos.to_vec2())));
        }
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_texts(s, texts)),
        _ => {}
    }
}

#[derive(Debug, PartialEq)]
enum Action {
    Busy,
    Open(String),
    Screenshot(PathBuf, Crop),
    Done,
    Failed(String),
}

#[derive(Debug, Default)]
pub struct Runner {
    lines: Vec<script::Line>,
    next_line: usize,
    coming_frames: VecDeque<Vec<Event>>,
    pointer: Option<Pos2>,
    idle_frames: usize,
    waiting_since: Option<Instant>,
    pending_screenshots: usize,
    lingering_at_end: bool,
    closing: bool,
    dragged_file: Option<PathBuf>,
    dropped_file: Option<PathBuf>,
}

impl Runner {
    pub fn new(lines: Vec<script::Line>) -> Runner {
        Runner {
            lines,
            ..Runner::default()
        }
    }

    fn last_line_taken(&self) -> usize {
        self.next_line
            .checked_sub(1)
            .and_then(|i| self.lines.get(i))
            .map_or(0, |l| l.number)
    }

    pub fn push_screenshot(&mut self, path: PathBuf) {
        let number = self.lines.last().map_or(0, |l| l.number) + 1;
        self.lines.push(script::Line {
            number,
            step: Step::Screenshot(path, Crop::Full),
        });
    }

    fn advance(
        &mut self,
        find: impl Fn(&Target) -> Option<Pos2>,
        texts: &[(String, Rect)],
    ) -> Action {
        if !self.coming_frames.is_empty() || self.pending_screenshots > 0 || self.closing {
            return Action::Busy;
        }
        if self.idle_frames > 0 {
            self.idle_frames -= 1;
            return Action::Busy;
        }
        let Some(line) = self.lines.get(self.next_line).cloned() else {
            if !self.lingering_at_end {
                self.lingering_at_end = true;
                self.idle_frames = LINGER_AT_END_FRAMES;
                return Action::Busy;
            }
            self.closing = true;
            return Action::Done;
        };
        let at = match &line.step {
            Step::WaitFor(target)
            | Step::Click { target, .. }
            | Step::Hover(target)
            | Step::Drag { target, .. } => {
                let Some(at) = find(target) else {
                    return self.target_missing(line.number, target, texts);
                };
                at
            }
            _ => Pos2::ZERO,
        };
        self.waiting_since = None;
        self.next_line += 1;
        match line.step {
            Step::Wait { seconds } => {
                self.idle_frames = (seconds * SCRIPT_FPS as f32).round().max(0.0) as usize;
            }
            Step::WaitFor(_) => {}
            Step::Click {
                button,
                count,
                modifiers,
                ..
            } => self.click(at, button, count, modifiers),
            Step::Hover(_) => {
                self.glide(at, false);
                self.pause(20);
            }
            Step::Drag { by, .. } => self.drag(at, by),
            Step::Scroll(delta) => {
                self.coming_frames.push_back(vec![Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta,
                    phase: egui::TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                }]);
                self.pause(PAUSE_FRAMES);
            }
            Step::Key(modifiers, key) => {
                for pressed in [true, false] {
                    self.coming_frames.push_back(vec![Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers,
                    }]);
                }
                self.pause(PAUSE_FRAMES);
            }
            Step::Type(text) => {
                // A few characters a second, as typed.
                for c in text.chars() {
                    self.coming_frames
                        .push_back(vec![Event::Text(c.to_string())]);
                    self.pause(3);
                }
                self.pause(8);
            }
            Step::Open(what) => {
                self.idle_frames = PAUSE_FRAMES;
                return Action::Open(what);
            }
            Step::DragFile(path) => {
                self.dragged_file = Some(path);
                self.pause(20);
            }
            Step::DropFile => {
                self.dropped_file = self.dragged_file.take();
                self.pause(20);
            }
            Step::Screenshot(path, crop) => {
                self.pending_screenshots += 1;
                return Action::Screenshot(path, crop);
            }
        }
        Action::Busy
    }

    fn click(&mut self, at: Pos2, button: PointerButton, count: u32, modifiers: Modifiers) {
        self.glide(at, false);
        // A moment over it, to show its hover.
        self.pause(PAUSE_FRAMES);
        if !modifiers.is_none() {
            self.coming_frames
                .push_back(vec![Event::ModifiersChanged(modifiers)]);
        }
        for _ in 0..count {
            for pressed in [true, false] {
                self.coming_frames.push_back(vec![Event::PointerButton {
                    pos: at,
                    button,
                    pressed,
                    modifiers,
                }]);
            }
        }
        if !modifiers.is_none() {
            self.coming_frames
                .push_back(vec![Event::ModifiersChanged(Modifiers::NONE)]);
        }
        self.pause(PAUSE_FRAMES);
    }

    fn drag(&mut self, from: Pos2, by: Vec2) {
        let to = from + by;
        let primary = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.glide(from, false);
        self.pause(6);
        self.coming_frames.push_back(vec![primary(from, true)]);
        self.pause(4);
        self.glide(to, true);
        self.pause(4);
        self.coming_frames.push_back(vec![primary(to, false)]);
        self.pause(PAUSE_FRAMES);
    }

    fn pause(&mut self, frames: usize) {
        self.coming_frames
            .extend(std::iter::repeat_n(Vec::new(), frames));
    }

    fn target_missing(
        &mut self,
        line_number: usize,
        target: &Target,
        texts: &[(String, Rect)],
    ) -> Action {
        let since = *self.waiting_since.get_or_insert_with(Instant::now);
        if since.elapsed() < TARGET_TIMEOUT {
            return Action::Busy;
        }
        self.closing = true;
        let mut shown: Vec<&str> = Vec::new();
        for (text, _) in texts.iter().rev() {
            if shown.len() == MAX_TEXTS_LISTED {
                break;
            }
            if !shown.contains(&text.as_str()) {
                shown.push(text);
            }
        }
        Action::Failed(format!(
            "line {line_number}: no {target:?} on screen; the topmost texts: {shown:?}"
        ))
    }

    fn glide(&mut self, to: Pos2, button_held: bool) {
        let from = self.pointer.unwrap_or(to);
        let min_frames = if button_held { 20 } else { 8 };
        let frames = ((from.distance(to) / 25.0) as usize).clamp(min_frames, 40);
        for i in 1..=frames {
            let t = i as f32 / frames as f32;
            let eased = t * t * (3.0 - 2.0 * t);
            self.coming_frames
                .push_back(vec![Event::PointerMoved(from.lerp(to, eased))]);
        }
        self.pointer = Some(to);
    }
}

fn named_node(scene: &Scene, name: &str) -> Option<usize> {
    scene.graph.nodes.iter().position(|node| {
        scene
            .repo
            .commit(node.commit)
            .oid
            .to_hex()
            .starts_with(name)
            || node.refs.iter().any(|&r| scene.repo.refs[r].name == name)
    })
}

/// In the canvas's upper left quarter, where an opened menu still fits.
fn emptiest_spot(scene: &Scene, view: &View, canvas: Rect) -> Pos2 {
    const GRID_STEPS: u8 = 16;
    let area = Rect::from_min_size(canvas.min, canvas.size() / 2.0).shrink(20.0);
    let clearance = |p: Pos2| {
        (0..scene.node_count())
            .map(|n| view.to_screen(canvas, scene.node_center(n)).distance_sq(p))
            .fold(f32::INFINITY, f32::min)
    };
    let fraction = |step: u8| f32::from(step) / f32::from(GRID_STEPS);
    (0..=GRID_STEPS)
        .flat_map(|i| (0..=GRID_STEPS).map(move |j| vec2(fraction(i), fraction(j))))
        .map(|uv| {
            let spot = area.lerp_inside(uv);
            (spot, clearance(spot))
        })
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .map_or(area.center(), |(spot, _)| spot)
}

fn report_tidiness(scene: &Scene) {
    let t = scene.net.tidiness();
    eprintln!(
        "graph: {} overlapping boxes, {} edges through boxes, {} doubling back, {} detours",
        t.overlaps, t.edges_through_boxes, t.edges_doubling_back, t.edges_detouring
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;

    fn runner(script: &str) -> Runner {
        Runner::new(script::parse(script).unwrap())
    }

    fn run_until_action(
        runner: &mut Runner,
        find: impl Fn(&Target) -> Option<Pos2>,
    ) -> (Action, Vec<Event>) {
        let mut events = Vec::new();
        for _ in 0..10_000 {
            let action = runner.advance(&find, &[]);
            events.extend(runner.coming_frames.pop_front().unwrap_or_default());
            if action != Action::Busy {
                return (action, events);
            }
        }
        panic!("the script never got anywhere");
    }

    fn presses(events: &[Event]) -> Vec<(Pos2, PointerButton, bool)> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    ..
                } => Some((*pos, *button, *pressed)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn clicks_where_the_text_is_after_moving_there() {
        let mut r = runner("right-click \"Show log\"\nscreenshot a.png popup");
        let at = pos2(300.0, 200.0);
        let find = |t: &Target| (*t == Target::Text("Show log".into())).then_some(at);
        let (action, events) = run_until_action(&mut r, find);
        assert_eq!(action, Action::Screenshot("a.png".into(), Crop::Popup));
        let secondary = PointerButton::Secondary;
        assert_eq!(
            presses(&events),
            [(at, secondary, true), (at, secondary, false)]
        );
        let first_press = events
            .iter()
            .position(|e| matches!(e, Event::PointerButton { .. }))
            .unwrap();
        assert!(events[..first_press].contains(&Event::PointerMoved(at)));
    }

    #[test]
    fn waits_for_the_screenshot_before_going_on() {
        let mut r = runner("screenshot a.png\nkey Escape");
        assert_eq!(
            r.advance(|_| None, &[]),
            Action::Screenshot("a.png".into(), Crop::Full)
        );
        for _ in 0..5 {
            assert_eq!(r.advance(|_| None, &[]), Action::Busy);
        }
        r.pending_screenshots -= 1;
        let (action, events) = run_until_action(&mut r, |_| None);
        assert_eq!(action, Action::Done);
        assert!(events.iter().any(|e| matches!(
            e,
            Event::Key {
                key: egui::Key::Escape,
                pressed: true,
                ..
            }
        )));
    }

    #[test]
    fn drags_with_the_button_held() {
        let mut r = runner("drag node:main 100,0");
        let at = pos2(50.0, 50.0);
        let (action, events) = run_until_action(&mut r, |_| Some(at));
        assert_eq!(action, Action::Done);
        let primary = PointerButton::Primary;
        let end = at + vec2(100.0, 0.0);
        assert_eq!(
            presses(&events),
            [(at, primary, true), (end, primary, false)]
        );
        let held = events
            .iter()
            .skip_while(|e| !matches!(e, Event::PointerButton { pressed: true, .. }))
            .filter(|e| matches!(e, Event::PointerMoved(_)))
            .count();
        assert!(held >= 20, "moved {held} times with the button down");
    }

    #[test]
    fn types_one_character_at_a_time() {
        let mut r = runner("type \"ab\"");
        let (_, events) = run_until_action(&mut r, |_| None);
        assert_eq!(events, [Event::Text("a".into()), Event::Text("b".into())]);
    }

    #[test]
    fn says_what_is_on_screen_when_a_target_never_shows() {
        let mut r = runner("wait 0.1\nclick \"Merge\"");
        r.waiting_since = Some(Instant::now() - TARGET_TIMEOUT);
        r.idle_frames = 0;
        r.next_line = 1;
        let texts = [("Rebase".to_owned(), Rect::NOTHING)];
        match r.advance(|_| None, &texts) {
            Action::Failed(message) => {
                assert!(
                    message.starts_with("line 2: no Text(\"Merge\")"),
                    "{message}"
                );
                assert!(message.contains("Rebase"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(r.advance(|_| None, &texts), Action::Busy);
    }

    #[test]
    fn finds_the_topmost_exact_text_first() {
        let rect = |x| Rect::from_min_size(pos2(x, 0.0), vec2(10.0, 10.0));
        let texts = [
            ("Merge".to_owned(), rect(0.0)),
            ("Merge branch".to_owned(), rect(100.0)),
            ("Merge".to_owned(), rect(200.0)),
        ];
        assert_eq!(find_text(&texts, "Merge"), Some(pos2(205.0, 5.0)));
        assert_eq!(find_text(&texts, "branch"), Some(pos2(105.0, 5.0)));
        assert_eq!(find_text(&texts, "Rebase"), None);
    }
}
