//! The menu bar's commands, carried out on the window in front (#339). The graph's own are
//! done here. Another window's (macOS only, where the menu bar is the system's) are handed to
//! it as the keys it already takes: ⌘F finds there, ⌘R reloads, ⌘+ sets its text size, and
//! Copy, Cut, Paste and Select All reach its text as they would from the keyboard.

use eframe::egui::{self, ViewportId};

use super::menu_bar::{self, Command, Front, Shown, State};
use super::{ParterreApp, SettingsPage};
use crate::keys::{self, Platform, Shortcut};
use crate::usage;

/// Where the events for a window's next pass wait (see [`take_injected`]).
fn injected_id(viewport: ViewportId) -> egui::Id {
    egui::Id::new(("menu-bar-injected", viewport))
}

/// Where a window notes whether a text field has its keyboard (see [`window_begin`]).
fn text_focus_id(viewport: ViewportId) -> egui::Id {
    egui::Id::new(("menu-bar-text-focus", viewport))
}

/// Gives `viewport` `events` in its next pass, as if typed there.
fn inject(ctx: &egui::Context, viewport: ViewportId, events: Vec<egui::Event>) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<egui::Event>>(injected_id(viewport))
            .extend(events)
    });
    ctx.request_repaint_of(viewport);
}

/// The events waiting for `viewport`.
pub fn take_injected(ctx: &egui::Context, viewport: ViewportId) -> Vec<egui::Event> {
    ctx.data_mut(|d| d.remove_temp::<Vec<egui::Event>>(injected_id(viewport)))
        .unwrap_or_default()
}

/// At the start of a window's pass (the log, a diff, a blame, compare, settings): takes what
/// the menu bar did there as input, and notes whether a text field has the keyboard, for its
/// Edit items.
pub fn window_begin(ui: &egui::Ui) {
    let ctx = ui.ctx();
    let viewport = ctx.viewport_id();
    let events = take_injected(ctx, viewport);
    if !events.is_empty() {
        ui.input_mut(|i| i.events.extend(events));
    }
    let focus = ctx.egui_wants_keyboard_input();
    ctx.data_mut(|d| d.insert_temp(text_focus_id(viewport), focus));
}

/// What [`Shortcut`] stands for as an event, for a window to act on.
fn key_event(shortcut: Shortcut) -> egui::Event {
    egui::Event::Key {
        key: shortcut.key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: shortcut.modifiers,
    }
}

impl ParterreApp {
    /// The window in front: the main one, or (macOS) another of parterre's.
    pub(super) fn front(&self, ctx: &egui::Context) -> (Front, ViewportId) {
        if Platform::CURRENT != Platform::Mac {
            return (Front::Graph, ViewportId::ROOT);
        }
        let focused = ctx.input(|i| {
            i.raw
                .viewports
                .iter()
                .find(|(_, v)| v.focused == Some(true))
                .map(|(id, _)| *id)
        });
        match focused {
            Some(id) if id != ViewportId::ROOT => (Front::Other, id),
            _ => (Front::Graph, ViewportId::ROOT),
        }
    }

    /// What the menus show now.
    pub(super) fn menu_state(&mut self, ctx: &egui::Context) -> State {
        let (front, viewport) = self.front(ctx);
        let text_focus = if viewport == ViewportId::ROOT {
            ctx.egui_wants_keyboard_input()
        } else {
            ctx.data(|d| d.get_temp(text_focus_id(viewport)))
                .unwrap_or(false)
        };
        let open = self.repo.as_ref().map(|r| r.path.clone());
        let recent = self
            .recent
            .iter()
            .filter(|p| {
                open.as_deref()
                    .is_none_or(|o| !parterre_core::recent::same_path(p, o))
            })
            .map(std::path::Path::to_path_buf)
            .collect();
        let (can_undo, can_redo, displaced) =
            self.scene.as_ref().map_or((false, false, false), |s| {
                (s.net.can_undo(), s.net.can_redo(), s.net.any_displaced())
            });
        let g = &self.settings.graph;
        let pull_requests = self
            .pull_requests
            .origin()
            .is_none()
            .then_some(super::toolbar::NO_PULL_REQUESTS_TIP);
        let shown = vec![
            (Shown::LocalBranches, g.show_local_branches, None),
            (Shown::RemoteBranches, g.show_remote_branches, None),
            (Shown::Tags, g.show_tags, None),
            (
                Shown::PullRequests,
                self.pull_requests_active(),
                pull_requests,
            ),
            (Shown::Worktrees, g.show_worktrees, None),
            (Shown::Stash, g.show_stash, None),
            (Shown::OtherRefs, g.show_other_refs, None),
        ];
        let git = self.git_menu();
        State {
            platform: Platform::CURRENT,
            front,
            text_focus,
            locked: crate::dialogs::ModalLock::locked(ctx),
            has_repo: self.repo.is_some(),
            recent,
            can_undo,
            can_redo,
            selected: self.selection.current().is_some(),
            auto_reload: self.settings.auto_reload,
            detail: g.simplification,
            shown,
            current_branch_only: g.current_branch_only,
            first_parent_only: g.first_parent_only,
            overview: self.settings.show_overview,
            status_bar: self.settings.show_status_bar,
            remember_moves: self.settings.remember_moves,
            displaced,
            drag: self.settings.net.model,
            direction: self.settings.layout.direction,
            newer: self.newer_release().map(|u| u.version),
            git,
        }
    }

    /// Carries out `command` on the window in front.
    pub(super) fn run_command(&mut self, ctx: &egui::Context, command: Command) {
        let (front, viewport) = self.front(ctx);
        if front == Front::Other && self.run_in_window(ctx, viewport, &command) {
            return;
        }
        let text = ctx.egui_wants_keyboard_input();
        match command {
            Command::About => self.about.open(ctx),
            Command::Download => {
                if let Some(update) = self.newer_release() {
                    self.download(ctx, &update.download);
                }
            }
            Command::Settings => self.open_settings(self.settings_page),
            Command::InstallCommandLineTool => self.install_command_line_tool(),
            // As closing the window: the settings are saved.
            Command::Quit => {
                ctx.send_viewport_cmd_to(ViewportId::ROOT, egui::ViewportCommand::Close)
            }
            Command::OpenFolder => self.pick_folder = true,
            Command::OpenRecent(path) => self.open_folder(&path),
            Command::ClearRecent => {
                self.recent.clear();
                // The open one comes back, so that it is listed once another is opened.
                if let Some(open) = self.repo.as_ref().map(|r| r.path.clone()) {
                    self.recent.add(&open);
                }
            }
            Command::Close => self.close_folder(),
            Command::Export(format) => self.export = Some(format),
            Command::Undo if text => inject(ctx, ViewportId::ROOT, vec![key_event(keys::UNDO)]),
            Command::Redo if text => inject(ctx, ViewportId::ROOT, vec![key_event(keys::redo())]),
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            // In the find field, the field's; else the selected commit's hash.
            Command::Copy if text => inject(ctx, ViewportId::ROOT, vec![egui::Event::Copy]),
            Command::Copy => self.copy_selected_hash(ctx),
            Command::Cut => inject(ctx, ViewportId::ROOT, vec![egui::Event::Cut]),
            Command::Paste => {
                if let Some(text) = clipboard_text() {
                    inject(ctx, ViewportId::ROOT, vec![egui::Event::Paste(text)]);
                }
            }
            Command::SelectAll => inject(ctx, ViewportId::ROOT, vec![key_event(keys::SELECT_ALL)]),
            Command::Find => self.search.request_focus = true,
            Command::FindNext => self.goto_search_hit(true),
            Command::FindPrevious => self.goto_search_hit(false),
            Command::Reload => self.reload_by_hand(ctx),
            Command::AutoReload => self.settings.auto_reload = !self.settings.auto_reload,
            Command::ZoomIn => self.zoom_by(1.0 / 0.8),
            Command::ZoomOut => self.zoom_by(0.8),
            Command::ActualSize => self.zoom_by(1.0 / self.view.zoom),
            Command::ZoomToFit => {
                usage::action(usage::Action::Fit);
                self.fit();
            }
            Command::GoToHead => {
                usage::action(usage::Action::GoToHead);
                self.go_to_head();
            }
            Command::Detail(s) => self.settings.graph.simplification = s,
            Command::Show(what) => self.toggle_shown(what),
            Command::CurrentBranchOnly => {
                let g = &mut self.settings.graph;
                g.current_branch_only = !g.current_branch_only;
            }
            Command::FirstParentOnly => {
                let g = &mut self.settings.graph;
                g.first_parent_only = !g.first_parent_only;
            }
            Command::FilterSettings => self.open_settings(SettingsPage::Filters),
            Command::Overview => self.settings.show_overview = !self.settings.show_overview,
            Command::StatusBar => self.settings.show_status_bar = !self.settings.show_status_bar,
            Command::ShowLog => {
                let nodes = self.selection.nodes.clone();
                self.show_log(&nodes);
            }
            Command::Compare(request) => self.compare_request(request),
            Command::CopyText(text) => {
                usage::action(usage::Action::Copy);
                ctx.copy_text(text);
            }
            Command::OpenPullRequest(url) => self.open_pull_request(&url),
            Command::OpenIn(opener, dir) => self.open_in(opener, &dir),
            Command::SelectSubtree(roots) => {
                usage::action(usage::Action::SelectSubtree);
                self.select_subtree(&roots);
            }
            Command::ReturnToLayout(nodes) => {
                usage::action(usage::Action::ReturnToLayout);
                self.return_to_layout(&nodes);
            }
            Command::CentreOn(node) => self.center_on(node),
            Command::Git(request) => self.branches.request(ctx, request, ViewportId::ROOT),
            Command::Fetch => self.fetch(ctx, ViewportId::ROOT),
            Command::RememberMoves => self.set_remember_moves(!self.settings.remember_moves),
            Command::ReturnAllToLayout => self.reset_positions(),
            Command::Drag(model) => self.set_drag_model(model),
            Command::Direction(d) => self.settings.layout.direction = d,
            Command::KeyboardAndMouse => self.show_shortcuts = true,
            Command::Legend => self.show_legend = true,
        }
    }

    /// `command` for another window in front, `viewport`, if it is that window's: handed to it
    /// as its keys. False for what isn't, which the app does as from the main window.
    fn run_in_window(
        &mut self,
        ctx: &egui::Context,
        viewport: ViewportId,
        command: &Command,
    ) -> bool {
        let events = match command {
            Command::Close => {
                ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Close);
                return true;
            }
            Command::Fetch => {
                self.fetch(ctx, viewport);
                return true;
            }
            Command::Copy => vec![egui::Event::Copy],
            Command::Cut => vec![egui::Event::Cut],
            Command::Paste => match clipboard_text() {
                Some(text) => vec![egui::Event::Paste(text)],
                None => return true,
            },
            Command::SelectAll => vec![key_event(keys::SELECT_ALL)],
            Command::Find => vec![key_event(keys::FIND)],
            Command::FindNext => vec![key_event(keys::find_next())],
            Command::FindPrevious => vec![key_event(keys::find_previous())],
            Command::Reload => vec![key_event(keys::RELOAD)],
            Command::ZoomIn => vec![key_event(keys::ZOOM_IN)],
            Command::ZoomOut => vec![key_event(keys::ZOOM_OUT)],
            Command::ActualSize => vec![key_event(keys::ACTUAL_SIZE)],
            _ => return false,
        };
        inject(ctx, viewport, events);
        true
    }

    fn toggle_shown(&mut self, what: Shown) {
        let g = &mut self.settings.graph;
        match what {
            Shown::LocalBranches => g.show_local_branches = !g.show_local_branches,
            Shown::RemoteBranches => g.show_remote_branches = !g.show_remote_branches,
            Shown::Tags => g.show_tags = !g.show_tags,
            Shown::Worktrees => g.show_worktrees = !g.show_worktrees,
            Shown::Stash => g.show_stash = !g.show_stash,
            Shown::OtherRefs => g.show_other_refs = !g.show_other_refs,
            Shown::PullRequests => self.toggle_pull_requests(),
        }
    }

    /// *Install Command Line Tool…* (macOS, #341): the answer comes in a later frame.
    fn install_command_line_tool(&mut self) {
        #[cfg(target_os = "macos")]
        if self.cli_install.is_none() {
            self.cli_install = Some(crate::macos::cli::install());
        }
    }

    /// The keyboard in the menu bar of Windows and Linux (Alt+letter, F10, arrows), before the
    /// window's own keys: what it takes, they don't see. macOS's menu bar is the system's.
    pub(super) fn menu_bar_keys(&mut self, ctx: &egui::Context) {
        if Platform::CURRENT == Platform::Mac {
            return;
        }
        let state = self.menu_state(ctx);
        let menus = menu_bar::build(&state);
        if let Some(command) = menu_bar::bar::keys(ctx, &menus) {
            self.run_command(ctx, command);
        }
    }

    /// The menu bar of Windows and Linux, above the toolbar; on macOS, the system's, brought up
    /// to date. Carries out what was chosen in it.
    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let state = self.menu_state(&ctx);
        let menus = menu_bar::build(&state);
        #[cfg(target_os = "macos")]
        {
            let badge = state.newer.is_some();
            let chosen = self
                .native_menu
                .update(&ctx, menus, state.text_focus, badge);
            for command in chosen {
                self.run_command(&ctx, command);
            }
            if let Some(rx) = &self.cli_install
                && let Ok(result) = rx.try_recv()
            {
                self.cli_install = None;
                self.status = Some(match result {
                    Ok(message) => (message, false),
                    Err(e) => (e, true),
                });
            }
            if self.cli_install.is_some() {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let mut chosen = None;
            egui::Panel::top("menu-bar")
                .frame(
                    egui::Frame::side_top_panel(&ctx.global_style())
                        .inner_margin(egui::Margin::symmetric(4, 0)),
                )
                .show_separator_line(false)
                .show(ui, |ui| chosen = menu_bar::bar::show(ui, &menus));
            if let Some(command) = chosen {
                self.run_command(&ctx, command);
            }
        }
    }
}

/// The text on the clipboard, for Paste from the menu bar (macOS; elsewhere the keyboard's
/// paste reaches egui by itself).
fn clipboard_text() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        crate::macos::clipboard_text()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}
