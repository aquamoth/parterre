//! A headless harness for the branch tool's dialogs: frame by frame against real, disposable
//! repositories, clicked by the text on screen, as a person would, failing on any widget id
//! clash egui reports.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Pos2, Rect};
use parterre_core::branches::Catalog;
use parterre_core::revgraph::GraphOptions;
use parterre_core::{Oid, Repo};

use super::branches::{Request, Tool};

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A file's text, with the line endings `core.autocrlf` gives it on checkout (Windows)
/// undone, as `common::read_text` in parterre-core's tests (#182).
pub fn read(dir: &Path, path: &str) -> String {
    std::fs::read_to_string(dir.join(path))
        .unwrap()
        .replace("\r\n", "\n")
}

pub fn write(dir: &Path, path: &str, text: &str) {
    std::fs::write(dir.join(path), text).unwrap();
}

pub struct Harness {
    pub ctx: egui::Context,
    pub tool: Tool,
    pub repo: Arc<Repo>,
    pub dir: tempfile::TempDir,
    pub time: f64,
    pub events: Vec<Event>,
    /// The modifier keys held.
    pub modifiers: egui::Modifiers,
    /// The texts on screen in the last frame, and where.
    pub texts: Vec<(String, Rect)>,
}

impl Harness {
    pub fn new(dir: tempfile::TempDir) -> Harness {
        let repo = Arc::new(parterre_core::git::load_repo(dir.path()).unwrap());
        let mut h = Harness {
            ctx: egui::Context::default(),
            tool: Tool::default(),
            repo,
            dir,
            time: 0.0,
            events: Vec::new(),
            modifiers: egui::Modifiers::NONE,
            texts: Vec::new(),
        };
        h.frame();
        h
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn rev(&self, rev: &str) -> Oid {
        Oid::from_hex(&git(self.path(), &["rev-parse", rev])).unwrap()
    }

    /// One frame; fails on any widget id clash egui reports.
    pub fn frame(&mut self) {
        self.time += 1.0 / 60.0;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
            time: Some(self.time),
            events: std::mem::take(&mut self.events),
            ..Default::default()
        };
        let (tool, repo) = (&mut self.tool, &self.repo);
        let mut output = self.ctx.run_ui(input, |ui| {
            tool.update(ui.ctx(), Some(repo));
            let palette = crate::theme::Palette::new(false, &[]);
            tool.show(ui.ctx(), &palette, &GraphOptions::default());
        });
        output.textures_delta.clear();
        self.texts.clear();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut self.texts);
        }
        let clashes: Vec<_> = self
            .texts
            .iter()
            .filter(|(t, _)| t.contains("use of") || t.contains("is above this"))
            .collect();
        assert!(clashes.is_empty(), "egui reports id clashes: {clashes:?}");
    }

    /// Frames until `done`, or fails after a while.
    pub fn until(&mut self, what: &str, done: impl Fn(&Harness) -> bool) {
        let start = Instant::now();
        while !done(self) {
            assert!(start.elapsed() < Duration::from_secs(20), "never: {what}");
            std::thread::sleep(Duration::from_millis(5));
            self.frame();
        }
    }

    pub fn shows(&self, text: &str) -> bool {
        self.texts.iter().any(|(t, _)| t == text)
    }

    pub fn shows_part(&self, text: &str) -> bool {
        self.texts.iter().any(|(t, _)| t.contains(text))
    }

    pub fn at(&self, text: &str) -> Pos2 {
        self.texts
            .iter()
            .find(|(t, _)| t == text)
            .unwrap_or_else(|| panic!("no {text:?} on screen: {:?}", self.texts))
            .1
            .center()
    }

    pub fn click_at(&mut self, at: Pos2, count: usize) {
        for _ in 0..count {
            for pressed in [true, false] {
                self.events.push(Event::PointerMoved(at));
                self.events.push(Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: self.modifiers,
                });
                self.frame();
            }
        }
        self.frame();
    }

    /// A click on `text` with `modifiers` held.
    pub fn click_with(&mut self, text: &str, modifiers: egui::Modifiers) {
        self.modifiers = modifiers;
        self.events.push(Event::ModifiersChanged(modifiers));
        self.click(text);
        self.modifiers = egui::Modifiers::NONE;
        self.events.push(Event::ModifiersChanged(self.modifiers));
        self.frame();
    }

    pub fn click(&mut self, text: &str) {
        let at = self.at(text);
        self.click_at(at, 1);
    }

    pub fn key(&mut self, key: egui::Key) {
        for pressed in [true, false] {
            self.events.push(Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: self.modifiers,
            });
            self.frame();
        }
        self.frame();
    }

    /// Types `text` where the keyboard focus is.
    pub fn type_text(&mut self, text: &str) {
        self.events.push(Event::Text(text.to_owned()));
        self.frame();
        self.frame();
    }

    /// Asks the tool for `request` and waits for `title` to show.
    pub fn ask(&mut self, request: Request, title: &str) {
        let ctx = self.ctx.clone();
        self.tool.request(&ctx, request, egui::ViewportId::ROOT);
        self.until("the dialog opens", |h| h.shows(title));
        // Its first frames place and size it.
        for _ in 0..5 {
            self.frame();
        }
    }

    /// The repository loaded again, as a reload after a change does.
    pub fn reload(&mut self) {
        self.repo = Arc::new(parterre_core::git::load_repo(self.path()).unwrap());
        self.frame();
    }
}

pub fn collect(shape: &egui::Shape, texts: &mut Vec<(String, Rect)>) {
    match shape {
        // Where its glyphs are: text wrapped after another starts its galley further left.
        egui::Shape::Text(t) => {
            let rect = t
                .galley
                .rows
                .iter()
                .filter(|r| !r.glyphs.is_empty())
                .map(|r| r.rect_without_leading_space())
                .reduce(|a, b| a.union(b))
                .unwrap_or(t.galley.rect);
            texts.push((t.galley.text().to_owned(), rect.translate(t.pos.to_vec2())));
        }
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, texts)),
        _ => {}
    }
}

/// The texts a menu shows, and what clicking `click` (if given) asks for.
pub fn menu(
    f: impl Fn(&mut egui::Ui) -> Option<Request>,
    click: Option<&str>,
) -> (Vec<String>, Option<Request>) {
    let ctx = egui::Context::default();
    let frame = |events: Vec<egui::Event>, texts: &mut Vec<(String, Rect)>| {
        let mut asked = None;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| asked = f(ui));
        output.textures_delta.clear();
        texts.clear();
        for clipped in &output.shapes {
            collect(&clipped.shape, texts);
        }
        asked
    };
    let mut texts = Vec::new();
    frame(Vec::new(), &mut texts);
    let mut asked = None;
    if let Some(text) = click {
        let at = texts
            .iter()
            .find(|(t, _)| t == text)
            .unwrap_or_else(|| panic!("no {text:?}: {texts:?}"))
            .1
            .center();
        for pressed in [true, false] {
            let events = vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ];
            asked = asked.or(frame(events, &mut texts));
        }
    }
    (texts.into_iter().map(|(t, _)| t).collect(), asked)
}

pub fn load(dir: &Path) -> (Repo, Catalog) {
    (
        parterre_core::git::load_repo(dir).unwrap(),
        Catalog::load(dir).unwrap(),
    )
}

/// The texts `banner` shows for the repository at `dir`.
pub fn banner_texts(dir: &Path) -> Vec<String> {
    let (repo, catalog) = load(dir);
    let ctx = egui::Context::default();
    let mut texts = Vec::new();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        super::rebase::banner(ui, &repo, &catalog);
    });
    output.textures_delta.clear();
    for clipped in &output.shapes {
        collect(&clipped.shape, &mut texts);
    }
    texts.into_iter().map(|(t, _)| t).collect()
}
