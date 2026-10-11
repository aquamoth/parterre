use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2, Rect, Vec2, vec2};
use parterre_core::branches::Catalog;
use parterre_core::revgraph::GraphOptions;
use parterre_core::{Oid, Repo};

use super::branches::{Request, Tool};

fn git_command(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(parterre_core::git::program());
    command
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com");
    command
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_command(dir, args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A new repository on `main` with the test identity, which parterre's own git reads from the
/// repository (CI has no global one). The config file is written at once rather than with a
/// `git config` call each (#309), and turns off automatic maintenance, which git starts as a
/// process of its own after every commit.
pub fn init(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    let config = dir.join(".git").join("config");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&config)
        .unwrap_or_else(|e| panic!("open {}: {e}", config.display()));
    std::io::Write::write_all(
        &mut file,
        b"[user]\n\tname = Test\n\temail = test@example.com\n[maintenance]\n\tauto = false\n",
    )
    .unwrap();
}

/// Without the CRLF line endings `core.autocrlf` gives files checked out on Windows (#182).
pub fn read(dir: &Path, path: &str) -> String {
    std::fs::read_to_string(dir.join(path))
        .unwrap()
        .replace("\r\n", "\n")
}

pub fn write(dir: &Path, path: &str, text: &str) {
    std::fs::write(dir.join(path), text).unwrap();
}

pub fn raw_input(size: Vec2, events: Vec<Event>) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
        events,
        ..Default::default()
    }
}

/// Returns the texts on screen, and where.
pub fn run_frame(
    ctx: &egui::Context,
    input: egui::RawInput,
    ui: impl FnMut(&mut egui::Ui),
) -> Vec<(String, Rect)> {
    let mut output = ctx.run_ui(input, ui);
    output.textures_delta.clear();
    let mut texts = Vec::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, &mut texts);
    }
    texts
}

/// egui reports them on screen.
pub fn assert_no_id_clashes(texts: &[(String, Rect)]) {
    let clashes: Vec<_> = texts
        .iter()
        .filter(|(t, _)| t.contains("use of") || t.contains("is above this"))
        .collect();
    assert!(clashes.is_empty(), "egui reports id clashes: {clashes:?}");
}

pub fn text_at(texts: &[(String, Rect)], text: &str) -> Pos2 {
    texts
        .iter()
        .find(|(t, _)| t == text)
        .unwrap_or_else(|| panic!("no {text:?} on screen: {texts:?}"))
        .1
        .center()
}

/// The press and the release, a frame's events each.
pub fn click_events(at: Pos2, button: PointerButton, modifiers: Modifiers) -> [Vec<Event>; 2] {
    [true, false].map(|pressed| {
        vec![
            Event::PointerMoved(at),
            Event::PointerButton {
                pos: at,
                button,
                pressed,
                modifiers,
            },
        ]
    })
}

pub struct Harness {
    pub ctx: egui::Context,
    pub tool: Tool,
    pub repo: Arc<Repo>,
    pub dir: tempfile::TempDir,
    pub time: f64,
    pub events: Vec<Event>,
    pub modifiers: Modifiers,
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
            modifiers: Modifiers::NONE,
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

    /// Fails on any widget id clash egui reports.
    pub fn frame(&mut self) {
        self.time += 1.0 / 60.0;
        let input = egui::RawInput {
            time: Some(self.time),
            ..raw_input(vec2(1600.0, 1000.0), std::mem::take(&mut self.events))
        };
        let (tool, repo) = (&mut self.tool, &self.repo);
        self.texts = run_frame(&self.ctx, input, |ui| {
            tool.update(ui.ctx(), Some(repo));
            let palette = crate::theme::Palette::new(false, &[]);
            tool.show(ui.ctx(), &palette, &GraphOptions::default());
        });
        assert_no_id_clashes(&self.texts);
    }

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
        text_at(&self.texts, text)
    }

    pub fn click_at(&mut self, at: Pos2, count: usize) {
        for _ in 0..count {
            for events in click_events(at, PointerButton::Primary, self.modifiers) {
                self.events.extend(events);
                self.frame();
            }
        }
        self.frame();
    }

    pub fn click_with(&mut self, text: &str, modifiers: Modifiers) {
        self.modifiers = modifiers;
        self.events.push(Event::ModifiersChanged(modifiers));
        self.click(text);
        self.modifiers = Modifiers::NONE;
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

    pub fn type_text(&mut self, text: &str) {
        self.events.push(Event::Text(text.to_owned()));
        self.frame();
        self.frame();
    }

    pub fn ask(&mut self, request: Request, title: &str) {
        let ctx = self.ctx.clone();
        self.tool.request(&ctx, request, egui::ViewportId::ROOT);
        self.until("the dialog opens", |h| h.shows(title));
        // Its first frames place and size it.
        for _ in 0..5 {
            self.frame();
        }
    }

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

/// The texts `f` shows in a frame `size` big, and what it returns when `click` is clicked.
fn show_and_click<T>(
    size: Vec2,
    mut f: impl FnMut(&mut egui::Ui) -> Option<T>,
    click: Option<&str>,
) -> (Vec<String>, Option<T>) {
    let ctx = egui::Context::default();
    let mut frame = |events: Vec<Event>| {
        let mut returned = None;
        let texts = run_frame(&ctx, raw_input(size, events), |ui| returned = f(ui));
        (texts, returned)
    };
    let (mut texts, _) = frame(Vec::new());
    let mut returned = None;
    if let Some(text) = click {
        let at = text_at(&texts, text);
        for events in click_events(at, PointerButton::Primary, Modifiers::NONE) {
            let (shown, now) = frame(events);
            texts = shown;
            returned = returned.or(now);
        }
    }
    (texts.into_iter().map(|(t, _)| t).collect(), returned)
}

/// The texts a menu shows, and what clicking `click` asks for.
pub fn menu(
    f: impl Fn(&mut egui::Ui) -> Option<Request>,
    click: Option<&str>,
) -> (Vec<String>, Option<Request>) {
    show_and_click(vec2(800.0, 600.0), f, click)
}

pub fn load(dir: &Path) -> (Repo, Catalog) {
    let repo = parterre_core::git::load_repo(dir).unwrap();
    let catalog = Catalog::of(&repo).unwrap();
    (repo, catalog)
}

pub fn banner_texts(dir: &Path) -> Vec<String> {
    let (repo, catalog) = load(dir);
    let ctx = egui::Context::default();
    let texts = run_frame(&ctx, egui::RawInput::default(), |ui| {
        super::rebase::banner(ui, &repo, &catalog);
    });
    texts.into_iter().map(|(t, _)| t).collect()
}

/// What clicking `click` in the banner of the repository at `dir` asks for.
pub fn banner_click(dir: &Path, click: &str) -> Option<Oid> {
    let (repo, catalog) = load(dir);
    let banner = |ui: &mut egui::Ui| super::rebase::banner(ui, &repo, &catalog);
    show_and_click(vec2(1200.0, 600.0), banner, Some(click)).1
}

/// A merge of `feature` into `main` that stops with `text.txt` conflicted in content and
/// `gone.txt` deleted by `feature` but changed on `main`.
pub fn stuck_merge() -> tempfile::TempDir {
    let dir = before_merge();
    merge_stops(dir.path());
    dir
}

/// [`stuck_merge`]'s history before `main` changes and merges.
pub fn before_merge() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    init(p);
    write(p, "text.txt", "one\ntwo\n");
    write(p, "gone.txt", "base\n");
    git(p, &["add", "-A"]);
    git(p, &["commit", "-q", "-m", "Base"]);
    git(p, &["switch", "-q", "-c", "feature"]);
    write(p, "text.txt", "one\nTWO (feature)\n");
    git(p, &["rm", "-q", "gone.txt"]);
    git(p, &["commit", "-q", "-am", "Feature"]);
    git(p, &["switch", "-q", "main"]);
    dir
}

/// On `main` of [`before_merge`]'s history: changes both files and merges `feature`, which
/// stops.
pub fn merge_stops(p: &Path) {
    write(p, "text.txt", "one\n2 (main)\n");
    write(p, "gone.txt", "main\n");
    git(p, &["commit", "-q", "-am", "Main"]);
    let out = git_command(p, &["merge", "-q", "feature"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "the merge stops");
}
