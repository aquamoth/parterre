//! The URL field also filters the signed-in user's GitHub repositories, listed under it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use eframe::egui::{self, RichText, Ui, ViewportId};
use parterre_core::branches::{Action, command_text};
use parterre_core::clone::{self, Cloning};
use parterre_forge::ForgeError;
use parterre_forge::github::{self, Listed, Repositories};

use super::branches::{folder_name_field, pick_folder, received, worker};
use crate::file_dialog::Pending;
use crate::{dialogs, widgets};

const MIN_LIST_ROWS: usize = 4;
const INITIAL_LIST_ROWS: usize = 6;
const LIST_ROW_HEIGHT: f32 = 24.0;

#[derive(Debug)]
enum Github {
    Loading(mpsc::Receiver<Result<Repositories, ForgeError>>),
    Loaded(Repositories),
    Failed(ForgeError),
}

#[derive(Debug)]
pub struct CloneDialog {
    url: String,
    parent: String,
    folder_name: String,
    folder_name_edited: bool,
    browse: Option<Pending<()>>,
    github: Github,
    scroll_to_picked: bool,
    clipboard_url: Option<mpsc::Receiver<Option<String>>>,
    pub opener: ViewportId,
    fresh: bool,
}

impl CloneDialog {
    /// Starts asking GitHub for the list, or reads `canned` (`--github-repositories-from`),
    /// and the clipboard if `clipboard`. `parent` is the parent folder used last.
    pub fn new(
        ctx: &egui::Context,
        parent: Option<&Path>,
        canned: Option<Arc<str>>,
        clipboard: bool,
        opener: ViewportId,
    ) -> Self {
        let github = match canned {
            Some(json) => match github::repositories_canned(&json) {
                Ok(repos) => Github::Loaded(repos),
                Err(e) => Github::Failed(e),
            },
            None => Github::Loading(worker(ctx, github::repositories)),
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
            scroll_to_picked: false,
            clipboard_url: clipboard
                .then(|| worker(ctx, || clipboard_text().filter(|t| remote_url(t)))),
            opener,
            fresh: true,
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
        if let Github::Loading(rx) = &self.github {
            let stopped = || Err(ForgeError::Network("the request stopped".into()));
            match received(rx, stopped) {
                Some(Ok(repos)) => self.github = Github::Loaded(repos),
                Some(Err(e)) => self.github = Github::Failed(e),
                None => {}
            }
        }
        let clipboard_url = self
            .clipboard_url
            .as_ref()
            .and_then(|rx| received(rx, || None));
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
        let shown = dialogs::Dialog::new("clone", "Clone a repository")
            .screen(crate::usage::Screen::Clone)
            .width(520.0)
            .opener(self.opener)
            .raise(fresh)
            .resizable()
            .show(ctx, |ui| {
                ui.label(RichText::new("Repository URL").strong());
                let mut url = self.url.clone();
                let hint = match &self.github {
                    Github::Failed(_) => "https://…, git@…",
                    _ => "https://…, git@…, or search your GitHub repositories",
                };
                let response = widgets::text_field(ui, &mut url, hint, ui.available_width());
                if fresh {
                    response.request_focus();
                }
                if response.changed() {
                    self.set_url(url);
                }
                ui.add_space(4.0);
                let mut chosen = None;
                self.github_list(ui, &mut chosen);
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
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                let commands: Vec<String> = if is_url {
                    vec![command_text(&clone::command(&cloning))]
                } else {
                    Vec::new()
                };
                dialogs::command_box(ui, &commands);
                let ready = is_url && error.is_none() && !busy;
                dialogs::actions(ui, "Clone", ready, false, false)
            });
        let mut answer = shown.inner;
        if shown.should_close() && answer == dialogs::Answer::Open {
            answer = dialogs::Answer::Cancel;
        }
        answer
    }

    /// The GitHub repositories the field matches, or a line saying why there are none.
    /// `chosen` gets the URL of the one clicked, and whether it was double-clicked.
    fn github_list(&mut self, ui: &mut Ui, chosen: &mut Option<(String, bool)>) {
        let weak = ui.visuals().weak_text_color();
        let repos = match &self.github {
            Github::Loading(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.colored_label(weak, "Loading your GitHub repositories…");
                });
                return;
            }
            // A build without GitHub has no list to speak of.
            Github::Failed(ForgeError::Unsupported) => return,
            Github::Failed(e) if e.needs_sign_in() => {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.colored_label(weak, "Sign in with ");
                    ui.label(RichText::new("gh auth login").monospace());
                    ui.colored_label(weak, " to pick from your GitHub repositories.");
                });
                return;
            }
            Github::Failed(e) => {
                ui.colored_label(
                    weak,
                    format!("Couldn't list your GitHub repositories: {e}."),
                );
                return;
            }
            Github::Loaded(repos) => repos,
        };
        if repos.list.is_empty() {
            return;
        }
        // Words look for repositories; a URL shows them all, the one it names picked.
        let url = self.url.trim();
        let picked = repos.find_url(url);
        let shown: Vec<&Listed> = if clone::is_url(url) {
            repos.list.iter().collect()
        } else {
            repos.matching(url)
        };
        let reveal = std::mem::take(&mut self.scroll_to_picked)
            .then(|| shown.iter().position(|l| Some(*l) == picked))
            .flatten();
        // Rows and the frame around them: its margins and line. The box keeps its height
        // whatever is found, at most the dialog's.
        let spacing = ui.spacing().item_spacing.y;
        let frame = 2.0 * 4.0 + 2.0;
        let rows = |n: usize| n as f32 * (LIST_ROW_HEIGHT + spacing) - spacing + frame;
        dialogs::growing(ui, rows(MIN_LIST_ROWS), |ui, height| {
            let height = height.unwrap_or(rows(INITIAL_LIST_ROWS)) - frame;
            egui::Frame::new()
                .stroke(egui::Stroke::new(1.0, widgets::tones(ui).field_line))
                .corner_radius(7)
                .inner_margin(4)
                .show(ui, |ui| {
                    if shown.is_empty() {
                        ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                            ui.set_min_size(egui::vec2(ui.available_width(), height));
                            ui.add_space(4.0);
                            ui.colored_label(weak, "  None of your GitHub repositories match.");
                        });
                        return;
                    }
                    let mut area = egui::ScrollArea::vertical()
                        .id_salt("clone-github-list")
                        .max_height(height)
                        .min_scrolled_height(height)
                        .auto_shrink([false, false]);
                    if let Some(ix) = reveal {
                        let offset = ix as f32 * (LIST_ROW_HEIGHT + spacing)
                            - (height - LIST_ROW_HEIGHT) / 2.0;
                        area = area.vertical_scroll_offset(offset.max(0.0));
                    }
                    area.show_rows(ui, LIST_ROW_HEIGHT, shown.len(), |ui, rows| {
                        for listed in &shown[rows] {
                            let selected = picked.is_some_and(|p| p.name == listed.name);
                            let row = list_row(ui, listed, selected);
                            let row = if listed.description.is_empty() {
                                row
                            } else {
                                row.on_hover_text(&listed.description)
                            };
                            if row.clicked() || row.double_clicked() {
                                *chosen = Some((repos.url(listed), row.double_clicked()));
                            }
                        }
                    });
                });
            ((), rows(INITIAL_LIST_ROWS))
        });
        if chosen.is_some() {
            self.scroll_to_picked = true;
        }
    }

    fn folder_fields(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Parent folder").strong());
        let browsing = self.browse.is_some();
        let (_, browse) = widgets::folder_field(ui, &mut self.parent, "Folder", browsing);
        if browse {
            self.browse = Some(pick_folder(ui.ctx(), "Parent folder", self.parent.trim()));
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Folder name").strong());
        let name = &mut self.folder_name;
        if folder_name_field(ui, name, &mut self.folder_name_edited, "Follow the URL") {
            self.folder_name = self.followed_name();
        }
    }
}

/// A row of the list, as wide as it, highlighted when `selected` or hovered.
fn list_row(ui: &mut Ui, listed: &Listed, selected: bool) -> egui::Response {
    let size = egui::vec2(ui.available_width(), LIST_ROW_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
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
        h.tool.clone_repository(&ctx, ViewportId::ROOT);
        h.until("the dialog opens", |h| h.shows("Clone a repository"));
        for _ in 0..5 {
            h.frame();
        }
        (h, parent)
    }

    #[test]
    fn words_filter_the_list_and_a_pick_fills_the_url_and_folder() {
        let (mut h, _parent) = open();
        assert!(h.shows("mira/checkoutprivate") && h.shows("acme/payments"));
        h.type_text("graph");
        assert!(h.shows("mira/parterre"));
        assert!(!h.shows("acme/payments"), "{:?}", h.texts);
        // The field's hint shows under its label: words name no folder.
        let folder = |h: &Harness| h.texts.iter().filter(|(t, _)| t == "Folder name").count();
        assert_eq!(folder(&h), 2);
        h.click("mira/parterre");
        assert!(h.shows("https://github.com/mira/parterre.git"));
        assert!(h.shows("parterre"), "the folder follows the URL");
        assert_eq!(folder(&h), 1);
        // A URL shows them all again, the one it names picked.
        assert!(h.shows("acme/payments"));
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
