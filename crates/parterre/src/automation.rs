//! Scripted runs (`--script`, `--screenshot`): the steps of a workflow script
//! ([`crate::script`]) fed to the window frame by frame, as a person's input, then exit. And
//! recording the window (`--record`, [`crate::record`]). See `docs/automation.md`.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Pos2, Rect, vec2};

use crate::record::{self, Recorder};
use crate::scene::Scene;
use crate::script::{self, Crop, Step, Target};
use crate::view::View;

/// A scripted run went wrong: parterre exits with a failure.
static FAILED: AtomicBool = AtomicBool::new(false);

pub fn fail() {
    FAILED.store(true, Ordering::Relaxed);
}

pub fn failed() -> bool {
    FAILED.load(Ordering::Relaxed)
}

#[derive(Debug, Default)]
pub struct Automation {
    /// Start with the whole graph fitted instead of at HEAD.
    pub fit: bool,
    /// Zoom to apply (around the canvas centre) after the initial view is set up.
    pub zoom: Option<f32>,
    /// Something is still loading (a diff, a blame or its history): the script waits.
    pub waiting: bool,
    /// The workflow script being run; `--screenshot` is its last step.
    pub script: Option<Runner>,
    /// Where the window is recorded to.
    pub record: Option<Recorder>,
    /// What a script step asks to open, for the app to take; put back while it can't yet.
    pub open: Option<String>,
    /// The texts on screen in the last frame, and where, for a script to aim at.
    pub texts: Texts,
    /// This frame is a screenshot of the script: no pointer, and not recorded.
    unrecorded: bool,
    /// When an interactive recording started.
    recording_since: Option<Instant>,
    frame: u32,
    frame_times: Vec<Instant>,
}

/// Why a screenshot was taken.
#[derive(Debug)]
enum Shot {
    /// A step of the script, cropped to this part of the window.
    Step(PathBuf, Option<Rect>),
    /// A frame of the recording, shown from this frame time on.
    Frame(u64),
}

impl Automation {
    /// A scripted run: no stored settings, viewports embedded, frames on a clock of their own.
    pub fn is_active(&self) -> bool {
        self.script.is_some()
    }

    /// Feeds the events of the script to the coming frame, and keeps time by frames, so a
    /// recording plays at the speed it would have had however slowly its frames were
    /// captured. Called before each frame, after [`Self::drive`] has counted the last.
    pub fn inject_input(&mut self, raw: &mut egui::RawInput) {
        let Some(runner) = &mut self.script else {
            return;
        };
        raw.time = Some(f64::from(self.frame) / 60.0);
        raw.predicted_dt = 1.0 / 60.0;
        raw.events
            .extend(runner.queue.pop_front().unwrap_or_default());
    }

    /// Automated runs step the physics at a fixed rate so they are reproducible.
    pub fn fixed_dt(&self) -> Option<f32> {
        self.is_active().then_some(1.0 / 60.0)
    }

    /// A step the app carries out (`open`) went wrong: the run stops with a failure.
    pub fn fail(&mut self, ctx: &egui::Context, message: &str) {
        let line = self.script.as_mut().map_or(0, Runner::end);
        eprintln!("parterre: script line {line}: {message}");
        fail();
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Called after each frame. Without a scene (no repository open) a script can still open
    /// and click the window's other parts.
    pub fn drive(
        &mut self,
        ctx: &egui::Context,
        scene: Option<&Scene>,
        view: &mut View,
        canvas: Rect,
    ) {
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
                Some(Shot::Frame(slot)) => {
                    if let Some(record) = &mut self.record {
                        record.push(&image, *slot);
                    }
                }
                Some(Shot::Step(path, crop)) => {
                    let image = match crop {
                        Some(rect) => image.region(rect, Some(ctx.pixels_per_point())),
                        None => (*image).clone(),
                    };
                    save(&image, path);
                    if let Some(runner) = &mut self.script {
                        runner.shots -= 1;
                    }
                }
                None => {}
            }
        }
        if !self.is_active() {
            self.record_frame(ctx, false);
            return;
        }
        self.frame += 1;
        self.frame_times.push(Instant::now());
        ctx.request_repaint();
        if self.frame == 2
            && let Some(z) = self.zoom
        {
            view.zoom_around(canvas, canvas.center(), z / view.zoom);
        }
        self.run_script(ctx, scene, view, canvas);
    }

    /// How fast the frames came, for benchmarks: printed when the run ends.
    fn report_frame_times(&self) {
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
    }

    /// Takes the next step of the script, once the window has settled.
    fn run_script(
        &mut self,
        ctx: &egui::Context,
        scene: Option<&Scene>,
        view: &View,
        canvas: Rect,
    ) {
        const START: u32 = 8;
        let Some(runner) = &mut self.script else {
            return;
        };
        if self.frame < START || self.waiting || self.open.is_some() {
            self.record_frame(ctx, false);
            return;
        }
        let texts = self.texts.lock().map(|t| t.clone()).unwrap_or_default();
        let find = |target: &Target| -> Option<Pos2> {
            match target {
                Target::Text(text) => find_text(&texts, text),
                Target::Node(name) => {
                    let scene = scene?;
                    let n = named_node(scene, name)?;
                    let at = view.to_screen(canvas, scene.node_center(n));
                    canvas.contains(at).then_some(at)
                }
                Target::Canvas => scene.map(|scene| emptiest_spot(scene, view, canvas)),
                Target::Toolbar(name) => ctx
                    .read_response(crate::app::toolbar_button_id(name))
                    .map(|r| r.rect.center()),
                Target::Point(p) => Some(p.to_pos2()),
            }
        };
        let mut shot = false;
        match runner.advance(find, &texts) {
            Action::Busy => {}
            Action::Open(what) => self.open = Some(what),
            Action::Shot(path, crop) => {
                let rect = crop_rect(ctx, crop);
                if crop != Crop::Full && rect.is_none() {
                    eprintln!("{}: no {crop:?} on screen to crop to", path.display());
                }
                let shot_of = Shot::Step(path, rect);
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
                    shot_of,
                )));
                shot = true;
            }
            Action::Done => {
                self.report_frame_times();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Action::Failed(message) => {
                eprintln!("parterre: script {message}");
                fail();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        self.record_frame(ctx, shot);
    }

    /// Paints the pointer and asks for the frame of the recording due, if any. A screenshot
    /// shows the frame after the one asking for it, so a screenshot of the script leaves the
    /// pointer out of that one, and the recording shows the frame before it a moment longer.
    fn record_frame(&mut self, ctx: &egui::Context, screenshot: bool) {
        if self.record.is_none() {
            return;
        }
        if std::mem::take(&mut self.unrecorded) {
            return;
        }
        record::paint_pointer(ctx);
        if screenshot {
            self.unrecorded = true;
            return;
        }
        let slot = if self.is_active() {
            u64::from(self.frame) * u64::from(record::FPS) / 60
        } else {
            let since = *self.recording_since.get_or_insert_with(Instant::now);
            ctx.request_repaint_after(Duration::from_secs_f32(1.0 / record::FPS as f32));
            (since.elapsed().as_secs_f64() * f64::from(record::FPS)) as u64
        };
        let Some(record) = &mut self.record else {
            return;
        };
        if record.requested.is_none_or(|r| slot > r) {
            record.requested = Some(slot);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
                Shot::Frame(slot),
            )));
        }
    }
}

fn save(image: &egui::ColorImage, path: &std::path::Path) {
    let [w, h] = image.size;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(dir);
    }
    match image::RgbaImage::from_raw(w as u32, h as u32, bytes).map(|img| img.save(path)) {
        Some(Ok(())) => eprintln!("saved screenshot to {}", path.display()),
        Some(Err(e)) => {
            eprintln!("could not save screenshot: {e}");
            fail();
        }
        None => eprintln!("screenshot had an unexpected size"),
    }
}

/// The part of the window a screenshot is cropped to: the topmost window, or every open menu
/// and popover, with room for their shadows.
fn crop_rect(ctx: &egui::Context, crop: Crop) -> Option<Rect> {
    let rect = ctx.memory(|m| {
        let areas = m.areas();
        let visible: Vec<_> = m.layer_ids().filter(|l| areas.is_visible(l)).collect();
        match crop {
            Crop::Full => None,
            Crop::Window => visible
                .iter()
                .rev()
                .find(|l| l.order == egui::Order::Middle && areas.parent_layer(**l).is_none())
                .and_then(|l| m.area_rect(l.id)),
            Crop::Popup => visible
                .iter()
                .filter(|l| l.order == egui::Order::Foreground)
                .filter_map(|l| m.area_rect(l.id))
                // Not empty areas, such as that of notifications when there are none.
                .filter(|r| r.width() > 1.0 && r.height() > 1.0)
                .reduce(Rect::union),
        }
    })?;
    Some(rect.expand(16.0).intersect(ctx.content_rect()))
}

/// Where `text` is on screen: the topmost exact match, else the topmost containing it.
fn find_text(texts: &[(String, Rect)], text: &str) -> Option<Pos2> {
    texts
        .iter()
        .rev()
        .find(|(t, _)| t == text)
        .or_else(|| texts.iter().rev().find(|(t, _)| t.contains(text)))
        .map(|(_, r)| r.center())
}

/// The texts painted in the last frame, and where; kept by [`TextCollector`].
pub type Texts = Arc<Mutex<Vec<(String, Rect)>>>;

/// Keeps the texts of every frame, for a script to aim at.
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

/// What the script asks of the window this frame.
#[derive(Debug, PartialEq)]
pub enum Action {
    Busy,
    Open(String),
    Shot(PathBuf, Crop),
    Done,
    Failed(String),
}

/// How long a step waits for its target to show.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Runs a workflow script, frame by frame: each step becomes the input of the frames that
/// follow, moving the pointer there as a person would.
#[derive(Debug)]
pub struct Runner {
    lines: Vec<script::Line>,
    next: usize,
    /// The events of the coming frames, one entry a frame.
    queue: VecDeque<Vec<Event>>,
    pointer: Option<Pos2>,
    /// Frames to do nothing.
    hold: u32,
    /// Since when the step has been waiting for its target.
    since: Option<Instant>,
    /// Screenshots asked for and not saved yet.
    shots: usize,
    finished: bool,
    /// Done or failed: the window is closing.
    ended: bool,
}

impl Runner {
    pub fn new(lines: Vec<script::Line>) -> Runner {
        Runner {
            lines,
            next: 0,
            queue: VecDeque::new(),
            pointer: None,
            hold: 0,
            since: None,
            shots: 0,
            finished: false,
            ended: false,
        }
    }

    /// Stops the script: the window is closing. The line of the last step taken.
    fn end(&mut self) -> usize {
        self.ended = true;
        self.next
            .checked_sub(1)
            .and_then(|i| self.lines.get(i))
            .map_or(0, |l| l.number)
    }

    /// Adds a screenshot of the whole window after the last step.
    pub fn push_screenshot(&mut self, path: PathBuf) {
        let number = self.lines.last().map_or(0, |l| l.number) + 1;
        self.lines.push(script::Line {
            number,
            step: Step::Screenshot(path, Crop::Full),
        });
    }

    /// Takes the next step if the last is done. `find` says where a target is on screen.
    pub fn advance(
        &mut self,
        find: impl Fn(&Target) -> Option<Pos2>,
        texts: &[(String, Rect)],
    ) -> Action {
        if !self.queue.is_empty() || self.shots > 0 || self.ended {
            return Action::Busy;
        }
        if self.hold > 0 {
            self.hold -= 1;
            return Action::Busy;
        }
        let Some(line) = self.lines.get(self.next).cloned() else {
            // Stay a moment on the end, for a recording to show it.
            if !self.finished {
                self.finished = true;
                self.hold = 15;
                return Action::Busy;
            }
            self.ended = true;
            return Action::Done;
        };
        let target = match &line.step {
            Step::WaitFor(t)
            | Step::Click { target: t, .. }
            | Step::Hover(t)
            | Step::Drag(t, _) => match find(t) {
                Some(at) => Some(at),
                None => return self.missing(&line, t, texts),
            },
            _ => None,
        };
        self.since = None;
        self.next += 1;
        let empty = |n| std::iter::repeat_n(Vec::new(), n);
        match line.step {
            Step::Wait(seconds) => self.hold = (seconds * 60.0).round().max(0.0) as u32,
            Step::WaitFor(_) => {}
            Step::Click {
                button,
                count,
                modifiers,
                ..
            } => {
                let at = target.unwrap_or_default();
                self.glide(at, false);
                // A moment over it, to show its hover.
                self.queue.extend(empty(12));
                if !modifiers.is_none() {
                    self.queue
                        .push_back(vec![Event::ModifiersChanged(modifiers)]);
                }
                let button = match button {
                    script::Button::Primary => egui::PointerButton::Primary,
                    script::Button::Secondary => egui::PointerButton::Secondary,
                };
                for _ in 0..count {
                    for pressed in [true, false] {
                        self.queue.push_back(vec![Event::PointerButton {
                            pos: at,
                            button,
                            pressed,
                            modifiers,
                        }]);
                    }
                }
                if !modifiers.is_none() {
                    self.queue
                        .push_back(vec![Event::ModifiersChanged(egui::Modifiers::NONE)]);
                }
                self.queue.extend(empty(12));
            }
            Step::Hover(_) => {
                self.glide(target.unwrap_or_default(), false);
                self.queue.extend(empty(20));
            }
            Step::Drag(_, by) => {
                let at = target.unwrap_or_default();
                self.glide(at, false);
                self.queue.extend(empty(6));
                let button = |pos, pressed| Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                self.queue.push_back(vec![button(at, true)]);
                self.queue.extend(empty(4));
                self.glide(at + by, true);
                self.queue.extend(empty(4));
                self.queue.push_back(vec![button(at + by, false)]);
                self.queue.extend(empty(12));
            }
            Step::Scroll(delta) => {
                self.queue.push_back(vec![Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta,
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                }]);
                self.queue.extend(empty(12));
            }
            Step::Key(modifiers, key) => {
                for pressed in [true, false] {
                    self.queue.push_back(vec![Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers,
                    }]);
                }
                self.queue.extend(empty(12));
            }
            Step::Type(text) => {
                // A few characters a second, as typed.
                for c in text.chars() {
                    self.queue.push_back(vec![Event::Text(c.to_string())]);
                    self.queue.extend(empty(3));
                }
                self.queue.extend(empty(8));
            }
            Step::Open(what) => {
                self.hold = 12;
                return Action::Open(what);
            }
            Step::Screenshot(path, crop) => {
                self.shots += 1;
                return Action::Shot(path, crop);
            }
        }
        Action::Busy
    }

    /// The target isn't on screen (yet).
    fn missing(
        &mut self,
        line: &script::Line,
        target: &Target,
        texts: &[(String, Rect)],
    ) -> Action {
        let since = *self.since.get_or_insert_with(Instant::now);
        if since.elapsed() < TIMEOUT {
            return Action::Busy;
        }
        self.ended = true;
        let mut shown: Vec<&str> = Vec::new();
        for (t, _) in texts.iter().rev() {
            if !shown.contains(&t.as_str()) && shown.len() < 40 {
                shown.push(t);
            }
        }
        Action::Failed(format!(
            "line {}: no {target:?} on screen; the topmost texts: {shown:?}",
            line.number
        ))
    }

    /// Moves the pointer to `to` over a few frames, as a hand would.
    fn glide(&mut self, to: Pos2, held: bool) {
        let from = self.pointer.unwrap_or(to);
        let frames = ((from.distance(to) / 25.0) as usize).clamp(if held { 20 } else { 8 }, 40);
        for i in 1..=frames {
            let t = i as f32 / frames as f32;
            let eased = t * t * (3.0 - 2.0 * t);
            self.queue
                .push_back(vec![Event::PointerMoved(from.lerp(to, eased))]);
        }
        self.pointer = Some(to);
    }
}

/// The node of a ref name or hash prefix.
fn named_node(scene: &Scene, name: &str) -> Option<usize> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;

    fn runner(script: &str) -> Runner {
        Runner::new(script::parse(script).unwrap())
    }

    /// Runs frames until the script asks for something other than more frames, as the window
    /// would, with `find` as what is on screen; and the events it fed the frames.
    fn run(runner: &mut Runner, find: impl Fn(&Target) -> Option<Pos2>) -> (Action, Vec<Event>) {
        let mut events = Vec::new();
        for _ in 0..10_000 {
            let action = runner.advance(&find, &[]);
            events.extend(runner.queue.pop_front().unwrap_or_default());
            if action != Action::Busy {
                return (action, events);
            }
        }
        panic!("the script never got anywhere");
    }

    fn presses(events: &[Event]) -> Vec<(Pos2, egui::PointerButton, bool)> {
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
        let (action, events) = run(&mut r, find);
        assert_eq!(action, Action::Shot("a.png".into(), Crop::Popup));
        let secondary = egui::PointerButton::Secondary;
        assert_eq!(
            presses(&events),
            [(at, secondary, true), (at, secondary, false)]
        );
        // The pointer got there before pressing.
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
            Action::Shot("a.png".into(), Crop::Full)
        );
        for _ in 0..5 {
            assert_eq!(r.advance(|_| None, &[]), Action::Busy);
        }
        r.shots -= 1;
        let (action, events) = run(&mut r, |_| None);
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
        let (action, events) = run(&mut r, |_| Some(at));
        assert_eq!(action, Action::Done);
        let primary = egui::PointerButton::Primary;
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
        let (_, events) = run(&mut r, |_| None);
        assert_eq!(events, [Event::Text("a".into()), Event::Text("b".into())]);
    }

    #[test]
    fn says_what_is_on_screen_when_a_target_never_shows() {
        let mut r = runner("wait 0.1\nclick \"Merge\"");
        r.since = Some(Instant::now() - TIMEOUT);
        r.hold = 0;
        r.next = 1;
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
        // And stops there.
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
