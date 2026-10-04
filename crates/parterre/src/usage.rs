//! Feature events (#264): the app's side. Which windows, dialogs, menus and actions are used
//! goes to [`parterre_telemetry::record`], which sends it only while usage statistics are sent;
//! the settings every event carries are [`properties`].

use eframe::egui::{self, Id, ViewportId};
use parterre_core::layout::Direction;
use parterre_core::log_layout::LogLayout;
use parterre_core::physics::DragModel;
use parterre_core::revgraph::Simplification;
use parterre_telemetry as telemetry;
pub use parterre_telemetry::{Action, Feature, Menu, Screen};

use crate::settings::{DiffForm, Settings};
use crate::theme::ThemeChoice;

/// The user started `action`.
pub fn action(action: Action) {
    telemetry::record(Feature::Action(action));
}

/// `screen` is shown, as `id`: recorded as it opens, the first frame it is shown after a frame
/// without. Call every frame it is shown.
pub fn screen(ctx: &egui::Context, id: Id, screen: Screen) {
    shown(ctx, id, Feature::Screen(screen));
}

/// `menu` is open, in the window `ctx` is drawing: recorded as it opens. Call every frame it is
/// shown.
pub fn menu(ctx: &egui::Context, menu: Menu) {
    let id = Id::new(("menu", menu.name())).with(ctx.viewport_id());
    shown(ctx, id, Feature::Menu(menu));
}

/// Records `feature` the first frame `id` is shown after a frame without it.
fn shown(ctx: &egui::Context, id: Id, feature: Feature) {
    let frame = ctx.cumulative_frame_nr_for(ViewportId::ROOT);
    let key = id.with("usage statistics");
    let last = ctx.data_mut(|d| {
        let last = d.get_temp::<u64>(key);
        d.insert_temp(key, frame);
        last
    });
    if !last.is_some_and(|last| last + 1 >= frame) {
        telemetry::record(feature);
    }
}

/// What every event says of the settings, the screen (its scale, `density`) and the
/// repositories: how many are in the recent list, and the open one's commits and nodes.
pub fn properties(
    settings: &Settings,
    density: Option<f32>,
    repositories: usize,
    commits: Option<usize>,
    nodes: Option<usize>,
) -> telemetry::Properties {
    telemetry::Properties {
        theme: match settings.theme {
            ThemeChoice::System => telemetry::Theme::System,
            ThemeChoice::Light => telemetry::Theme::Light,
            ThemeChoice::Dark => telemetry::Theme::Dark,
        },
        text_size: text_size(settings.text_size),
        graph_mode: match settings.graph.simplification {
            Simplification::Decorated => telemetry::GraphMode::LabelledCommits,
            Simplification::Forks => telemetry::GraphMode::LabelledForks,
            Simplification::BranchesAndMerges => telemetry::GraphMode::BranchingsAndMerges,
            Simplification::AllCommits => telemetry::GraphMode::AllCommits,
        },
        graph_direction: match settings.layout.direction {
            Direction::NewestTop => telemetry::GraphDirection::NewestTop,
            Direction::NewestBottom => telemetry::GraphDirection::NewestBottom,
            Direction::NewestLeft => telemetry::GraphDirection::NewestLeft,
            Direction::NewestRight => telemetry::GraphDirection::NewestRight,
        },
        log_layout: match settings.log_window.layout {
            LogLayout::Stacked => telemetry::LogLayout::Stacked,
            LogLayout::SideBySide => telemetry::LogLayout::SideBySide,
            LogLayout::DetailsBelow => telemetry::LogLayout::DetailsBelow,
            LogLayout::FilesRight => telemetry::LogLayout::FilesRight,
        },
        drag_model: match settings.net.model {
            DragModel::Adapt => telemetry::DragModel::Adapt,
            DragModel::Free => telemetry::DragModel::Free,
            DragModel::Subtree => telemetry::DragModel::Subtree,
        },
        diff_form: match settings.diff_window.form {
            DiffForm::SideBySide => telemetry::DiffForm::SideBySide,
            DiffForm::Unified => telemetry::DiffForm::Unified,
        },
        auto_reload: settings.auto_reload,
        pull_requests: settings.graph.show_pull_requests,
        screen_density: density,
        repositories,
        commits,
        nodes,
    }
}

/// The text size step nearest `size`: always one of the steps offered.
fn text_size(size: f32) -> f32 {
    let distance = |step: &f32| (step - size).abs();
    parterre_core::text_size::STEPS
        .into_iter()
        .min_by(|a, b| distance(a).total_cmp(&distance(b)))
        .unwrap_or(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parterre_telemetry::{Value, recording};

    fn frame(ctx: &egui::Context, mut show: impl FnMut(&egui::Context)) {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| show(ui.ctx()));
        output.textures_delta.clear();
    }

    #[test]
    fn a_window_counts_once_per_opening() {
        let ctx = egui::Context::default();
        let id = Id::new("about");
        let ((), recorded) = recording(|| {
            for _ in 0..3 {
                frame(&ctx, |ctx| screen(ctx, id, Screen::About));
            }
            // Closed for a frame, then opened again.
            frame(&ctx, |_| {});
            frame(&ctx, |ctx| screen(ctx, id, Screen::About));
            frame(&ctx, |ctx| screen(ctx, id, Screen::About));
        });
        assert_eq!(recorded, [Feature::Screen(Screen::About); 2]);
    }

    #[test]
    fn two_windows_of_a_kind_count_twice_and_a_menu_once() {
        let ctx = egui::Context::default();
        let ((), recorded) = recording(|| {
            for _ in 0..2 {
                frame(&ctx, |ctx| {
                    screen(ctx, Id::new(("diff", 1)), Screen::Diff);
                    screen(ctx, Id::new(("diff", 2)), Screen::Diff);
                    menu(ctx, Menu::Node);
                });
            }
        });
        let diff = Feature::Screen(Screen::Diff);
        assert_eq!(recorded, [diff, diff, Feature::Menu(Menu::Node)]);
    }

    #[test]
    fn the_properties_are_the_settings_names_and_never_free_text() {
        let mut settings = Settings {
            theme: ThemeChoice::Dark,
            text_size: 1.2501,
            ..Settings::default()
        };
        settings.log_window.layout = LogLayout::FilesRight;
        // Free text in the settings never reaches a property.
        settings.graph.ref_filter = "secret-branch".into();
        settings.graph.hide_branches = "feature/*".into();
        let properties = properties(&settings, Some(2.0), 4, Some(1234), Some(56));
        let pairs = properties.pairs();
        let value = |key| pairs.iter().find(|(k, _)| *k == key).unwrap().1;
        assert_eq!(value("theme"), Value::Name("dark"));
        assert_eq!(value("text_size"), Value::Number(1.25));
        assert_eq!(value("log_layout"), Value::Name("files_right"));
        assert_eq!(value("graph_mode"), Value::Name("labelled_commits"));
        assert_eq!(value("commit_count_range"), Value::Name("1000-9999"));
        assert_eq!(value("node_count_range"), Value::Name("10-99"));
        for (key, value) in pairs {
            if let Value::Name(name) = value {
                assert!(
                    name.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-+".contains(c)),
                    "{key}: {name}"
                );
                assert!(!name.contains("secret") && !name.contains("feature"));
            }
        }
    }

    #[test]
    fn every_text_size_step_is_sent_as_itself() {
        for step in parterre_core::text_size::STEPS {
            assert_eq!(text_size(step), step);
        }
        assert_eq!(text_size(9.0), parterre_core::text_size::MAX);
    }
}
