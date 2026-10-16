//! A tab for the URL, then one per forge this build can ask, listing the user's repositories
//! there to pick from, or why it can't. The parent folder and folder name under the tabs are
//! for all of them.

use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use eframe::egui::{self, RichText, Ui, ViewportId};
use parterre_core::branches::{Action, command_text};
use parterre_core::clone::{self, Cloning};
use parterre_forge::{Forge, ForgeError, Listed, Repositories, azure, github};

use crate::file_dialog::{self, Pending};
use crate::{dialogs, focus, widgets, worker};

const MIN_LIST_ROWS: usize = 4;
const INITIAL_LIST_ROWS: usize = 6;
const LIST_ROW_HEIGHT: f32 = 24.0;
/// What the list's frame adds to its rows: margins and line, above and below.
const LIST_FRAME_HEIGHT: f32 = 2.0 * 4.0 + 2.0;

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

/// The tabs, there from the start: the URL's, then one for each forge this build can ask.
fn tabs() -> Vec<Tab> {
    std::iter::once(Tab::Url)
        .chain(
            [Forge::Github, Forge::AzureDevops]
                .into_iter()
                .filter(|forge| forge.supported())
                .map(Tab::Forge),
        )
        .collect()
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
    scroll_to_picked: bool,
    last_scroll_offset: f32,
    clipboard_url: Option<mpsc::Receiver<Option<String>>>,
    pub opener: ViewportId,
    fresh: bool,
    /// The folder a clone would go in, there already, picked to open instead.
    pub opened: Option<PathBuf>,
}

impl CloneDialog {
    /// Starts asking GitHub and Azure DevOps for the lists, or reads `canned`
    /// (`--github-repositories-from`) and asks neither, and the clipboard if `clipboard`.
    /// `parent` is the parent folder used last; `known` the repositories opened before, whose
    /// remotes name Azure DevOps organisations.
    pub fn new(
        ctx: &egui::Context,
        parent: Option<&Path>,
        canned: Option<Arc<str>>,
        known: Vec<PathBuf>,
        clipboard: bool,
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
            scroll_to_picked: false,
            last_scroll_offset: 0.0,
            clipboard_url: clipboard
                .then(|| worker::spawn(ctx, || clipboard_text().filter(|t| remote_url(t)))),
            opener,
            fresh: true,
            opened: None,
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

    /// The folder name follows the URL unless typed.
    fn set_url(&mut self, url: String) {
        self.url = url;
        if !self.folder_name_edited {
            self.folder_name = self.followed_name();
        }
    }

    /// The folder name the URL gives, none for words looked for.
    fn followed_name(&self) -> String {
        if clone::is_url(&self.url) {
            clone::folder_name(&self.url)
        } else {
            String::new()
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
                let labels: Vec<(Tab, String)> = tabs()
                    .into_iter()
                    .map(|tab| match tab {
                        Tab::Url => (tab, "URL".to_owned()),
                        Tab::Forge(forge) => (tab, forge.to_string()),
                    })
                    .collect();
                let items: Vec<(Tab, &str)> =
                    labels.iter().map(|(t, l)| (*t, l.as_str())).collect();
                let before = self.tab;
                let mut tab = self.tab;
                let mut chosen = None;
                let tabs_id = egui::Id::new("clone-tabs");
                // Left and Right on the tabs leave them the keyboard; a click doesn't.
                let on_tabs = ui.memory(|m| m.had_focus_last_frame(tabs_id))
                    && !ui.input(|i| i.pointer.any_click());
                // A build that can ask no forge has the URL's page alone, with no tab.
                if items.len() > 1 {
                    widgets::tabs(ui, tabs_id, &mut tab, &items, |ui| {
                        self.page(ui, &mut chosen)
                    });
                } else {
                    self.page(ui, &mut chosen);
                }
                self.tab = tab;
                self.focus_field |= self.tab != before && !on_tabs;
                if let Some((url, twice)) = chosen {
                    self.set_url(url);
                    if twice && self.cloning().error().is_none() && !busy {
                        return dialogs::Answer::Primary;
                    }
                }
                ui.add_space(8.0);
                self.folder_fields(ui);
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
                            self.opened = Some(existing);
                        }
                    });
                }
                let commands: Vec<String> = if is_url {
                    vec![command_text(&clone::command(&cloning))]
                } else {
                    Vec::new()
                };
                dialogs::command_box(ui, &commands);
                let ready = is_url && error.is_none() && !busy;
                dialogs::actions(ui, dialogs::Primary::new("Clone").enabled(ready))
            });
        let mut answer = shown.inner;
        if shown.should_close() && answer == dialogs::Answer::Open {
            answer = dialogs::Answer::Cancel;
        }
        answer
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

    /// The selected tab's page: its field, and under it an area as tall in every tab, so that
    /// the dialog keeps its size. `chosen` gets the URL of a repository clicked, and whether it
    /// was double-clicked.
    fn page(&mut self, ui: &mut Ui, chosen: &mut Option<(String, bool)>) {
        let width = ui.available_width();
        let field = match self.tab {
            Tab::Url => {
                let mut url = self.url.clone();
                let response = widgets::text_field(ui, &mut url, "https://…, git@…", width);
                if response.changed() {
                    self.set_url(url);
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
            match self.tab {
                Tab::Url => note(ui, height, |ui| {
                    ui.label(
                        "Any URL git clones from: GitHub, Azure DevOps or any other host, over \
                         HTTPS or SSH. git signs in with its credential helpers, as for a fetch.",
                    );
                }),
                Tab::Forge(forge) => self.forge_area(ui, forge, height - LIST_FRAME_HEIGHT, chosen),
            }
            ((), rows(INITIAL_LIST_ROWS))
        });
    }

    /// `forge`'s repositories that the words looked for match, in a list `height` tall inside
    /// its frame, or why there are none.
    fn forge_area(
        &mut self,
        ui: &mut Ui,
        forge: Forge,
        height: f32,
        chosen: &mut Option<(String, bool)>,
    ) {
        let listing = match forge {
            Forge::Github => &self.github,
            Forge::AzureDevops => &self.azure,
        };
        let note_height = height + LIST_FRAME_HEIGHT;
        let repos = match listing {
            Listing::Loading(_) => {
                return note(ui, note_height, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Loading your {forge} repositories…"));
                    });
                });
            }
            Listing::Failed(e) if e.needs_sign_in() && forge == Forge::Github => {
                return note(ui, note_height, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.label("Sign in with ");
                        ui.label(RichText::new("gh auth login").monospace());
                        ui.label(
                            " in a terminal, and your GitHub repositories are listed here the \
                             next time.",
                        );
                    });
                });
            }
            Listing::Failed(e) => {
                let e = e.to_string();
                return note(ui, note_height, |ui| {
                    ui.label(format!("Couldn't list your {forge} repositories: {e}."));
                });
            }
            Listing::Loaded(repos) if repos.list.is_empty() => {
                return note(ui, note_height, |ui| match forge {
                    Forge::Github => {
                        ui.label("Your GitHub account has no repositories.");
                    }
                    Forge::AzureDevops => {
                        ui.label(
                            "No Azure DevOps organisation that git has a sign-in for was found. \
                             Clone one of its repositories by its URL, signing in when git asks, \
                             and the organisation's repositories are listed here the next time.",
                        );
                    }
                });
            }
            Listing::Loaded(repos) => repos,
        };
        let picked = repos.find_url(self.url.trim());
        let shown = repos.matching(&self.query);
        let reveal = std::mem::take(&mut self.scroll_to_picked)
            .then(|| shown.iter().position(|l| Some(*l) == picked))
            .flatten();
        let weak = ui.visuals().weak_text_color();
        let spacing = ui.spacing().item_spacing.y;
        // One stop for Tab, which the keys move the pick in, as far as it shows.
        let list = list_id(forge);
        let at = picked.and_then(|p| shown.iter().position(|l| l.name == p.name));
        let page = ((height + spacing) / (LIST_ROW_HEIGHT + spacing)).floor() as usize;
        let keys = widgets::list_keys(ui, list, at, shown.len(), page);
        if let Some(i) = keys.select {
            *chosen = Some((shown[i].url.clone(), false));
        } else if keys.enter
            && let Some(i) = at
        {
            *chosen = Some((shown[i].url.clone(), true));
        }
        let at = keys.select.or(at);
        let cue = focus::shows_own_by_id(ui.ctx(), list).then(|| at.unwrap_or(0));
        let mut cued = false;
        let mut clicked = false;
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, widgets::tones(ui).field_line))
            .corner_radius(7)
            .inner_margin(4)
            .show(ui, |ui| {
                if shown.is_empty() {
                    ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                        ui.set_min_size(egui::vec2(ui.available_width(), height));
                        ui.add_space(4.0);
                        ui.colored_label(
                            weak,
                            format!("  None of your {forge} repositories match."),
                        );
                    });
                    return;
                }
                let mut area = egui::ScrollArea::vertical()
                    .id_salt(("clone-list", forge))
                    .max_height(height)
                    .min_scrolled_height(height)
                    .auto_shrink([false, false]);
                if let Some(ix) = reveal {
                    let offset =
                        ix as f32 * (LIST_ROW_HEIGHT + spacing) - (height - LIST_ROW_HEIGHT) / 2.0;
                    area = area.vertical_scroll_offset(offset.max(0.0));
                } else if let Some(ix) = keys.select {
                    // Moved to by the keyboard: scrolled only as far as brings it in view.
                    let top = ix as f32 * (LIST_ROW_HEIGHT + spacing);
                    let offset = self
                        .last_scroll_offset
                        .max(top + LIST_ROW_HEIGHT - height)
                        .min(top);
                    area = area.vertical_scroll_offset(offset.max(0.0));
                }
                let room = egui::Rect::from_min_size(
                    ui.cursor().min,
                    egui::vec2(ui.available_width(), height),
                );
                let stop = widgets::list_stop(ui, list, room);
                let output = area.show_rows(ui, LIST_ROW_HEIGHT, shown.len(), |ui, rows| {
                    for (ix, listed) in shown.iter().enumerate().skip(rows.start).take(rows.len()) {
                        let row = list_row(ui, listed, at == Some(ix));
                        if cue == Some(ix) {
                            widgets::paint_list_cue(ui, row.rect);
                            cued = true;
                        }
                        focus::click_row(&row, list);
                        let row = if listed.description.is_empty() {
                            row
                        } else {
                            row.on_hover_text(&listed.description)
                        };
                        if row.clicked() || row.double_clicked() {
                            *chosen = Some((listed.url.clone(), row.double_clicked()));
                            clicked = true;
                        }
                    }
                });
                self.last_scroll_offset = output.state.offset.y;
                widgets::list_stop_ring(ui, &stop, cued);
            });
        if clicked {
            self.scroll_to_picked = true;
        }
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
            self.folder_name = self.followed_name();
        }
    }
}

/// The stop for Tab of `forge`'s list.
fn list_id(forge: Forge) -> egui::Id {
    egui::Id::new(("clone-list-stop", forge))
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
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width() - 16.0);
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let at = egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(at, galley, visuals.text_color());
    response
}

/// A row's text: the repository's `owner/name`, then whether it's private, a fork or archived.
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
            10.0,
            egui::TextFormat::simple(font, ui.visuals().weak_text_color()),
        );
    }
    job
}

/// A URL of a remote, not a local path: what's worth taking from the clipboard.
fn remote_url(text: &str) -> bool {
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
        let tab = |h: &Harness| h.tool.clone.as_ref().expect("open").tab;
        let press = |h: &mut Harness, modifiers: egui::Modifiers| {
            h.modifiers = modifiers;
            h.key(egui::Key::Tab);
            h.modifiers = egui::Modifiers::NONE;
        };
        let back = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
        press(&mut h, egui::Modifiers::CTRL);
        assert_eq!(tab(&h), Tab::Forge(Forge::Github));
        press(&mut h, back);
        assert_eq!(tab(&h), Tab::Url);
        press(&mut h, back);
        assert_eq!(tab(&h), Tab::Forge(Forge::AzureDevops));
        press(&mut h, egui::Modifiers::CTRL);
        assert_eq!(tab(&h), Tab::Url);
    }

    fn focused(h: &Harness) -> Option<egui::Id> {
        h.ctx.memory(|m| m.focused())
    }

    fn url(h: &Harness) -> String {
        h.tool.clone.as_ref().expect("open").url.clone()
    }

    /// GitHub's repositories as its list shows them, with no words looked for.
    fn listed(h: &Harness) -> Vec<String> {
        let Listing::Loaded(repos) = &h.tool.clone.as_ref().expect("open").github else {
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
        h.modifiers = egui::Modifiers::SHIFT;
        h.key(egui::Key::Tab);
        h.modifiers = egui::Modifiers::NONE;
        let tabs = egui::Id::new("clone-tabs");
        assert_eq!(focused(&h), Some(tabs), "back from the field: the tabs");
        h.key(egui::Key::ArrowRight);
        assert_eq!(
            h.tool.clone.as_ref().unwrap().tab,
            Tab::Forge(Forge::Github)
        );
        assert_eq!(
            focused(&h),
            Some(tabs),
            "Right leaves the keyboard with the tabs"
        );
        h.key(egui::Key::ArrowLeft);
        h.key(egui::Key::ArrowLeft);
        assert_eq!(
            h.tool.clone.as_ref().unwrap().tab,
            Tab::Url,
            "and stops at the first"
        );
        h.key(egui::Key::ArrowRight);
        h.key(egui::Key::Tab);
        assert!(h.ctx.text_edit_focused(), "into the page: its search field");
        h.key(egui::Key::Tab);
        assert_eq!(focused(&h), Some(list_id(Forge::Github)), "then the list");
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
            focused(&h),
            Some(list_id(Forge::Github)),
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
        focused(h)
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
        assert_eq!(focused(&h), Some(list_id(Forge::Github)));
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
        assert!(remote_url("https://github.com/aquamoth/parterre.git"));
        assert!(remote_url(" git@github.com:aquamoth/parterre.git\n"));
        assert!(!remote_url("/home/me/repo"));
        assert!(!remote_url("parterre"));
        assert!(!remote_url("https://a\nhttps://b"));
    }
}
