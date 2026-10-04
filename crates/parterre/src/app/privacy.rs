//! Usage statistics and crash reports (#261): the first-run prompt, and the usage statistics
//! sent as the user's choices say. Decided in #223, #225 and #227; Settings › Privacy shows and
//! changes the choices (`settings_window.rs`).
//!
//! Nothing is sent before the prompt is answered, the update check included. Scripted runs never
//! send, and show the prompt only when a script opens it (`open first-run`).
//!
//! The usage statistics are told of the user's input ([`UserInput`]) and the repositories opened,
//! which decide their sessions (#262).

use std::path::{Path, PathBuf};

use eframe::egui::{self, Align, Layout, RichText, Ui, vec2};
use parterre_core::Repo;
use parterre_telemetry::{Build, Choices, Context, Lifecycle, Usage};

use super::ParterreApp;
use crate::settings::{Privacy, PrivacyAnswer};
use crate::widgets;

pub(super) const USAGE: &str = "Usage statistics";
pub(super) const USAGE_TEXT: &str = "Installs, launches and which features are used, with a \
    random ID for this installation. Never personal information.";
pub(super) const CRASHES: &str = "Crash reports";
pub(super) const CRASHES_TEXT: &str = "What parterre was doing when it crashed. May contain \
    personal information, such as a file or branch name.";
const WHERE: &str = "Sent to PostHog in the EU. Settings › Privacy changes either later.";

/// The install ID scripted runs show: they make none, and their pictures stay the same.
const EXAMPLE_ID: &str = "6f1c2b7e-3a4d-4e8f-9b0a-1c2d3e4f5a6b";

/// How many frames the start of the usage statistics waits for the screen's size.
const SCREEN_WAIT: u32 = 30;

/// What is sent to PostHog, and the first-run prompt.
#[derive(Debug)]
pub(super) struct Telemetry {
    /// The choices, the install ID and the version that ran last, as stored.
    pub privacy: Privacy,
    /// The first-run prompt, while it is up: what is ticked in it.
    pub prompt: Option<Choices>,
    /// Crash reports as parterre started with them: a change takes effect at the next start.
    pub crash_reports_at_start: bool,
    /// `DO_NOT_TRACK` is set: nothing goes to PostHog, and the switches say so.
    pub do_not_track: bool,
    /// A scripted run, which sends nothing.
    scripted: bool,
    /// Usage statistics on their way, while they are sent.
    usage: Option<Usage>,
    /// The launch's events, until they are sent or the user is known not to want them.
    pending: Vec<Lifecycle>,
    /// Frames waited for the screen's size before the launch is told; `None` once told, or
    /// while the prompt is up.
    launching: Option<u32>,
    /// The launch is this installation's first.
    first_run: bool,
    /// The screen's size the launch was told with.
    screen: Option<[f32; 2]>,
    /// The repository opened last ([`repository_of`]), for the usage statistics started after.
    repository: Option<PathBuf>,
}

impl Telemetry {
    pub fn new(mut privacy: Privacy, scripted: bool) -> Telemetry {
        let do_not_track = parterre_telemetry::do_not_track();
        // Scripted runs read no stored settings: as if answered with the defaults.
        if scripted && privacy.answer.is_none() && parterre_telemetry::has_usage_statistics() {
            let choices = Choices::default();
            privacy.answer = Some(PrivacyAnswer {
                install_id: EXAMPLE_ID.to_owned(),
                usage_statistics: choices.usage_statistics,
                crash_reports: choices.crash_reports,
            });
        }
        let answered = privacy.answer.is_some();
        let asks = parterre_telemetry::asks(Build::THIS, answered, do_not_track) && !scripted;
        Telemetry {
            crash_reports_at_start: privacy.choices().is_some_and(|c| c.crash_reports),
            privacy,
            prompt: asks.then(Choices::default),
            do_not_track,
            scripted,
            usage: None,
            pending: Vec::new(),
            launching: (!asks).then_some(0),
            first_run: false,
            screen: None,
            repository: None,
        }
    }

    /// The user opened `repo`: another repository than the one opened last starts a new
    /// session.
    pub fn opened(&mut self, repo: &Repo) {
        let repository = repository_of(repo);
        if let Some(usage) = &mut self.usage {
            usage.opened(repository);
        }
        self.repository = Some(repository.to_owned());
    }

    /// Whether the usage statistics are to be sent now.
    fn sends(&self) -> bool {
        let choices = self.privacy.choices();
        !self.scripted && parterre_telemetry::sends(Build::THIS, choices, self.do_not_track)
    }

    /// Tells the launch once the screen's size is known (or after a while without), and then
    /// starts and stops the usage statistics as the user's choices say.
    fn update(&mut self, ctx: &egui::Context) {
        let input = ctx.with_plugin(|seen: &mut UserInput| std::mem::take(&mut seen.0));
        if input == Some(true)
            && let Some(usage) = &mut self.usage
        {
            usage.input();
        }
        if self.prompt.is_some() {
            return;
        }
        if let Some(waited) = self.launching {
            let screen = ctx
                .input(|i| i.viewport().monitor_size)
                // In logical pixels, whatever the text size.
                .map(|size| (size * ctx.zoom_factor()).into());
            if screen.is_none() && waited < SCREEN_WAIT {
                self.launching = Some(waited + 1);
                ctx.request_repaint();
                return;
            }
            self.launching = None;
            self.screen = screen;
            let last = self.privacy.last_version.as_deref();
            self.pending = parterre_telemetry::launch(self.first_run, last, crate::VERSION);
            self.privacy.last_version = Some(parterre_telemetry::app_version(crate::VERSION));
        }
        if !self.sends() {
            // Unticked: nothing more is sent, the launch's events included.
            self.usage = None;
            self.pending.clear();
        } else if self.usage.is_none()
            && let Some(answer) = &self.privacy.answer
        {
            let context = Context {
                install_id: answer.install_id.clone(),
                version: crate::VERSION.to_owned(),
                screen: self.screen,
                git_version: parterre_core::git::version,
            };
            let events = std::mem::take(&mut self.pending);
            self.usage = Usage::start(Some(answer.choices()), context, events);
            // The repository open as they start is their first session's.
            if let (Some(usage), Some(repository)) = (&mut self.usage, &self.repository) {
                usage.opened(repository);
            }
        }
    }

    /// The prompt answered: the install ID is made, and the launch is told as a first run.
    fn answer(&mut self, choices: Choices) {
        self.privacy.answer(choices);
        self.prompt = None;
        self.crash_reports_at_start = choices.crash_reports;
        self.first_run = true;
        self.launching = Some(0);
    }

    /// The window closes: `Application Backgrounded`, and what is queued is sent.
    pub fn close(&mut self) {
        if let Some(usage) = self.usage.take() {
            usage.close();
        }
    }
}

/// A repository as the usage statistics' sessions tell them apart: by its main worktree, the
/// same whichever of its worktrees is open (or by its own folder when git lists none).
fn repository_of(repo: &Repo) -> &Path {
    repo.worktrees.first().map_or(&repo.path, |main| &main.path)
}

/// Notes whether the user gave input, in any of parterre's windows, since the usage statistics
/// last asked: a key, the pointer, the wheel, a touch. Registered once, with
/// [`egui::Context::add_plugin`]; each window's input passes through it.
#[derive(Debug, Default)]
pub(super) struct UserInput(bool);

impl egui::Plugin for UserInput {
    fn debug_name(&self) -> &'static str {
        "parterre-user-input"
    }

    fn input_hook(&mut self, _ctx: &egui::Context, input: &mut egui::RawInput) {
        self.0 |= input.events.iter().any(is_user_input);
    }
}

/// Whether `event` is the user's doing, not the window's or egui's own.
fn is_user_input(event: &egui::Event) -> bool {
    !matches!(
        event,
        egui::Event::WindowFocused(_) | egui::Event::PointerGone | egui::Event::Screenshot { .. }
    )
}

impl ParterreApp {
    /// The usage statistics, as the user's choices say. Call every frame.
    pub(super) fn usage_statistics(&mut self, ctx: &egui::Context) {
        self.telemetry.update(ctx);
    }

    /// Automation: the first-run prompt, as at the first start (`open first-run`).
    pub(super) fn open_first_run_prompt(&mut self) {
        self.telemetry.prompt = Some(Choices::default());
    }

    /// The first-run prompt, while it is up; once answered, the answer is stored at once.
    pub(super) fn first_run_prompt(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Stored at once: the install ID is never made twice.
        if self.telemetry.prompt(ctx)
            && let Some(storage) = frame.storage_mut()
        {
            eframe::App::save(self, storage);
            storage.flush();
        }
    }
}

impl Telemetry {
    /// The first-run prompt (#227), while it is up: a modal dialog that closes only through
    /// *Continue*. No close button, no Esc, no "later". True once answered.
    fn prompt(&mut self, ctx: &egui::Context) -> bool {
        let Some(choices) = &mut self.prompt else {
            return false;
        };
        let shown = crate::dialogs::Dialog::new("first-run", "Usage statistics and crash reports")
            .width(430.0)
            .modal()
            .undismissable()
            .show(ctx, |ui| {
                choice(ui, &mut choices.usage_statistics, USAGE, USAGE_TEXT);
                ui.add_space(10.0);
                choice(ui, &mut choices.crash_reports, CRASHES, CRASHES_TEXT);
                ui.add_space(12.0);
                ui.label(RichText::new(WHERE).weak());
                ui.add_space(12.0);
                ui.separator();
                let row = vec2(ui.available_width(), 34.0);
                ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                    widgets::primary_button(ui, "Continue", 90.0).clicked()
                })
                .inner
            });
        // Closing its window does nothing (where the platform still shows a close button).
        if !shown.inner {
            return false;
        }
        let choices = *choices;
        self.answer(choices);
        true
    }
}

/// A checkbox with its explanation underneath, lined up with its label.
fn choice(ui: &mut Ui, on: &mut bool, label: &str, text: &str) {
    ui.checkbox(on, RichText::new(label).strong());
    ui.horizontal(|ui| {
        ui.add_space(ui.spacing().icon_width + ui.spacing().icon_spacing);
        ui.add(egui::Label::new(RichText::new(text).weak()).wrap());
    });
}

#[cfg(test)]
mod tests {
    use eframe::egui::{Event, Key, Pos2, Rect};

    use super::*;

    /// The prompt in a headless window, clicked by the text on screen as a person would.
    struct Screen {
        ctx: egui::Context,
        telemetry: Telemetry,
        time: f64,
        events: Vec<Event>,
        texts: Vec<(String, Rect)>,
    }

    impl Screen {
        fn new() -> Screen {
            let ctx = egui::Context::default();
            // As in scripted runs: the dialog drawn in the main window.
            ctx.set_embed_viewports(true);
            let mut telemetry = Telemetry::new(Privacy::default(), false);
            telemetry.prompt = Some(Choices::default());
            telemetry.launching = None;
            let mut screen = Screen {
                ctx,
                telemetry,
                time: 0.0,
                events: Vec::new(),
                texts: Vec::new(),
            };
            for _ in 0..3 {
                screen.frame();
            }
            screen
        }

        fn frame(&mut self) {
            self.time += 1.0 / 60.0;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0))),
                time: Some(self.time),
                events: std::mem::take(&mut self.events),
                ..Default::default()
            };
            let telemetry = &mut self.telemetry;
            let mut output = self.ctx.run_ui(input, |ui| {
                telemetry.prompt(ui.ctx());
            });
            output.textures_delta.clear();
            self.texts.clear();
            for clipped in &output.shapes {
                super::super::tool_harness::collect(&clipped.shape, &mut self.texts);
            }
        }

        fn shows(&self, text: &str) -> bool {
            self.texts.iter().any(|(t, _)| t == text)
        }

        fn press(&mut self, at: Pos2) {
            for pressed in [true, false] {
                self.events.push(Event::PointerMoved(at));
                self.events.push(Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
                self.frame();
            }
            self.frame();
        }

        fn click(&mut self, text: &str) {
            let at = self
                .texts
                .iter()
                .find(|(t, _)| t == text)
                .unwrap_or_else(|| panic!("no {text:?} on screen: {:?}", self.texts))
                .1
                .center();
            self.press(at);
        }

        fn key(&mut self, key: Key) {
            for pressed in [true, false] {
                self.events.push(Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                });
                self.frame();
            }
            self.frame();
        }
    }

    #[test]
    fn the_prompt_says_what_was_decided() {
        let screen = Screen::new();
        for text in [
            "Usage statistics and crash reports",
            USAGE,
            CRASHES,
            WHERE,
            "Continue",
        ] {
            assert!(screen.shows(text), "{text:?}: {:?}", screen.texts);
        }
        assert!(!screen.shows("Close") && !screen.shows("Cancel"));
    }

    #[test]
    fn esc_enter_and_clicks_beside_it_leave_the_prompt_up() {
        let mut screen = Screen::new();
        screen.key(Key::Escape);
        screen.key(Key::Enter);
        screen.click("Usage statistics and crash reports");
        screen.press(Pos2::new(5.0, 5.0));
        screen.press(Pos2::new(1190.0, 790.0));
        assert_eq!(screen.telemetry.prompt, Some(Choices::default()));
        assert!(screen.telemetry.privacy.answer.is_none());
        assert!(screen.shows("Continue"));
    }

    #[test]
    fn continue_answers_with_what_is_ticked() {
        let mut screen = Screen::new();
        screen.click(USAGE);
        screen.click(CRASHES);
        assert!(
            screen.telemetry.privacy.answer.is_none(),
            "not before Continue"
        );
        screen.click("Continue");
        assert_eq!(screen.telemetry.prompt, None);
        assert!(!screen.shows("Continue"));
        let answer = screen.telemetry.privacy.answer.clone().unwrap();
        assert!(!answer.usage_statistics);
        assert!(answer.crash_reports);
        assert_eq!(answer.install_id.len(), 36);
        // The first run, told once the window has its size.
        assert!(screen.telemetry.first_run);
        assert!(screen.telemetry.crash_reports_at_start);
        assert_eq!(screen.telemetry.launching, Some(0));
    }

    #[test]
    fn a_first_start_asks_and_a_later_one_does_not() {
        if parterre_telemetry::do_not_track() || !parterre_telemetry::has_usage_statistics() {
            return;
        }
        let first = Telemetry::new(Privacy::default(), false);
        assert_eq!(first.prompt, Some(Choices::default()));
        assert_eq!(first.launching, None);
        let mut answered = Privacy::default();
        answered.answer(Choices::default());
        let later = Telemetry::new(answered, false);
        assert_eq!(later.prompt, None);
        assert_eq!(later.launching, Some(0));
    }

    #[test]
    fn scripted_runs_do_not_ask_and_show_an_example_id() {
        let t = Telemetry::new(Privacy::default(), true);
        assert_eq!(t.prompt, None);
        assert!(!t.sends());
        if parterre_telemetry::has_usage_statistics() {
            let answer = t.privacy.answer.as_ref().unwrap();
            assert_eq!(answer.install_id, EXAMPLE_ID);
            assert_eq!(answer.choices(), Choices::default());
        }
    }

    #[test]
    fn a_launch_remembers_its_version_and_sends_nothing_from_tests() {
        let mut t = Telemetry::new(Privacy::default(), true);
        t.privacy.last_version = Some("0.0.1".into());
        let ctx = egui::Context::default();
        for _ in 0..=SCREEN_WAIT {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| t.update(ui.ctx()));
            output.textures_delta.clear();
        }
        assert_eq!(t.launching, None);
        let version = parterre_telemetry::app_version(crate::VERSION);
        assert_eq!(t.privacy.last_version, Some(version));
        // Scripted, and a debug build: the launch's events are dropped, nothing started.
        assert!(t.pending.is_empty());
        assert!(t.usage.is_none());
    }

    #[test]
    fn user_input_is_noted_until_asked_and_the_windows_own_events_are_not() {
        let ctx = egui::Context::default();
        ctx.add_plugin(UserInput::default());
        let pass = |events: Vec<Event>| {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |_| {});
            output.textures_delta.clear();
        };
        let asked = || ctx.with_plugin(|seen: &mut UserInput| std::mem::take(&mut seen.0));
        pass(vec![Event::WindowFocused(true), Event::PointerGone]);
        assert_eq!(asked(), Some(false));
        // Two passes, as when another window and then the main one ran, before it is asked.
        pass(vec![Event::PointerMoved(Pos2::new(5.0, 5.0))]);
        pass(Vec::new());
        assert_eq!(asked(), Some(true));
        assert_eq!(asked(), Some(false));
        pass(vec![Event::Text("a".into())]);
        assert_eq!(asked(), Some(true));
    }

    #[test]
    fn worktrees_of_a_repository_are_the_same_repository() {
        let head = parterre_core::Head::Branch {
            name: "refs/heads/topic".into(),
            target: None,
        };
        let mut repo = Repo::new("/src/a-topic".into(), Vec::new(), Vec::new(), head);
        // Where git lists none.
        assert_eq!(repository_of(&repo), Path::new("/src/a-topic"));
        // The main worktree, listed first.
        for path in ["/src/a", "/src/a-topic"] {
            repo.worktrees.push(parterre_core::Worktree {
                path: path.into(),
                head: None,
                branch: None,
                locked: false,
                missing: false,
                open: path == "/src/a-topic",
            });
        }
        assert_eq!(repository_of(&repo), Path::new("/src/a"));
    }
}
