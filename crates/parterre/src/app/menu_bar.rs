//! The menus are built afresh each frame from [`State`], as a tree of [`Entry`]s that both
//! renderers draw: `bar` with egui on Windows and Linux, `macos` with AppKit.

use std::path::{Path, PathBuf};

use parterre_core::layout::Direction;
use parterre_core::physics::DragModel;
use parterre_core::revgraph::Simplification;

use super::branches;
use super::compare_window::CompareRequest;
use crate::export::Format;
use crate::keys::{self, Platform, Shortcut};
use crate::menu::Mark;

pub mod bar;
#[cfg(target_os = "macos")]
pub mod macos;

#[derive(Clone, Debug)]
pub enum Command {
    About,
    Download,
    Settings,
    InstallCommandLineTool,
    Quit,
    OpenFolder,
    CloneRepository,
    OpenRecent(PathBuf),
    ClearRecent,
    Close,
    Export(Format),
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Find,
    FindNext,
    FindPrevious,
    Reload,
    AutoReload,
    ZoomIn,
    ZoomOut,
    ActualSize,
    ZoomToFit,
    GoToHead,
    Detail(Simplification),
    Show(Shown),
    CurrentBranchOnly,
    FirstParentOnly,
    FilterSettings,
    Overview,
    StatusBar,
    ShowLog,
    Compare(CompareRequest),
    Git(branches::Request),
    Fetch,
    RememberMoves,
    ReturnAllToLayout,
    Drag(DragModel),
    Direction(Direction),
    KeyboardAndMouse,
    Legend,
    CopyText(String),
    OpenPullRequest(String),
    OpenIn(super::Opener, PathBuf),
    SelectSubtree(Vec<usize>),
    ReturnToLayout(Vec<usize>),
    CentreOn(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    LocalBranches,
    RemoteBranches,
    Tags,
    PullRequests,
    Worktrees,
    Stash,
    OtherRefs,
}

impl Shown {
    #[cfg(test)]
    pub const ALL: [Shown; 7] = [
        Shown::LocalBranches,
        Shown::RemoteBranches,
        Shown::Tags,
        Shown::PullRequests,
        Shown::Worktrees,
        Shown::Stash,
        Shown::OtherRefs,
    ];

    fn label(self) -> &'static str {
        match self {
            Shown::LocalBranches => "Local branches",
            Shown::RemoteBranches => "Remote branches",
            Shown::Tags => "Tags",
            Shown::PullRequests => "Pull requests",
            Shown::Worktrees => "Worktrees",
            Shown::Stash => "Stash",
            Shown::OtherRefs => "Other refs",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// macOS's application menu.
    App,
    File,
    Edit,
    View,
    Git,
    Layout,
    Window,
    Help,
}

impl Kind {
    pub fn usage(self) -> crate::usage::Menu {
        use crate::usage::Menu;
        match self {
            Kind::App => Menu::AppMenu,
            Kind::File => Menu::FileMenu,
            Kind::Edit => Menu::EditMenu,
            Kind::View => Menu::ViewMenu,
            Kind::Git => Menu::GitMenu,
            Kind::Layout => Menu::LayoutMenu,
            Kind::Window => Menu::WindowMenu,
            Kind::Help => Menu::HelpMenu,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Menu {
    pub kind: Kind,
    pub title: String,
    pub entries: Vec<Entry>,
    pub accent: bool,
}

#[derive(Clone, Debug)]
pub enum Entry {
    Item(Item),
    Separator,
    Heading(String),
    Submenu(Submenu),
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    System(System),
}

/// Why an entry is greyed out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// A git operation is running: the entry applies again once it's done.
    GitBusy,
    /// The branch information is loading: likewise.
    Loading,
    Other(String),
}

impl Why {
    pub fn waiting(busy: bool) -> Why {
        if busy { Why::GitBusy } else { Why::Loading }
    }

    pub fn text(&self) -> &str {
        match self {
            Why::GitBusy => branches::OPERATION_RUNNING,
            Why::Loading => branches::LOADING_BRANCHES,
            Why::Other(text) => text,
        }
    }

    pub fn is_waiting(&self) -> bool {
        matches!(self, Why::GitBusy | Why::Loading)
    }
}

impl From<&str> for Why {
    fn from(text: &str) -> Why {
        Why::Other(text.to_owned())
    }
}

impl From<String> for Why {
    fn from(text: String) -> Why {
        Why::Other(text)
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub label: String,
    /// `None` for an item that is only ever greyed out.
    pub command: Option<Command>,
    pub shortcut: Option<Shortcut>,
    pub enabled: bool,
    pub mark: Mark,
    pub accent: bool,
    pub tip: Option<String>,
    pub why_disabled: Option<Why>,
    /// Weak text in the shortcut's place.
    pub detail: Option<String>,
    pub folder_icon: Option<PathBuf>,
}

impl Item {
    pub fn new(label: impl Into<String>, command: Command) -> Item {
        Item::with_command(label.into(), Some(command))
    }

    pub fn disabled(label: impl Into<String>, why: impl Into<Why>) -> Item {
        Item::with_command(label.into(), None).blocked(Some(why))
    }

    fn with_command(label: String, command: Option<Command>) -> Item {
        Item {
            label,
            command,
            shortcut: None,
            enabled: true,
            mark: Mark::None,
            accent: false,
            tip: None,
            why_disabled: None,
            detail: None,
            folder_icon: None,
        }
    }

    pub fn key(mut self, shortcut: Shortcut) -> Item {
        self.shortcut = Some(shortcut);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Item {
        self.enabled = enabled;
        self
    }

    pub fn blocked(mut self, why: Option<impl Into<Why>>) -> Item {
        if let Some(why) = why {
            self.enabled = false;
            self.why_disabled = Some(why.into());
        }
        self
    }

    pub fn check(mut self, on: bool) -> Item {
        self.mark = Mark::Check(on);
        self
    }

    pub fn radio(mut self, on: bool) -> Item {
        self.mark = Mark::Radio(on);
        self
    }

    pub fn tip(mut self, tip: impl Into<String>) -> Item {
        self.tip = Some(tip.into());
        self
    }
}

impl From<Item> for Entry {
    fn from(item: Item) -> Entry {
        Entry::Item(item)
    }
}

#[derive(Clone, Debug)]
pub struct Submenu {
    pub label: String,
    pub entries: Vec<Entry>,
    pub enabled: bool,
    pub why_disabled: Option<Why>,
}

impl Submenu {
    pub fn new(label: impl Into<String>, entries: Vec<Entry>) -> Submenu {
        Submenu {
            label: label.into(),
            entries,
            enabled: true,
            why_disabled: None,
        }
    }
}

impl From<Submenu> for Entry {
    fn from(submenu: Submenu) -> Entry {
        Entry::Submenu(submenu)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum System {
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Minimize,
    Zoom,
    BringAllToFront,
    FullScreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Front {
    Graph,
    /// macOS only: elsewhere the menu bar is in the graph's window.
    Other,
}

#[derive(Clone, Debug)]
pub struct State {
    pub platform: Platform,
    pub front: Front,
    /// In the window in front.
    pub text_focus: bool,
    /// A modal dialog is up.
    pub locked: bool,
    pub has_repo: bool,
    /// The open one left out.
    pub recent: Vec<PathBuf>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub has_selection: bool,
    pub auto_reload: bool,
    pub detail: Simplification,
    /// What View › Show has: on, and why it can't be, if it can't.
    pub shown: Vec<(Shown, bool, Option<&'static str>)>,
    pub current_branch_only: bool,
    pub first_parent_only: bool,
    pub overview: bool,
    pub status_bar: bool,
    pub remember_moves: bool,
    pub any_displaced: bool,
    pub drag: DragModel,
    pub direction: Direction,
    pub newer_version: Option<String>,
    pub git_menu: Vec<Entry>,
}

/// Title Case on macOS, as Apple's guidelines have it: every word but articles, conjunctions
/// and prepositions of four letters or fewer, and the first and last always. Words with a
/// capital after their first letter (HEAD, WebP) and parterre's name stay as they are.
/// Elsewhere sentence case: as given.
pub fn case(platform: Platform, text: &str) -> String {
    if platform != Platform::Mac {
        return text.to_owned();
    }
    const MINOR: [&str; 22] = [
        "a", "an", "the", "and", "but", "or", "nor", "for", "so", "yet", "as", "at", "by", "in",
        "of", "on", "to", "up", "via", "with", "into", "onto",
    ];
    let words: Vec<&str> = text.split(' ').collect();
    let last = words.len().saturating_sub(1);
    let capital = |word: &str| -> String {
        let mut chars = word.chars();
        match chars.next() {
            Some(c) => c.to_uppercase().chain(chars).collect(),
            None => String::new(),
        }
    };
    words
        .iter()
        .enumerate()
        .map(|(i, &word)| {
            let keep = word == "parterre"
                || word.contains('{')
                || word.chars().skip(1).any(char::is_uppercase)
                || (i != 0 && i != last && MINOR.contains(&word));
            if keep {
                word.to_owned()
            } else {
                word.split('/').map(capital).collect::<Vec<_>>().join("/")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// [`case`] of `template`, its `{}`s then filled in with `names`, which keep their case.
pub fn case_with(platform: Platform, template: &str, names: &[&str]) -> String {
    let cased = case(platform, template);
    let mut parts = cased.split("{}");
    let mut out = parts.next().unwrap_or_default().to_owned();
    for (part, name) in parts.zip(names.iter().chain(std::iter::repeat(&""))) {
        out.push_str(name);
        out.push_str(part);
    }
    out
}

/// For each label, the index of its underlined character, unique in the menu ignoring case.
/// Chosen as KDE chooses them for menus that name none: the first letters of words first, in
/// order, then any other letter or digit of what is still without one.
pub fn access_keys(labels: &[Option<&str>]) -> Vec<Option<usize>> {
    let mut taken = std::collections::HashSet::new();
    let mut keys: Vec<Option<usize>> = vec![None; labels.len()];
    let candidates = |label: &str, initials: bool| -> Vec<(usize, char)> {
        let mut previous = ' ';
        let mut found = Vec::new();
        for (i, c) in label.chars().enumerate() {
            let starts_word = !previous.is_alphanumeric();
            if c.is_alphanumeric() && (starts_word || !initials) {
                found.push((i, c));
            }
            previous = c;
        }
        found
    };
    for initials in [true, false] {
        for (key, label) in keys.iter_mut().zip(labels) {
            let Some(label) = label else { continue };
            if key.is_some() {
                continue;
            }
            for (i, c) in candidates(label, initials) {
                if taken.insert(c.to_lowercase().collect::<String>()) {
                    *key = Some(i);
                    break;
                }
            }
        }
    }
    keys
}

/// Lower case, as typed.
pub fn access_letter(label: &str, key: Option<usize>) -> Option<String> {
    let c = label.chars().nth(key?)?;
    Some(c.to_lowercase().collect())
}

pub fn build(state: &State) -> Vec<Menu> {
    let p = state.platform;
    let mac = p == Platform::Mac;
    let mut menus = Vec::new();
    if mac {
        menus.push(Menu {
            kind: Kind::App,
            title: "parterre".into(),
            entries: app_menu(state),
            accent: false,
        });
    }
    let titled = |kind, title: &str, entries| Menu {
        kind,
        title: title.to_owned(),
        entries,
        accent: false,
    };
    menus.push(titled(Kind::File, "File", file_menu(state)));
    menus.push(titled(Kind::Edit, "Edit", edit_menu(state)));
    menus.push(titled(Kind::View, "View", view_menu(state)));
    menus.push(titled(
        Kind::Git,
        "Git",
        graph_only(state, state.git_menu.clone()),
    ));
    menus.push(titled(Kind::Layout, "Layout", layout_menu(state)));
    if mac {
        menus.push(titled(Kind::Window, "Window", window_menu()));
    }
    let mut help = titled(Kind::Help, "Help", help_menu(state));
    help.accent = !mac && state.newer_version.is_some();
    menus.push(help);
    if state.locked {
        for menu in &mut menus {
            grey_all_but_quit(&mut menu.entries);
        }
    }
    menus
}

fn grey_all_but_quit(entries: &mut [Entry]) {
    for entry in entries {
        match entry {
            Entry::Item(item) if !matches!(item.command, Some(Command::Quit)) => {
                item.enabled = false
            }
            Entry::Submenu(submenu) => grey_all_but_quit(&mut submenu.entries),
            _ => {}
        }
    }
}

/// Greyed out while another window is in front.
fn graph_only(state: &State, mut entries: Vec<Entry>) -> Vec<Entry> {
    if state.front == Front::Graph {
        return entries;
    }
    // Fetch is every window's: it fetches from the window in front.
    fn grey(entries: &mut [Entry]) {
        for entry in entries {
            match entry {
                Entry::Item(item) if !matches!(item.command, Some(Command::Fetch)) => {
                    item.enabled = false
                }
                Entry::Submenu(submenu) => {
                    submenu.enabled = false;
                    grey(&mut submenu.entries);
                }
                _ => {}
            }
        }
    }
    grey(&mut entries);
    entries
}

fn item(p: Platform, label: &str, command: Command) -> Item {
    Item::new(case(p, label), command)
}

fn app_menu(state: &State) -> Vec<Entry> {
    let p = state.platform;
    let mut entries: Vec<Entry> = vec![item(p, "About parterre", Command::About).into()];
    entries.extend(download(state));
    entries.extend([
        Entry::Separator,
        item(p, "Settings…", Command::Settings)
            .key(keys::SETTINGS)
            .into(),
        item(
            p,
            "Install command line tool…",
            Command::InstallCommandLineTool,
        )
        .tip("Puts parterre on the PATH, as /usr/local/bin/parterre")
        .into(),
        Entry::Separator,
        Entry::System(System::Services),
        Entry::Separator,
        Entry::System(System::Hide),
        Entry::System(System::HideOthers),
        Entry::System(System::ShowAll),
        Entry::Separator,
        item(p, "Quit parterre", Command::Quit)
            .key(keys::QUIT)
            .into(),
    ]);
    entries
}

fn download(state: &State) -> Option<Entry> {
    let version = state.newer_version.as_ref()?;
    let mut item = Item::new(format!("Download {version}"), Command::Download);
    item.accent = true;
    Some(item.into())
}

fn file_menu(state: &State) -> Vec<Entry> {
    let p = state.platform;
    let mac = p == Platform::Mac;
    let graph = state.front == Front::Graph;
    let names_and_places: Vec<(String, String)> = state
        .recent
        .iter()
        .map(|path| super::name_and_place(path))
        .collect();
    let mut recent: Vec<Entry> = state
        .recent
        .iter()
        .zip(&names_and_places)
        .map(|(path, (name, place))| {
            let mut item = if mac {
                // Names alone, with the folder above where two are the same.
                let clash = names_and_places.iter().filter(|(n, _)| n == name).count() > 1;
                let label = if clash {
                    let above = path
                        .parent()
                        .and_then(Path::file_name)
                        .map_or_else(|| place.clone(), |n| n.to_string_lossy().into_owned());
                    format!("{name} — {above}")
                } else {
                    name.clone()
                };
                let mut item = Item::new(label, Command::OpenRecent(path.clone()));
                item.folder_icon = Some(path.clone());
                item
            } else {
                let mut item = Item::new(name.clone(), Command::OpenRecent(path.clone()));
                item.detail = Some(place.clone());
                item
            };
            item.tip = Some(path.display().to_string());
            item.into()
        })
        .collect();
    if !recent.is_empty() {
        recent.push(Entry::Separator);
    }
    let clear = if mac {
        "Clear Menu"
    } else {
        "Clear recent folders"
    };
    recent.push(
        Item::new(clear, Command::ClearRecent)
            .enabled(!state.recent.is_empty())
            .into(),
    );
    let mut open_recent = Submenu::new(case(p, "Open recent"), recent);
    open_recent.enabled = !state.recent.is_empty() || mac;
    let close = if mac && !graph {
        item(p, "Close window", Command::Close)
    } else {
        item(p, "Close folder", Command::Close).enabled(state.has_repo)
    };
    let mut entries = vec![
        item(p, "Open folder…", Command::OpenFolder)
            .key(keys::OPEN)
            .into(),
        item(p, "Clone repository…", Command::CloneRepository).into(),
        open_recent.into(),
        Entry::Separator,
        close.key(keys::CLOSE).into(),
        Entry::Separator,
    ];
    let exports = Format::ALL.map(|format| {
        item(
            p,
            &format!("Export as {}…", format.name()),
            Command::Export(format),
        )
        .enabled(state.has_repo)
        .into()
    });
    entries.extend(graph_only(state, exports.to_vec()));
    if !mac {
        entries.push(Entry::Separator);
        entries.push(
            item(p, "Settings…", Command::Settings)
                .key(keys::SETTINGS)
                .into(),
        );
        let quit = match p {
            Platform::Windows => item(p, "Exit", Command::Quit),
            _ => item(p, "Quit", Command::Quit).key(keys::QUIT),
        };
        entries.push(quit.into());
    }
    entries
}

fn edit_menu(state: &State) -> Vec<Entry> {
    let p = state.platform;
    let mac = p == Platform::Mac;
    let graph = state.front == Front::Graph;
    let (undo, redo) = if state.text_focus && graph {
        ("Undo", "Redo")
    } else {
        ("Undo move", "Redo move")
    };
    let moves = vec![
        item(p, undo, Command::Undo)
            .key(keys::UNDO)
            .enabled(state.can_undo || state.text_focus)
            .into(),
        item(p, redo, Command::Redo)
            .key(keys::redo_on(p))
            .enabled(state.can_redo || state.text_focus)
            .into(),
    ];
    let mut entries = graph_only(state, moves);
    entries.push(Entry::Separator);
    let copy = item(p, "Copy", Command::Copy)
        .key(keys::COPY)
        .enabled(state.text_focus || !graph || state.has_selection);
    if mac {
        let text = state.text_focus;
        entries.extend([
            item(p, "Cut", Command::Cut)
                .key(keys::CUT)
                .enabled(text)
                .into(),
            copy.into(),
            item(p, "Paste", Command::Paste)
                .key(keys::PASTE)
                .enabled(text)
                .into(),
            item(p, "Select all", Command::SelectAll)
                .key(keys::SELECT_ALL)
                .enabled(text || !graph)
                .into(),
        ]);
    } else {
        entries.push(copy.into());
    }
    entries.push(Entry::Separator);
    let find = vec![
        item(p, "Find…", Command::Find).key(keys::FIND).into(),
        item(p, "Find next", Command::FindNext)
            .key(keys::find_next_on(p))
            .into(),
        item(p, "Find previous", Command::FindPrevious)
            .key(keys::find_previous_on(p))
            .into(),
    ];
    if mac {
        entries.push(Submenu::new("Find", find).into());
    } else {
        entries.extend(find);
    }
    entries
}

fn view_menu(state: &State) -> Vec<Entry> {
    let p = state.platform;
    let mac = p == Platform::Mac;
    let reload_key = keys::reload_shown_on(p);
    let mut entries = vec![
        item(p, "Reload", Command::Reload)
            .key(reload_key)
            .enabled(state.has_repo)
            .into(),
    ];
    entries.extend(graph_only(
        state,
        vec![
            item(p, "Reload automatically", Command::AutoReload)
                .check(state.auto_reload)
                .into(),
        ],
    ));
    entries.push(Entry::Separator);
    // Not graph only: in another window they set its text size.
    entries.extend([
        item(p, "Zoom in", Command::ZoomIn)
            .key(keys::ZOOM_IN)
            .into(),
        item(p, "Zoom out", Command::ZoomOut)
            .key(keys::ZOOM_OUT)
            .into(),
        item(p, "Actual size", Command::ActualSize)
            .key(keys::ACTUAL_SIZE)
            .into(),
    ]);
    let mut rest = vec![
        item(p, "Zoom to fit", Command::ZoomToFit)
            .key(keys::ZOOM_TO_FIT)
            .into(),
        Entry::Separator,
        item(p, "Go to HEAD", Command::GoToHead)
            .key(keys::GO_TO_HEAD)
            .into(),
        Entry::Separator,
        Entry::Heading(case(p, "Detail")),
    ];
    for s in Simplification::ALL {
        rest.push(
            item(p, s.label(), Command::Detail(s))
                .radio(state.detail == s)
                .into(),
        );
    }
    rest.push(Entry::Separator);
    let shown = state
        .shown
        .iter()
        .map(|&(what, on, why)| {
            item(p, what.label(), Command::Show(what))
                .check(on)
                .blocked(why)
                .into()
        })
        .collect();
    rest.push(Submenu::new(case(p, "Show"), shown).into());
    rest.extend([
        Entry::Separator,
        Entry::Heading(case(p, "Filter")),
        item(p, "Current branch only", Command::CurrentBranchOnly)
            .check(state.current_branch_only)
            .into(),
        item(p, "First parent only", Command::FirstParentOnly)
            .check(state.first_parent_only)
            .into(),
        item(
            p,
            "Branch filter and hidden branches…",
            Command::FilterSettings,
        )
        .into(),
        Entry::Separator,
    ]);
    // Apple's way: the item says what it does, rather than a check mark.
    if mac {
        let verb = |on: bool| if on { "Hide" } else { "Show" };
        rest.push(
            Item::new(
                format!("{} Overview Map", verb(state.overview)),
                Command::Overview,
            )
            .into(),
        );
        rest.push(
            Item::new(
                format!("{} Status Bar", verb(state.status_bar)),
                Command::StatusBar,
            )
            .into(),
        );
    } else {
        rest.push(
            item(p, "Overview map", Command::Overview)
                .check(state.overview)
                .into(),
        );
        rest.push(
            item(p, "Status bar", Command::StatusBar)
                .check(state.status_bar)
                .into(),
        );
    }
    entries.extend(graph_only(state, rest));
    if mac {
        entries.push(Entry::Separator);
        entries.push(Entry::System(System::FullScreen));
    }
    entries
}

fn layout_menu(state: &State) -> Vec<Entry> {
    let p = state.platform;
    let mut entries = vec![
        item(p, "Remember moved nodes", Command::RememberMoves)
            .check(state.remember_moves)
            .into(),
        item(p, "Return all nodes to layout", Command::ReturnAllToLayout)
            .enabled(state.any_displaced)
            .into(),
        Entry::Separator,
        Entry::Heading(case(p, "Dragging")),
    ];
    for (model, key) in DragModel::ALL.into_iter().zip(keys::DRAG) {
        entries.push(
            item(p, model.label(), Command::Drag(model))
                .key(key)
                .radio(state.drag == model)
                .into(),
        );
    }
    entries.push(Entry::Separator);
    entries.push(Entry::Heading(case(p, "Direction")));
    for d in Direction::ALL {
        entries.push(
            item(p, d.label(), Command::Direction(d))
                .radio(state.direction == d)
                .into(),
        );
    }
    graph_only(state, entries)
}

fn window_menu() -> Vec<Entry> {
    vec![
        Entry::System(System::Minimize),
        Entry::System(System::Zoom),
        Entry::Separator,
        Entry::System(System::BringAllToFront),
    ]
}

fn help_menu(state: &State) -> Vec<Entry> {
    let p = state.platform;
    let mac = p == Platform::Mac;
    let mut entries = Vec::new();
    if !mac && let Some(download) = download(state) {
        entries.push(download);
        entries.push(Entry::Separator);
    }
    entries.push(item(p, "Keyboard and mouse", Command::KeyboardAndMouse).into());
    entries.push(item(p, "Legend", Command::Legend).into());
    if !mac {
        entries.push(Entry::Separator);
        entries.push(item(p, "About parterre", Command::About).into());
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(platform: Platform) -> State {
        State {
            platform,
            front: Front::Graph,
            text_focus: false,
            locked: false,
            has_repo: true,
            recent: vec![PathBuf::from("/src/a/app"), PathBuf::from("/src/b/app")],
            can_undo: true,
            can_redo: false,
            has_selection: true,
            auto_reload: true,
            detail: Simplification::Decorated,
            shown: Shown::ALL.iter().map(|&s| (s, true, None)).collect(),
            current_branch_only: false,
            first_parent_only: false,
            overview: false,
            status_bar: true,
            remember_moves: false,
            any_displaced: false,
            drag: DragModel::Adapt,
            direction: Direction::NewestTop,
            newer_version: None,
            git_menu: vec![
                Item::new("Show log", Command::ShowLog).into(),
                Item::new("Fetch", Command::Fetch).into(),
            ],
        }
    }

    fn titles(menus: &[Menu]) -> Vec<&str> {
        menus.iter().map(|m| m.title.as_str()).collect()
    }

    fn labels(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| match e {
                Entry::Item(i) => i.label.clone(),
                Entry::Separator => "|".into(),
                Entry::Heading(h) => format!("[{h}]"),
                Entry::Submenu(s) => format!("{} >", s.label),
                Entry::System(s) => format!("{s:?}"),
            })
            .collect()
    }

    fn find(menus: &[Menu], kind: Kind) -> &Menu {
        menus.iter().find(|m| m.kind == kind).unwrap()
    }

    fn item_labelled<'a>(entries: &'a [Entry], label: &str) -> &'a Item {
        entries
            .iter()
            .find_map(|e| match e {
                Entry::Item(i) if i.label == label => Some(i),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {label} in {:?}", labels(entries)))
    }

    #[test]
    fn title_case_on_macos_sentence_case_elsewhere() {
        let mac = Platform::Mac;
        assert_eq!(case(mac, "Open recent"), "Open Recent");
        assert_eq!(
            case(mac, "Return all nodes to layout"),
            "Return All Nodes to Layout"
        );
        assert_eq!(case(mac, "Newest on the left"), "Newest on the Left");
        assert_eq!(case(mac, "Go to HEAD"), "Go to HEAD");
        assert_eq!(case(mac, "Export as WebP…"), "Export as WebP…");
        assert_eq!(case(mac, "About parterre"), "About parterre");
        assert_eq!(
            case(mac, "Show/hide overview map"),
            "Show/Hide Overview Map"
        );
        assert_eq!(
            case_with(mac, "Merge {} into {}…", &["feature/x", "main"]),
            "Merge feature/x into main…"
        );
        assert_eq!(
            case_with(Platform::Linux, "Rebase {} onto {}", &["main", "{}"]),
            "Rebase main onto {}"
        );
        assert_eq!(case(Platform::Windows, "Open recent"), "Open recent");
    }

    #[test]
    fn access_keys_are_first_letters_then_any_free_one() {
        let titles = ["File", "Edit", "View", "Git", "Layout", "Help"].map(Some);
        let keys = access_keys(&titles);
        assert_eq!(keys, vec![Some(0); 6], "every title its first letter");
        let file = [
            Some("Open folder…"),
            Some("Open recent"),
            None,
            Some("Close folder"),
            None,
            Some("Export as SVG…"),
            Some("Export as PNG…"),
            Some("Settings…"),
            Some("Quit"),
        ];
        let keys = access_keys(&file);
        let letters: Vec<Option<String>> = file
            .iter()
            .zip(&keys)
            .map(|(l, &k)| l.and_then(|l| access_letter(l, k)))
            .collect();
        let s = |c: &str| Some(c.to_owned());
        assert_eq!(
            letters,
            [
                s("o"),
                s("r"),
                None,
                s("c"),
                None,
                s("e"),
                s("a"),
                s("s"),
                s("q")
            ]
        );
        assert_eq!(
            access_keys(&[Some("ab"), Some("ba"), Some("b")]),
            [Some(0), Some(0), None]
        );
    }

    #[test]
    fn macos_has_the_app_and_window_menus() {
        let menus = build(&state(Platform::Mac));
        assert_eq!(
            titles(&menus),
            [
                "parterre", "File", "Edit", "View", "Git", "Layout", "Window", "Help"
            ]
        );
        let menus = build(&state(Platform::Linux));
        assert_eq!(
            titles(&menus),
            ["File", "Edit", "View", "Git", "Layout", "Help"]
        );
    }

    #[test]
    fn settings_and_about_go_where_the_platform_keeps_them() {
        let mac = build(&state(Platform::Mac));
        let app = labels(&find(&mac, Kind::App).entries);
        assert_eq!(
            app,
            [
                "About parterre",
                "|",
                "Settings…",
                "Install Command Line Tool…",
                "|",
                "Services",
                "|",
                "Hide",
                "HideOthers",
                "ShowAll",
                "|",
                "Quit parterre"
            ]
        );
        let file = labels(&find(&mac, Kind::File).entries);
        assert!(!file.contains(&"Settings…".to_owned()));
        let windows = build(&state(Platform::Windows));
        let file = labels(&find(&windows, Kind::File).entries);
        assert_eq!(&file[file.len() - 2..], ["Settings…", "Exit"]);
        let help = labels(&find(&windows, Kind::Help).entries);
        assert_eq!(
            help,
            ["Keyboard and mouse", "Legend", "|", "About parterre"]
        );
        let linux = build(&state(Platform::Linux));
        let file = &find(&linux, Kind::File).entries;
        assert_eq!(item_labelled(file, "Quit").shortcut, Some(keys::QUIT));
    }

    #[test]
    fn a_newer_release_is_offered_first_in_help_or_under_about() {
        let mut s = state(Platform::Linux);
        s.newer_version = Some("0.8.0".into());
        let menus = build(&s);
        let help = find(&menus, Kind::Help);
        assert!(help.accent);
        assert_eq!(labels(&help.entries)[..2], ["Download 0.8.0", "|"]);
        s.platform = Platform::Mac;
        let menus = build(&s);
        assert!(!find(&menus, Kind::Help).accent);
        assert_eq!(
            labels(&find(&menus, Kind::App).entries)[..2],
            ["About parterre", "Download 0.8.0"]
        );
    }

    #[test]
    fn open_recent_tells_same_names_apart() {
        let mac = build(&state(Platform::Mac));
        let Entry::Submenu(recent) = &find(&mac, Kind::File).entries[2] else {
            panic!("no Open Recent");
        };
        assert_eq!(
            labels(&recent.entries),
            ["app — a", "app — b", "|", "Clear Menu"]
        );
        let linux = build(&state(Platform::Linux));
        let file = &find(&linux, Kind::File).entries;
        assert_eq!(labels(file)[..2], ["Open folder…", "Clone repository…"]);
        let Entry::Submenu(recent) = &file[2] else {
            panic!("no Open recent");
        };
        assert_eq!(recent.label, "Open recent");
        assert_eq!(
            labels(&recent.entries),
            ["app", "app", "|", "Clear recent folders"]
        );
    }

    #[test]
    fn another_window_in_front_greys_what_only_the_graph_does() {
        let mut s = state(Platform::Mac);
        s.front = Front::Other;
        let menus = build(&s);
        let file = &find(&menus, Kind::File).entries;
        assert!(item_labelled(file, "Close Window").enabled);
        assert!(!item_labelled(file, "Export as SVG…").enabled);
        let edit = &find(&menus, Kind::Edit).entries;
        assert!(!item_labelled(edit, "Undo Move").enabled);
        assert!(item_labelled(edit, "Copy").enabled);
        let view = &find(&menus, Kind::View).entries;
        assert!(item_labelled(view, "Reload").enabled);
        assert!(item_labelled(view, "Zoom In").enabled);
        assert!(!item_labelled(view, "Zoom to Fit").enabled);
        assert!(!item_labelled(view, "Go to HEAD").enabled);
        let git = &find(&menus, Kind::Git).entries;
        assert!(!item_labelled(git, "Show log").enabled);
        assert!(item_labelled(git, "Fetch").enabled);
        let Some(Entry::Submenu(show)) = view
            .iter()
            .find(|e| matches!(e, Entry::Submenu(s) if s.label == "Show"))
        else {
            panic!("no Show");
        };
        assert!(show.entries.iter().all(|e| match e {
            Entry::Item(i) => !i.enabled,
            _ => true,
        }));
        let layout = &find(&menus, Kind::Layout).entries;
        assert!(!item_labelled(layout, "Remember Moved Nodes").enabled);
    }

    #[test]
    fn a_modal_dialog_leaves_only_quit() {
        let mut s = state(Platform::Mac);
        s.locked = true;
        let menus = build(&s);
        let app = &find(&menus, Kind::App).entries;
        assert!(item_labelled(app, "Quit parterre").enabled);
        assert!(!item_labelled(app, "Settings…").enabled);
        let file = &find(&menus, Kind::File).entries;
        assert!(!item_labelled(file, "Open Folder…").enabled);
    }

    #[test]
    fn keys_as_decided() {
        let menus = build(&state(Platform::Mac));
        let edit = &find(&menus, Kind::Edit).entries;
        let redo = item_labelled(edit, "Redo Move").shortcut.unwrap();
        assert_eq!(redo.label_on(Platform::Mac), "⇧⌘Z");
        let view = &find(&menus, Kind::View).entries;
        let reload = item_labelled(view, "Reload").shortcut.unwrap();
        assert_eq!(reload.label_on(Platform::Mac), "⌘R");
        assert_eq!(
            labels(view)
                .iter()
                .filter(|l| l.starts_with("Show "))
                .count(),
            2,
            "Show Overview Map and Show Status Bar, no check marks: {:?}",
            labels(view)
        );
        let menus = build(&state(Platform::Windows));
        let view = &find(&menus, Kind::View).entries;
        assert_eq!(
            item_labelled(view, "Reload").shortcut,
            Some(keys::RELOAD_F5)
        );
        assert!(matches!(
            item_labelled(view, "Overview map").mark,
            Mark::Check(false)
        ));
    }
}
