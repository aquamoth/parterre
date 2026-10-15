//! *Clone repository…* (#358): one field for the URL, which also filters the signed-in user's
//! GitHub and Azure DevOps repositories listed under it, then the parent folder and the clone's
//! folder name.
//! The branch tool runs the clone like a fetch, and the app opens it once it's done.

use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use eframe::egui::{self, RichText, Ui, ViewportId};
use parterre_core::branches::{Action, command_text};
use parterre_core::clone::{self, Cloning};
use parterre_forge::{Forge, ForgeError, Listed, Repositories, azure, github};

use crate::file_dialog::Pending;
use crate::{dialogs, widgets};

/// The list's rows, at least, while the dialog is resized.
const MIN_ROWS: usize = 4;
/// The list's rows at most when the dialog opens.
const SHOWN_ROWS: usize = 6;
const ROW: f32 = 24.0;

/// The user's repositories on a forge, as far as they've come.
#[derive(Debug)]
enum Listing {
    Loading(mpsc::Receiver<Result<Repositories, ForgeError>>),
    Loaded(Repositories),
    Failed(ForgeError),
}

impl Listing {
    /// Takes in what the worker found; true if it came.
    fn receive(&mut self, forge: Forge) -> bool {
        let Listing::Loading(rx) = self else {
            return false;
        };
        *self = match rx.try_recv() {
            Ok(Ok(repos)) => Listing::Loaded(repos),
            Ok(Err(e)) => Listing::Failed(e),
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                Listing::Failed(ForgeError::Network(forge, "the request stopped".into()))
            }
        };
        true
    }

    fn list(&self) -> &[Listed] {
        match self {
            Listing::Loaded(repos) => &repos.list,
            _ => &[],
        }
    }
}

#[derive(Debug)]
pub struct CloneDialog {
    url: String,
    parent: String,
    name: String,
    /// The folder name was typed, so it no longer follows the URL.
    name_edited: bool,
    browse: Option<Pending<()>>,
    github: Listing,
    azure: Listing,
    /// What both have listed: GitHub's, then Azure DevOps'.
    repos: Repositories,
    /// The list scrolls to the repository picked, which may have been found by words.
    reveal: bool,
    /// A URL from the clipboard, for an empty field.
    clipboard: Option<mpsc::Receiver<Option<String>>>,
    pub opener: ViewportId,
    fresh: bool,
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
                Listing::Loading(worker(ctx, github::repositories)),
                Listing::Loading(worker(ctx, move || azure::repositories(&known))),
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
            name: String::new(),
            name_edited: false,
            browse: None,
            repos: Repositories {
                list: [github.list(), azure.list()].concat(),
            },
            github,
            azure,
            reveal: false,
            clipboard: clipboard
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
            name: self.name.trim().to_owned(),
        }
    }

    pub fn action(&self) -> Action {
        Action::Clone(Box::new(self.cloning()))
    }

    /// The URL is set: the folder name follows it unless typed.
    fn set_url(&mut self, url: String) {
        self.url = url;
        if !self.name_edited {
            self.name = self.followed_name();
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
        let github = self.github.receive(Forge::Github);
        if self.azure.receive(Forge::AzureDevops) || github {
            self.repos.list = [self.github.list(), self.azure.list()].concat();
        }
        let clipboard = self.clipboard.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(text) => Some(text),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(None),
        });
        if let Some(text) = clipboard {
            self.clipboard = None;
            if let Some(text) = text
                && self.url.is_empty()
            {
                self.set_url(text.trim().to_owned());
            }
        }
        if let Some(answer) = self.browse.as_ref().and_then(|p| p.answer()) {
            self.browse = None;
            if let Some(dir) = answer {
                self.parent = dir.to_string_lossy().into_owned();
            }
        }
    }

    /// `busy` while another git operation runs.
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
                let hint = if self.repos.list.is_empty() && !self.loading() {
                    "https://…, git@…"
                } else {
                    "https://…, git@…, or search your repositories"
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
                self.repository_list(ui, &mut chosen);
                if let Some((url, twice)) = chosen {
                    self.set_url(url);
                    if twice && self.cloning().error().is_none() && !busy {
                        return dialogs::Answer::Primary;
                    }
                }
                ui.add_space(8.0);
                self.folder_fields(ui);
                let cloning = self.cloning();
                let error = clone::is_url(&cloning.url)
                    .then(|| cloning.error())
                    .flatten();
                if let Some(error) = &error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                let commands: Vec<String> = if clone::is_url(&cloning.url) {
                    vec![command_text(&clone::command(&cloning))]
                } else {
                    Vec::new()
                };
                dialogs::command_box(ui, &commands);
                let ready = clone::is_url(&cloning.url) && error.is_none() && !busy;
                dialogs::actions(ui, "Clone", ready, false, false)
            });
        let mut answer = shown.inner;
        if shown.should_close() && answer == dialogs::Answer::Open {
            answer = dialogs::Answer::Cancel;
        }
        answer
    }

    /// A forge is still being asked.
    fn loading(&self) -> bool {
        [&self.github, &self.azure]
            .iter()
            .any(|l| matches!(l, Listing::Loading(_)))
    }

    /// The repositories the field matches, GitHub's then Azure DevOps', with a line while they
    /// load, and one for each forge saying why it has none, if there's anything to say (not to
    /// someone who doesn't use Azure DevOps). `chosen` gets the URL of the one clicked, and
    /// whether it was double-clicked.
    fn repository_list(&mut self, ui: &mut Ui, chosen: &mut Option<(String, bool)>) {
        let weak = ui.visuals().weak_text_color();
        if self.loading() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.colored_label(weak, "Loading your repositories…");
            });
        }
        for (listing, forge) in [
            (&self.github, Forge::Github),
            (&self.azure, Forge::AzureDevops),
        ] {
            match listing {
                // A build without the forge has no list to speak of.
                Listing::Loading(_)
                | Listing::Loaded(_)
                | Listing::Failed(ForgeError::Unsupported(_)) => {}
                Listing::Failed(e) if e.needs_sign_in() && forge == Forge::Github => {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.colored_label(weak, "Sign in with ");
                        ui.label(RichText::new("gh auth login").monospace());
                        ui.colored_label(weak, " to pick from your GitHub repositories.");
                    });
                }
                Listing::Failed(e) => {
                    ui.colored_label(
                        weak,
                        format!("Couldn't list your {forge} repositories: {e}."),
                    );
                }
            }
        }
        let repos = &self.repos;
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
        let reveal = std::mem::take(&mut self.reveal)
            .then(|| shown.iter().position(|l| Some(*l) == picked))
            .flatten();
        // Rows and the frame around them: its margins and line. The box keeps its height
        // whatever is found, at most the dialog's.
        let spacing = ui.spacing().item_spacing.y;
        let frame = 2.0 * 4.0 + 2.0;
        let rows = |n: usize| n as f32 * (ROW + spacing) - spacing + frame;
        dialogs::growing(ui, rows(MIN_ROWS), |ui, height| {
            let height = height.unwrap_or(rows(SHOWN_ROWS)) - frame;
            egui::Frame::new()
                .stroke(egui::Stroke::new(1.0, widgets::tones(ui).field_line))
                .corner_radius(7)
                .inner_margin(4)
                .show(ui, |ui| {
                    if shown.is_empty() {
                        ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                            ui.set_min_size(egui::vec2(ui.available_width(), height));
                            ui.add_space(4.0);
                            ui.colored_label(weak, "  None of your repositories match.");
                        });
                        return;
                    }
                    let mut area = egui::ScrollArea::vertical()
                        .id_salt("clone-github-list")
                        .max_height(height)
                        .min_scrolled_height(height)
                        .auto_shrink([false, false]);
                    if let Some(ix) = reveal {
                        let offset = ix as f32 * (ROW + spacing) - (height - ROW) / 2.0;
                        area = area.vertical_scroll_offset(offset.max(0.0));
                    }
                    area.show_rows(ui, ROW, shown.len(), |ui, rows| {
                        for listed in &shown[rows] {
                            let selected = picked.is_some_and(|p| p.name == listed.name);
                            let row = list_row(ui, listed, selected);
                            let row = if listed.description.is_empty() {
                                row
                            } else {
                                row.on_hover_text(&listed.description)
                            };
                            if row.clicked() || row.double_clicked() {
                                *chosen = Some((listed.url.clone(), row.double_clicked()));
                            }
                        }
                    });
                });
            ((), rows(SHOWN_ROWS))
        });
        if chosen.is_some() {
            self.reveal = true;
        }
    }

    fn folder_fields(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Parent folder").strong());
        let browsing = self.browse.is_some();
        let (_, browse) = widgets::folder_field(ui, &mut self.parent, "Folder", browsing);
        if browse {
            let mut dialog = rfd::AsyncFileDialog::new().set_title("Parent folder");
            if let Some(dir) = Path::new(self.parent.trim())
                .ancestors()
                .find(|a| a.is_dir())
            {
                dialog = dialog.set_directory(dir);
            }
            self.browse = Some(Pending::start((), dialog.pick_folder(), ui.ctx()));
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Folder name").strong());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let width = ui.available_width() - widgets::BUTTON - 4.0;
            let mut name = self.name.clone();
            if widgets::text_field(ui, &mut name, "Folder name", width).changed() {
                // Separators and what Windows forbids can't be typed.
                self.name = name
                    .chars()
                    .filter(|c| !parterre_core::worktree_folder::forbidden(*c))
                    .collect();
                self.name_edited = true;
            }
            if ui
                .add_enabled_ui(self.name_edited, |ui| {
                    widgets::icon_button(ui, parterre_core::glyphs::RESET, false)
                })
                .inner
                .on_hover_text("Follow the URL")
                .clicked()
            {
                self.name_edited = false;
                self.name = self.followed_name();
            }
        });
    }
}

/// A row of the list, as wide as it, highlighted when `selected` or hovered.
fn list_row(ui: &mut Ui, listed: &Listed, selected: bool) -> egui::Response {
    let size = egui::vec2(ui.available_width(), ROW);
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

/// Runs `work` on a worker thread, repainting when it's done.
fn worker<T: Send + 'static>(
    ctx: &egui::Context,
    work: impl FnOnce() -> T + Send + 'static,
) -> mpsc::Receiver<T> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(work());
        ctx.request_repaint();
    });
    rx
}

/// A URL of a remote, not a local path: what's worth taking from the clipboard.
fn remote_url(text: &str) -> bool {
    let text = text.trim();
    clone::is_url(text)
        && !text.contains('\n')
        && !text.starts_with(['/', '\\', '~', '.'])
        && !Path::new(text).is_absolute()
}

/// The text on the clipboard.
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
        h.until("the clone is done", |h| h.tool.open.is_some());
        let path = parent.path().join(&name);
        assert_eq!(h.tool.open.as_deref(), Some(path.as_path()));
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
