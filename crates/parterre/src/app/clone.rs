//! A tab for the URL, then one per forge this build can ask, listing the user's repositories
//! there to pick from, or why it can't. The parent folder and folder name under the tabs are
//! for all of them.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, mpsc};

use eframe::egui::{self, RichText, Ui, ViewportId};
use parterre_core::branches::{Action, command_text};
use parterre_core::clone::{self, Cloning};
use parterre_forge::{Forge, ForgeError, Listed, Repositories, azure, github};

use crate::file_dialog::{self, Pending};
use crate::{dialogs, focus, widgets, worker};

const MIN_LIST_ROWS: usize = 4;
const INITIAL_LIST_ROWS: usize = 6;
const LIST_ROW_HEIGHT: f32 = 24.0;
const LIST_MARGIN: i8 = 4;
const LIST_LINE_WIDTH: f32 = 1.0;
/// Margins and line, above and below.
const LIST_FRAME_HEIGHT: f32 = 2.0 * (LIST_MARGIN as f32 + LIST_LINE_WIDTH);
const ROW_TEXT_PAD: f32 = 8.0;
const ROW_MARKS_GAP: f32 = 10.0;

/// The URL's tab, then one for each forge this build can ask.
static TABS: LazyLock<Vec<(Tab, &'static str)>> = LazyLock::new(|| {
    let forges = [Forge::Github, Forge::AzureDevops]
        .into_iter()
        .filter(|forge| forge.supported())
        .map(Tab::Forge);
    std::iter::once(Tab::Url)
        .chain(forges)
        .map(|tab| (tab, tab.label()))
        .collect()
});

#[derive(Debug)]
enum Listing {
    Loading(mpsc::Receiver<Result<Repositories, ForgeError>>),
    Loaded(Repositories),
    Failed(ForgeError),
}

impl Listing {
    fn receive(&mut self, forge: Forge) {
        let Listing::Loading(rx) = self else {
            return;
        };
        let stopped = || Err(ForgeError::Network(forge, "the request stopped".into()));
        *self = match worker::received(rx, stopped) {
            Some(Ok(repos)) => Listing::Loaded(repos),
            Some(Err(e)) => Listing::Failed(e),
            None => return,
        };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Url,
    Forge(Forge),
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Url => "URL",
            Tab::Forge(forge) => forge.name(),
        }
    }
}

/// A repository chosen in a forge's list, by the URL it clones from.
struct Chosen {
    url: String,
    /// Double-clicked, or Enter pressed on it: cloned at once, if it can be.
    clone_now: bool,
}

#[derive(Debug, Default)]
struct ListScroll {
    /// To the repository picked, which may have been found by words.
    to_picked: bool,
    last_offset: f32,
}

#[derive(Debug)]
pub struct CloneDialog {
    url: String,
    parent: String,
    folder_name: String,
    folder_name_edited: bool,
    browse: Option<Pending<()>>,
    github: Listing,
    azure: Listing,
    tab: Tab,
    /// Shared by the forges' tabs.
    query: String,
    focus_field: bool,
    list_scroll: ListScroll,
    clipboard_url: Option<mpsc::Receiver<Option<String>>>,
    pub opener: ViewportId,
    fresh: bool,
    /// The folder a clone would go in, there already, picked to open instead.
    pub open_instead: Option<PathBuf>,
}

impl CloneDialog {
    /// Starts asking GitHub and Azure DevOps for the lists, or reads `canned`
    /// (`--github-repositories-from`) and asks neither. `parent` is the parent folder used last;
    /// `known` the repositories opened before, whose remotes name Azure DevOps organisations.
    pub fn new(
        ctx: &egui::Context,
        parent: Option<&Path>,
        canned: Option<Arc<str>>,
        known: Vec<PathBuf>,
        read_clipboard: bool,
        opener: ViewportId,
    ) -> Self {
        let (github, azure) = match canned {
            Some(json) => (
                match github::repositories_canned(&json) {
                    Ok(repos) => Listing::Loaded(repos),
                    Err(e) => Listing::Failed(e),
                },
                Listing::Loaded(Repositories::default()),
            ),
            None => (
                Listing::Loading(worker::spawn(ctx, github::repositories)),
                Listing::Loading(worker::spawn(ctx, move || azure::repositories(&known))),
            ),
        };
        let parent = parent
            .filter(|p| p.is_dir())
            .map(Path::to_path_buf)
            .or_else(std::env::home_dir)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        CloneDialog {
            url: String::new(),
            parent,
            folder_name: String::new(),
            folder_name_edited: false,
            browse: None,
            github,
            azure,
            tab: Tab::Url,
            query: String::new(),
            focus_field: true,
            list_scroll: ListScroll::default(),
            clipboard_url: read_clipboard
                .then(|| worker::spawn(ctx, || clipboard_text().filter(|t| is_remote_url(t)))),
            opener,
            fresh: true,
            open_instead: None,
        }
    }

    /// Brings it forward, when asked for again while it's open.
    pub fn raise(&mut self) {
        self.fresh = true;
    }

    pub fn cloning(&self) -> Cloning {
        Cloning {
            url: self.url.trim().to_owned(),
            parent: PathBuf::from(self.parent.trim()),
            name: self.folder_name.trim().to_owned(),
        }
    }

    pub fn action(&self) -> Action {
        Action::Clone(Box::new(self.cloning()))
    }

    fn set_url(&mut self, url: String) {
        self.url = url;
        self.follow_url();
    }

    /// The folder name follows the URL unless typed.
    fn follow_url(&mut self) {
        if !self.folder_name_edited {
            self.folder_name = if clone::is_url(&self.url) {
                clone::folder_name(&self.url)
            } else {
                String::new()
            };
        }
    }

    /// Takes in what the workers found, and a folder picked with *Browse…*.
    fn receive(&mut self) {
        self.github.receive(Forge::Github);
        self.azure.receive(Forge::AzureDevops);
        let clipboard_url = self
            .clipboard_url
            .as_ref()
            .and_then(|rx| worker::received(rx, || None));
        if let Some(url) = clipboard_url {
            self.clipboard_url = None;
            if let Some(url) = url
                && self.url.is_empty()
            {
                self.set_url(url.trim().to_owned());
            }
        }
        if let Some(answer) = self.browse.as_ref().and_then(Pending::answer) {
            self.browse = None;
            if let Some(dir) = answer {
                self.parent = dir.to_string_lossy().into_owned();
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, busy: bool) -> dialogs::Answer {
        self.receive();
        let fresh = std::mem::take(&mut self.fresh);
        self.focus_field |= fresh;
        let shown = dialogs::Dialog::new("clone", "Clone a repository")
            .screen(crate::usage::Screen::Clone)
            .width(520.0)
            .opener(self.opener)
            .raise(fresh)
            .resizable()
            .show(ctx, |ui| {
                if let Some(chosen) = self.tabbed_page(ui) {
                    self.set_url(chosen.url);
                    if chosen.clone_now && self.cloning().error().is_none() && !busy {
                        return dialogs::Answer::Primary;
                    }
                }
                ui.add_space(8.0);
                self.folder_fields(ui);
                self.command_and_actions(ui, busy)
            });
        let mut answer = shown.inner;
        if shown.should_close() && answer == dialogs::Answer::Open {
            answer = dialogs::Answer::Cancel;
        }
        answer
    }

    /// The selected tab's page under the tabs; the URL's alone, with no tab, in a build that can
    /// ask no forge.
    fn tabbed_page(&mut self, ui: &mut Ui) -> Option<Chosen> {
        if TABS.len() == 1 {
            return self.page(ui);
        }
        // Left and Right on the tabs leave them the keyboard; a click doesn't.
        let on_tabs = ui.memory(|m| m.had_focus_last_frame(tabs_id()))
            && !ui.input(|i| i.pointer.any_click());
        let mut tab = self.tab;
        let chosen = widgets::tabs(ui, tabs_id(), &mut tab, &TABS, |ui| self.page(ui));
        if tab != self.tab {
            self.tab = tab;
            self.focus_field |= !on_tabs;
        }
        chosen
    }

    /// Gives the tab's field the keyboard, if it is to have it, until it has: a frame may be
    /// laid out only to be discarded, as a new window's first is.
    fn take_focus(&mut self, field: &egui::Response) {
        if self.focus_field {
            if field.has_focus() {
                self.focus_field = false;
            } else {
                field.request_focus();
            }
        }
    }

    /// The selected tab's field, and under it an area as tall in every tab, so that the dialog
    /// keeps its size.
    fn page(&mut self, ui: &mut Ui) -> Option<Chosen> {
        let width = ui.available_width();
        let field = match self.tab {
            Tab::Url => {
                let response = widgets::text_field(ui, &mut self.url, "https://…, git@…", width);
                if response.changed() {
                    self.follow_url();
                }
                response
            }
            Tab::Forge(forge) => {
                let hint = format!("Search your {forge} repositories");
                widgets::text_field(ui, &mut self.query, &hint, width)
            }
        };
        self.take_focus(&field);
        ui.add_space(4.0);
        let spacing = ui.spacing().item_spacing.y;
        let rows = |n: usize| n as f32 * (LIST_ROW_HEIGHT + spacing) - spacing + LIST_FRAME_HEIGHT;
        dialogs::growing(ui, rows(MIN_LIST_ROWS), |ui, height| {
            let height = height.unwrap_or(rows(INITIAL_LIST_ROWS));
            let chosen = match self.tab {
                Tab::Url => {
                    note(ui, height, |ui| {
                        ui.label(
                            "Any URL git clones from: GitHub, Azure DevOps or any other host, \
                             over HTTPS or SSH. git signs in with its credential helpers, as for \
                             a fetch.",
                        );
                    });
                    None
                }
                Tab::Forge(forge) => self.forge_area(ui, forge, height),
            };
            (chosen, rows(INITIAL_LIST_ROWS))
        })
    }

    /// `forge`'s repositories that the words looked for match, in a list `height` tall with its
    /// frame, or why there are none.
    fn forge_area(&mut self, ui: &mut Ui, forge: Forge, height: f32) -> Option<Chosen> {
        let listing = match forge {
            Forge::Github => &self.github,
            Forge::AzureDevops => &self.azure,
        };
        let repos = loaded_or_note(ui, forge, listing, height)?;
        let shown = repos.matching(&self.query);
        let picked = repos
            .find_url(self.url.trim())
            .and_then(|p| shown.iter().position(|l| l.name == p.name));
        let list_height = height - LIST_FRAME_HEIGHT;
        repository_list(
            ui,
            forge,
            &shown,
            picked,
            list_height,
            &mut self.list_scroll,
        )
    }

    fn folder_fields(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Parent folder").strong());
        let browsing = self.browse.is_some();
        let (_, browse) = widgets::folder_field(ui, &mut self.parent, "Folder", browsing);
        if browse {
            self.browse = Some(file_dialog::pick_folder(
                ui.ctx(),
                "Parent folder",
                self.parent.trim(),
            ));
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Folder name").strong());
        let name = &mut self.folder_name;
        if widgets::folder_name_field(ui, name, &mut self.folder_name_edited, "Follow the URL") {
            self.follow_url();
        }
    }

    /// Why the clone can't start, if it can't, the command it runs, and the buttons.
    fn command_and_actions(&mut self, ui: &mut Ui, busy: bool) -> dialogs::Answer {
        let cloning = self.cloning();
        let is_url = clone::is_url(&cloning.url);
        let error = is_url.then(|| cloning.error()).flatten();
        if let Some(error) = &error {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(ui.visuals().error_fg_color, error);
                if let Some(existing) = cloning.existing()
                    && widgets::text_button(ui, "Open")
                        .on_hover_text("Open this folder instead")
                        .clicked()
                {
                    self.open_instead = Some(existing);
                }
            });
        }
        let command = is_url.then(|| command_text(&clone::command(&cloning)));
        dialogs::command_box(ui, command.as_slice());
        let ready = is_url && error.is_none() && !busy;
        dialogs::actions(ui, dialogs::Primary::new("Clone").enabled(ready))
    }
}

fn tabs_id() -> egui::Id {
    egui::Id::new("clone-tabs")
}

fn list_stop_id(forge: Forge) -> egui::Id {
    egui::Id::new(("clone-list-stop", forge))
}

/// The repositories `listing` has, if it has any; else why not, in an area `height` tall.
fn loaded_or_note<'a>(
    ui: &mut Ui,
    forge: Forge,
    listing: &'a Listing,
    height: f32,
) -> Option<&'a Repositories> {
    match listing {
        Listing::Loaded(repos) if !repos.list.is_empty() => return Some(repos),
        Listing::Loading(_) => note(ui, height, |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Loading your {forge} repositories…"));
            });
        }),
        Listing::Failed(e) if e.needs_sign_in() && forge == Forge::Github => {
            note(ui, height, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label("Sign in with ");
                    ui.label(RichText::new("gh auth login").monospace());
                    ui.label(
                        " in a terminal, and your GitHub repositories are listed here the next \
                         time.",
                    );
                });
            });
        }
        Listing::Failed(e) => note(ui, height, |ui| {
            ui.label(format!("Couldn't list your {forge} repositories: {e}."));
        }),
        Listing::Loaded(_) => note(ui, height, |ui| match forge {
            Forge::Github => {
                ui.label("Your GitHub account has no repositories.");
            }
            Forge::AzureDevops => {
                ui.label(
                    "No Azure DevOps organisation that git has a sign-in for was found. Clone \
                     one of its repositories by its URL, signing in when git asks, and the \
                     organisation's repositories are listed here the next time.",
                );
            }
        }),
    }
    None
}

/// `shown` in a list `height` tall inside its frame, row `picked` selected. The list is one stop
/// for Tab, which the keys move the pick in.
fn repository_list(
    ui: &mut Ui,
    forge: Forge,
    shown: &[&Listed],
    picked: Option<usize>,
    height: f32,
    scroll: &mut ListScroll,
) -> Option<Chosen> {
    let spacing = ui.spacing().item_spacing.y;
    let pitch = LIST_ROW_HEIGHT + spacing;
    let stop_id = list_stop_id(forge);
    let rows_per_page = ((height + spacing) / pitch).floor() as usize;
    let keys = widgets::list_keys(ui, stop_id, picked, shown.len(), rows_per_page);
    let choose = |row: usize, clone_now| Chosen {
        url: shown[row].url.clone(),
        clone_now,
    };
    let mut chosen = match (keys.select, picked) {
        (Some(row), _) => Some(choose(row, false)),
        (None, Some(row)) if keys.enter => Some(choose(row, true)),
        _ => None,
    };
    let selected = keys.select.or(picked);
    let to_picked = std::mem::take(&mut scroll.to_picked)
        .then_some(picked)
        .flatten();
    let cue = focus::shows_own_by_id(ui.ctx(), stop_id).then(|| selected.unwrap_or(0));
    let mut cued = false;
    egui::Frame::new()
        .stroke(egui::Stroke::new(
            LIST_LINE_WIDTH,
            widgets::tones(ui).field_line,
        ))
        .corner_radius(7)
        .inner_margin(LIST_MARGIN)
        .show(ui, |ui| {
            if shown.is_empty() {
                note(ui, height, |ui| {
                    ui.label(format!("  None of your {forge} repositories match."));
                });
                return;
            }
            let top = |row: usize| row as f32 * pitch;
            let offset = match (to_picked, keys.select) {
                (Some(row), _) => Some(top(row) - (height - LIST_ROW_HEIGHT) / 2.0),
                // Moved to by the keyboard: scrolled only as far as brings it in view.
                (None, Some(row)) => Some(
                    scroll
                        .last_offset
                        .max(top(row) + LIST_ROW_HEIGHT - height)
                        .min(top(row)),
                ),
                (None, None) => None,
            };
            let mut area = egui::ScrollArea::vertical()
                .id_salt(("clone-list", forge))
                .max_height(height)
                .min_scrolled_height(height)
                .auto_shrink([false, false]);
            if let Some(offset) = offset {
                area = area.vertical_scroll_offset(offset.max(0.0));
            }
            let room = egui::Rect::from_min_size(
                ui.cursor().min,
                egui::vec2(ui.available_width(), height),
            );
            let stop = widgets::list_stop(ui, stop_id, room);
            let output = area.show_rows(ui, LIST_ROW_HEIGHT, shown.len(), |ui, rows| {
                for row in rows {
                    let listed = shown[row];
                    let response = list_row(ui, listed, selected == Some(row));
                    if cue == Some(row) {
                        widgets::paint_list_cue(ui, response.rect);
                        cued = true;
                    }
                    focus::take_on_row_click(&response, stop_id);
                    let response = if listed.description.is_empty() {
                        response
                    } else {
                        response.on_hover_text(&listed.description)
                    };
                    if response.clicked() || response.double_clicked() {
                        chosen = Some(choose(row, response.double_clicked()));
                        scroll.to_picked = true;
                    }
                }
            });
            scroll.last_offset = output.state.offset.y;
            widgets::list_stop_ring(ui, &stop, cued);
        });
    chosen
}

/// An area `height` tall with what `add` says, in weak text, at its top.
fn note(ui: &mut Ui, height: f32, add: impl FnOnce(&mut Ui)) {
    ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
        ui.set_min_size(egui::vec2(ui.available_width(), height));
        ui.visuals_mut().override_text_color = Some(ui.visuals().weak_text_color());
        ui.add_space(4.0);
        add(ui);
    });
}

/// A row of the list, as wide as it, highlighted when `selected` or hovered. Not a stop for Tab
/// of its own: the list is one.
fn list_row(ui: &mut Ui, listed: &Listed, selected: bool) -> egui::Response {
    let size = egui::vec2(ui.available_width(), LIST_ROW_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::CLICK);
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let visuals = ui.visuals();
    let fill = if selected {
        Some(visuals.selection.bg_fill)
    } else if response.hovered() {
        Some(visuals.widgets.hovered.weak_bg_fill)
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, 5, fill);
    }
    let mut job = row_text(ui, listed);
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 2.0 * ROW_TEXT_PAD);
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let at = egui::pos2(
        rect.left() + ROW_TEXT_PAD,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(at, galley, visuals.text_color());
    response
}

/// The repository's name, then whether it's private, a fork or archived.
fn row_text(ui: &Ui, listed: &Listed) -> egui::text::LayoutJob {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &listed.name,
        0.0,
        egui::TextFormat::simple(font.clone(), ui.visuals().text_color()),
    );
    let marks: Vec<&str> = [
        (listed.private, "private"),
        (listed.fork, "fork"),
        (listed.archived, "archived"),
    ]
    .into_iter()
    .filter_map(|(on, mark)| on.then_some(mark))
    .collect();
    if !marks.is_empty() {
        job.append(
            &marks.join(" · "),
            ROW_MARKS_GAP,
            egui::TextFormat::simple(font, ui.visuals().weak_text_color()),
        );
    }
    job
}

/// What's worth taking from the clipboard: a URL of a remote, not a local path.
fn is_remote_url(text: &str) -> bool {
    let text = text.trim();
    clone::is_url(text)
        && !text.contains('\n')
        && !text.starts_with(['/', '\\', '~', '.'])
        && !Path::new(text).is_absolute()
}

fn clipboard_text() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        crate::macos::clipboard_text()
    }
    #[cfg(not(target_os = "macos"))]
    {
        arboard::Clipboard::new().ok()?.get_text().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tool_harness::{Harness, git, init};

    const REPOS: &str = r#"[
        {"name": "mira/checkout", "private": true},
        {"name": "mira/parterre", "description": "Revision graph viewer"},
        {"name": "acme/payments"}
    ]"#;

    /// The harness with *Clone repository…* open over a repository of one commit, listing
    /// [`REPOS`], and the parent folder clones go in.
    fn open() -> (Harness, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path());
        git(
            dir.path(),
            &["commit", "-q", "--allow-empty", "-m", "first"],
        );
        let parent = tempfile::tempdir().unwrap();
        let mut h = Harness::new(dir);
        h.tool.canned_repositories = Some(Arc::from(REPOS));
        h.tool.clone_parent = Some(parent.path().to_owned());
        let ctx = h.ctx.clone();
        h.tool.clone_repository(&ctx, ViewportId::ROOT, Vec::new());
        h.until("the dialog opens", |h| h.shows("Clone a repository"));
        for _ in 0..5 {
            h.frame();
        }
        (h, parent)
    }

    fn dialog(h: &Harness) -> &CloneDialog {
        h.tool.clone.as_ref().expect("the dialog is open")
    }

    #[test]
    fn a_forges_tab_finds_by_words_and_a_pick_fills_the_url_and_folder() {
        let (mut h, _parent) = open();
        // The URL's tab first, then a forge's.
        assert!(h.shows("URL") && h.shows("GitHub") && h.shows("Azure DevOps"));
        assert!(!h.shows("acme/payments"), "the URL's tab lists nothing");
        h.click("GitHub");
        assert!(h.shows("mira/checkoutprivate") && h.shows("acme/payments"));
        h.type_text("graph");
        assert!(h.shows("mira/parterre"));
        assert!(!h.shows("acme/payments"), "{:?}", h.texts);
        // The field's hint shows under its label: nothing picked names no folder.
        let folder = |h: &Harness| h.texts.iter().filter(|(t, _)| t == "Folder name").count();
        assert_eq!(folder(&h), 2);
        h.click("mira/parterre");
        assert!(h.shows("parterre"), "the folder follows the URL");
        assert_eq!(folder(&h), 1);
        h.click("URL");
        assert!(h.shows("https://github.com/mira/parterre.git"));
    }

    /// Ctrl+Tab goes to the next tab, Ctrl+Shift+Tab to the previous, round from either end.
    #[test]
    fn ctrl_tab_goes_both_ways() {
        let (mut h, _parent) = open();
        let tab = |h: &Harness| dialog(h).tab;
        let (forth, back) = (
            egui::Modifiers::CTRL,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        );
        h.key_with(egui::Key::Tab, forth);
        assert_eq!(tab(&h), Tab::Forge(Forge::Github));
        h.key_with(egui::Key::Tab, back);
        assert_eq!(tab(&h), Tab::Url);
        h.key_with(egui::Key::Tab, back);
        assert_eq!(tab(&h), Tab::Forge(Forge::AzureDevops));
        h.key_with(egui::Key::Tab, forth);
        assert_eq!(tab(&h), Tab::Url);
    }

    fn url(h: &Harness) -> String {
        dialog(h).url.clone()
    }

    /// GitHub's repositories as its list shows them, with no words looked for.
    fn listed(h: &Harness) -> Vec<String> {
        let Listing::Loaded(repos) = &dialog(h).github else {
            panic!("not loaded");
        };
        repos.matching("").iter().map(|l| l.url.clone()).collect()
    }

    /// Tab goes through the dialog as it reads, each control once: the tabs are one stop,
    /// which Left and Right move within, and so is the list, with Up, Down, Home and End.
    #[test]
    fn tab_visits_each_control_once_in_reading_order() {
        let (mut h, _parent) = open();
        assert!(
            h.ctx.text_edit_focused(),
            "the URL's field has the keyboard"
        );
        h.key_with(egui::Key::Tab, egui::Modifiers::SHIFT);
        let tabs = tabs_id();
        assert_eq!(h.focused(), Some(tabs), "back from the field: the tabs");
        h.key(egui::Key::ArrowRight);
        assert_eq!(dialog(&h).tab, Tab::Forge(Forge::Github));
        assert_eq!(
            h.focused(),
            Some(tabs),
            "Right leaves the keyboard with the tabs"
        );
        h.key(egui::Key::ArrowLeft);
        h.key(egui::Key::ArrowLeft);
        assert_eq!(dialog(&h).tab, Tab::Url, "and stops at the first");
        h.key(egui::Key::ArrowRight);
        h.key(egui::Key::Tab);
        assert!(h.ctx.text_edit_focused(), "into the page: its search field");
        h.key(egui::Key::Tab);
        assert_eq!(
            h.focused(),
            Some(list_stop_id(Forge::Github)),
            "then the list"
        );
        let urls = listed(&h);
        h.key(egui::Key::ArrowDown);
        assert_eq!(url(&h), urls[0], "Down picks the first");
        h.key(egui::Key::ArrowDown);
        assert_eq!(url(&h), urls[1]);
        h.key(egui::Key::End);
        assert_eq!(url(&h), urls[urls.len() - 1]);
        h.key(egui::Key::Home);
        assert_eq!(url(&h), urls[0]);
        assert_eq!(
            h.focused(),
            Some(list_stop_id(Forge::Github)),
            "the list keeps the keyboard"
        );
        h.key(egui::Key::Tab);
        assert!(
            h.ctx.text_edit_focused(),
            "past the rows: the parent folder"
        );
        h.key(egui::Key::Tab);
        assert!(focus_on(&h, "Browse…"));
        h.key(egui::Key::Tab);
        assert!(h.ctx.text_edit_focused(), "the folder name");
        // Its button to follow the URL again is greyed: not a stop.
        h.key(egui::Key::Tab);
        assert!(focus_on(&h, "Git command"));
        h.key(egui::Key::Tab);
        assert!(focus_on(&h, "Cancel"), "Cancel, then the button after it");
        h.key(egui::Key::Tab);
        assert!(focus_on(&h, "Clone"));
    }

    /// Whether the widget with the keyboard is the one showing `text`.
    fn focus_on(h: &Harness, text: &str) -> bool {
        let at = h.at(text);
        h.focused()
            .and_then(|id| h.ctx.read_response(id))
            .is_some_and(|r| r.rect.contains(at))
    }

    /// A click on a tab gives the keyboard to its field; a click on a row, to the list.
    #[test]
    fn a_click_gives_the_keyboard_to_what_was_clicked() {
        let (mut h, _parent) = open();
        h.click("GitHub");
        assert!(h.ctx.text_edit_focused(), "the tab's search field");
        h.click("acme/payments");
        assert_eq!(h.focused(), Some(list_stop_id(Forge::Github)));
        let urls = listed(&h);
        let at = urls.iter().position(|u| *u == url(&h)).unwrap();
        h.key(egui::Key::ArrowUp);
        assert_eq!(url(&h), urls[at.saturating_sub(1)]);
    }

    /// Every tab is there from the start; one whose forge has no list says why, where the list
    /// would be.
    #[test]
    fn every_tab_is_there_at_once_and_says_why_it_lists_nothing() {
        let (mut h, _parent) = open();
        let dialog = h.tool.clone.as_mut().expect("open");
        let (_tx, rx) = mpsc::channel();
        dialog.github = Listing::Loading(rx);
        dialog.azure = Listing::Loaded(Repositories::default());
        h.frame();
        assert!(h.shows("URL") && h.shows("GitHub") && h.shows("Azure DevOps"));
        h.click("GitHub");
        assert!(
            h.shows("Loading your GitHub repositories…"),
            "{:?}",
            h.texts
        );
        h.tool.clone.as_mut().expect("open").github = Listing::Failed(ForgeError::NotSignedIn);
        h.frame();
        assert!(h.shows("gh auth login"), "{:?}", h.texts);
        h.click("Azure DevOps");
        assert!(
            h.shows_part("No Azure DevOps organisation"),
            "{:?}",
            h.texts
        );
    }

    /// The folder a clone would go in is there already: *Open* beside the error opens it, in
    /// place of the clone.
    #[test]
    fn an_existing_folder_can_be_opened_instead() {
        let (mut h, parent) = open();
        let url = h.path().to_string_lossy().into_owned();
        let existing = parent.path().join(clone::folder_name(&url));
        std::fs::create_dir(&existing).unwrap();
        h.type_text(&url);
        assert!(h.shows_part("already exists"), "{:?}", h.texts);
        h.click("Open");
        assert_eq!(h.tool.cloned.as_deref(), Some(existing.as_path()));
        assert!(h.tool.clone.is_none(), "the dialog closes");
    }

    #[test]
    fn cloning_a_url_typed_opens_the_clone() {
        let (mut h, parent) = open();
        let url = h.path().to_string_lossy().into_owned();
        h.type_text(&url);
        let name = clone::folder_name(&url);
        assert!(h.shows(&name), "{:?}", h.texts);
        h.click("Clone");
        h.until("the clone is done", |h| h.tool.cloned.is_some());
        let path = parent.path().join(&name);
        assert_eq!(h.tool.cloned.as_deref(), Some(path.as_path()));
        assert!(parterre_core::git::load_repo(&path).is_ok());
    }

    #[test]
    fn only_remote_urls_come_from_the_clipboard() {
        assert!(is_remote_url("https://github.com/aquamoth/parterre.git"));
        assert!(is_remote_url(" git@github.com:aquamoth/parterre.git\n"));
        assert!(!is_remote_url("/home/me/repo"));
        assert!(!is_remote_url("parterre"));
        assert!(!is_remote_url("https://a\nhttps://b"));
    }
}
