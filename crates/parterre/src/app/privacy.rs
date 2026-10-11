//! Nothing is sent before the first-run prompt is answered, the update check included.
//! Scripted runs never send, and show the prompt only when a script opens it.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Align, Layout, RichText, Ui, vec2};
use parterre_core::Repo;
use parterre_telemetry::{Build, Choices, Context, Lifecycle, Properties, Usage};

use super::ParterreApp;
use crate::settings::{Privacy, PrivacyAnswer};
use crate::widgets;

pub(super) const USAGE_LABEL: &str = "Usage statistics";
pub(super) const USAGE_TEXT: &str = "Installs, launches and which features are used, with a \
    random ID for this installation. Never personal information.";
pub(super) const CRASHES_LABEL: &str = "Crash reports";
pub(super) const CRASHES_TEXT: &str = "What parterre was doing when it crashed. May contain \
    personal information, such as a file or branch name.";
const SENT_WHERE: &str = "Sent to PostHog in the EU. Settings › Privacy changes either later.";

const SCRIPTED_INSTALL_ID: &str = "6f1c2b7e-3a4d-4e8f-9b0a-1c2d3e4f5a6b";

const SCREEN_SIZE_WAIT_FRAMES: u32 = 30;

#[derive(Debug)]
pub(super) struct Telemetry {
    pub privacy: Privacy,
    pub prompt: Option<Choices>,
    // Crash reports start once: changing the setting takes effect at the next start.
    pub crash_reports_at_start: bool,
    pub do_not_track: bool,
    scripted: bool,
    usage: Option<Usage>,
    pending_launch_events: Vec<Lifecycle>,
    /// Frames waited so far for the screen size, to tell the launch with; `None` once told, or
    /// while the prompt is up.
    launching: Option<u32>,
    first_run: bool,
    screen_size: Option<[f32; 2]>,
    last_repository: Option<PathBuf>,
    registered_properties: Option<Properties>,
}

impl Telemetry {
    pub fn new(mut privacy: Privacy, scripted: bool) -> Telemetry {
        let do_not_track = parterre_telemetry::do_not_track();
        // Scripted runs read no stored settings: answered with the defaults.
        if scripted && privacy.answer.is_none() && parterre_telemetry::has_usage_statistics() {
            let choices = Choices::default();
            privacy.answer = Some(PrivacyAnswer {
                install_id: SCRIPTED_INSTALL_ID.to_owned(),
                usage_statistics: choices.usage_statistics,
                crash_reports: choices.crash_reports,
            });
        }
        let answered = privacy.answer.is_some();
        let asks = parterre_telemetry::asks_first_run_prompt(Build::THIS, answered, do_not_track)
            && !scripted;
        let telemetry = Telemetry {
            crash_reports_at_start: privacy.choices().is_some_and(|c| c.crash_reports),
            privacy,
            prompt: asks.then(Choices::default),
            do_not_track,
            scripted,
            usage: None,
            pending_launch_events: Vec::new(),
            launching: (!asks).then_some(0),
            first_run: false,
            screen_size: None,
            last_repository: None,
            registered_properties: None,
        };
        telemetry.start_crash_reports();
        telemetry
    }

    fn start_crash_reports(&self) {
        if !self.scripted && !cfg!(test) {
            parterre_telemetry::start_crash_reports(
                self.privacy.choices(),
                crate::VERSION,
                parterre_core::git::version,
            );
        }
    }

    pub fn opened(&mut self, repo: &Repo) {
        let repository = repository_of(repo);
        if let Some(usage) = &mut self.usage {
            usage.opened(repository);
        }
        self.last_repository = Some(repository.to_owned());
    }

    fn sends(&self) -> bool {
        let choices = self.privacy.choices();
        !self.scripted
            && parterre_telemetry::sends_usage_statistics(Build::THIS, choices, self.do_not_track)
    }

    fn update(&mut self, ctx: &egui::Context, properties: Properties) {
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
            if screen.is_none() && waited < SCREEN_SIZE_WAIT_FRAMES {
                self.launching = Some(waited + 1);
                ctx.request_repaint();
                return;
            }
            self.launching = None;
            self.screen_size = screen;
            let last = self.privacy.last_version.as_deref();
            self.pending_launch_events =
                parterre_telemetry::launch(self.first_run, last, crate::VERSION);
            self.privacy.last_version = Some(parterre_telemetry::app_version(crate::VERSION));
        }
        if !self.sends() {
            self.usage = None;
            self.pending_launch_events.clear();
            return;
        }
        if self.registered_properties != Some(properties) {
            self.registered_properties = Some(properties);
            parterre_telemetry::register(properties);
        }
        if self.usage.is_none()
            && let Some(answer) = &self.privacy.answer
        {
            let context = Context {
                install_id: answer.install_id.clone(),
                version: crate::VERSION.to_owned(),
                screen: self.screen_size,
                git_version: parterre_core::git::version,
            };
            let events = std::mem::take(&mut self.pending_launch_events);
            self.usage = Usage::start(Some(answer.choices()), context, events);
            if let (Some(usage), Some(repository)) = (&mut self.usage, &self.last_repository) {
                usage.opened(repository);
            }
        }
    }

    fn answer(&mut self, choices: Choices) {
        self.privacy.answer(choices);
        self.prompt = None;
        self.crash_reports_at_start = choices.crash_reports;
        self.start_crash_reports();
        self.first_run = true;
        self.launching = Some(0);
    }

    pub fn close(&mut self) {
        if let Some(usage) = self.usage.take() {
            usage.close();
        }
    }
}

/// Its main worktree, which git lists first, so that every worktree counts as one repository.
fn repository_of(repo: &Repo) -> &Path {
    repo.worktrees.first().map_or(&repo.path, |main| &main.path)
}

/// Whether there was user input since last taken. A plugin, as every window's input passes
/// through it.
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

fn is_user_input(event: &egui::Event) -> bool {
    !matches!(
        event,
        egui::Event::WindowFocused(_) | egui::Event::PointerGone | egui::Event::Screenshot { .. }
    )
}

impl ParterreApp {
    pub(super) fn usage_statistics(&mut self, ctx: &egui::Context) {
        let properties = crate::usage::properties(
            &self.settings,
            ctx.native_pixels_per_point(),
            self.recent.iter().count(),
            self.repo.as_ref().map(|r| r.commits.len()),
            self.scene.as_ref().map(|s| s.graph.nodes.len()),
        );
        self.telemetry.update(ctx, properties);
    }

    pub(super) fn open_first_run_prompt(&mut self) {
        self.telemetry.prompt = Some(Choices::default());
    }

    pub(super) fn first_run_prompt(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Saved at once: a crash before the next save would make a second install ID.
        if self.telemetry.prompt(ctx)
            && let Some(storage) = frame.storage_mut()
        {
            eframe::App::save(self, storage);
            storage.flush();
        }
    }
}

impl Telemetry {
    /// True once answered: only *Continue* closes it.
    fn prompt(&mut self, ctx: &egui::Context) -> bool {
        let Some(choices) = &mut self.prompt else {
            return false;
        };
        let shown = crate::dialogs::Dialog::new("first-run", "Usage statistics and crash reports")
            .width(430.0)
            .modal()
            .undismissable()
            .show(ctx, |ui| {
                choice(ui, &mut choices.usage_statistics, USAGE_LABEL, USAGE_TEXT);
                ui.add_space(10.0);
                choice(ui, &mut choices.crash_reports, CRASHES_LABEL, CRASHES_TEXT);
                ui.add_space(12.0);
                ui.label(RichText::new(SENT_WHERE).weak());
                ui.add_space(12.0);
                ui.separator();
                let row = vec2(ui.available_width(), 34.0);
                ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                    widgets::primary_button(ui, "Continue", 90.0).clicked()
                })
                .inner
            });
        // Closing its window, where the platform still offers to, does nothing.
        if !shown.inner {
            return false;
        }
        let choices = *choices;
        self.answer(choices);
        true
    }
}

fn choice(ui: &mut Ui, on: &mut bool, label: &str, text: &str) {
    ui.checkbox(on, RichText::new(label).strong());
    ui.horizontal(|ui| {
        ui.add_space(ui.spacing().icon_width + ui.spacing().icon_spacing);
        ui.add(egui::Label::new(RichText::new(text).weak()).wrap());
    });
}

#[cfg(test)]
mod tests {
    use eframe::egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, vec2};

    use super::*;
    use crate::app::tool_harness::{click_events, raw_input, run_frame, text_at};

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
            // The dialog in the main window, as in scripted runs.
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
                time: Some(self.time),
                ..raw_input(vec2(1200.0, 800.0), std::mem::take(&mut self.events))
            };
            let telemetry = &mut self.telemetry;
            self.texts = run_frame(&self.ctx, input, |ui| {
                telemetry.prompt(ui.ctx());
            });
        }

        fn shows(&self, text: &str) -> bool {
            self.texts.iter().any(|(t, _)| t == text)
        }

        fn press(&mut self, at: Pos2) {
            for events in click_events(at, PointerButton::Primary, Modifiers::NONE) {
                self.events.extend(events);
                self.frame();
            }
            self.frame();
        }

        fn click(&mut self, text: &str) {
            self.press(text_at(&self.texts, text));
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
            USAGE_LABEL,
            CRASHES_LABEL,
            SENT_WHERE,
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
        screen.click(USAGE_LABEL);
        screen.click(CRASHES_LABEL);
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
            assert_eq!(answer.install_id, SCRIPTED_INSTALL_ID);
            assert_eq!(answer.choices(), Choices::default());
        }
    }

    #[test]
    fn a_launch_remembers_its_version_and_sends_nothing_from_tests() {
        let mut t = Telemetry::new(Privacy::default(), true);
        t.privacy.last_version = Some("0.0.1".into());
        let ctx = egui::Context::default();
        for _ in 0..=SCREEN_SIZE_WAIT_FRAMES {
            let properties = crate::usage::properties(&Default::default(), None, 0, None, None);
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                t.update(ui.ctx(), properties)
            });
            output.textures_delta.clear();
        }
        assert_eq!(t.launching, None);
        let version = parterre_telemetry::app_version(crate::VERSION);
        assert_eq!(t.privacy.last_version, Some(version));
        // Scripted: the launch's events are dropped, nothing started.
        assert!(t.pending_launch_events.is_empty());
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
        // Two passes, as when another window ran before the main one.
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
        assert_eq!(repository_of(&repo), Path::new("/src/a-topic"));
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
