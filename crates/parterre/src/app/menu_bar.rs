//! The menu bar (#339, laid out in #344): parterre's commands, in menus at the top. On macOS
//! the system's menu bar, in Title Case; on Windows and Linux a row of parterre's own above the
//! toolbar, in sentence case. The menus are built afresh each frame from what the app holds
//! ([`State`]), as a tree of [`Entry`]s that both renderers draw: `bar` with egui, `macos`
//! with AppKit. A chosen item gives its [`Command`], which acts on the window in front: what
//! only the graph does is greyed out while another window is in front.

use std::path::PathBuf;

use parterre_core::layout::Direction;
use parterre_core::physics::DragModel;
use parterre_core::revgraph::Simplification;

use super::branches;
use super::compare_window::CompareRequest;
use crate::export::Format;
use crate::keys::{self, Platform, Shortcut};
use crate::menu::Mark;

// Drawn on Windows and Linux; macOS has the system's.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub mod bar;
#[cfg(target_os = "macos")]
pub mod macos;

/// What a menu item does.
#[derive(Clone, Debug)]
pub enum Command {
    About,
    Download,
    Settings,
    InstallCommandLineTool,
    Quit,
    OpenFolder,
    OpenRecent(PathBuf),
    ClearRecent,
    /// Close Folder, or with another window in front (macOS), Close Window.
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
    /// PROTOTYPE (#323): the right-click menu's own items.
    Proto(super::prototype_context_menu::Action),
}

/// What View › Show turns on and off.
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

/// Which menu it is, for the usage statistics and for the renderers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The application menu, named parterre (macOS).
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

/// A menu of the bar.
#[derive(Clone, Debug)]
pub struct Menu {
    pub kind: Kind,
    pub title: String,
    pub entries: Vec<Entry>,
    /// The title in the accent colour: Help while a newer release is out (Windows, Linux).
    pub accent: bool,
}

#[derive(Clone, Debug)]
pub enum Entry {
    Item(Item),
    Separator,
    /// A small title over the items that follow, such as Detail or Filter.
    Heading(String),
    Submenu(Submenu),
    /// What macOS itself provides.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    System(System),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub label: String,
    pub command: Command,
    pub shortcut: Option<Shortcut>,
    pub enabled: bool,
    pub mark: Mark,
    /// In the accent colour, bold: *Download*.
    pub accent: bool,
    /// Said on hover, enabled or not.
    pub tip: Option<String>,
    /// Said on hover while it is greyed out: why.
    pub why: Option<String>,
    /// Weak text where a shortcut would go: the folder a recent one is in (Windows, Linux).
    pub detail: Option<String>,
    /// A folder whose icon it shows (macOS's Open Recent).
    pub icon: Option<PathBuf>,
}

impl Item {
    pub fn new(label: impl Into<String>, command: Command) -> Item {
        Item {
            label: label.into(),
            command,
            shortcut: None,
            enabled: true,
            mark: Mark::None,
            accent: false,
            tip: None,
            why: None,
            detail: None,
            icon: None,
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

    /// Greyed out for `why`, if there is one.
    pub fn blocked(mut self, why: Option<impl Into<String>>) -> Item {
        if let Some(why) = why {
            self.enabled = false;
            self.why = Some(why.into());
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
    pub why: Option<String>,
}

impl Submenu {
    pub fn new(label: impl Into<String>, entries: Vec<Entry>) -> Submenu {
        Submenu {
            label: label.into(),
            entries,
            enabled: true,
            why: None,
        }
    }
}

impl From<Submenu> for Entry {
    fn from(submenu: Submenu) -> Entry {
        Entry::Submenu(submenu)
    }
}

/// An item macOS provides, which AppKit carries out.
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

/// The window in front, which the commands act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Front {
    Graph,
    /// The log, a diff, a blame, the compare or the settings window (macOS only: elsewhere the
    /// menu bar is in the main window, and that is in front when it is used).
    Other,
}

/// What the menus show: the state of the app as it matters to them.
#[derive(Clone, Debug)]
pub struct State {
    pub platform: Platform,
    pub front: Front,
    /// A text field has the keyboard in the window in front: Cut, Paste and Select All are its.
    pub text_focus: bool,
    /// A modal dialog locks the windows: nothing but Quit until it is answered.
    pub locked: bool,
    pub has_repo: bool,
    /// The recent folders, the open one left out.
    pub recent: Vec<PathBuf>,
    pub can_undo: bool,
    pub can_redo: bool,
    /// A node is selected, whose hash Copy copies.
    pub selected: bool,
    pub auto_reload: bool,
    pub detail: Simplification,
    /// Each of View › Show: on, and why it can't be, if it can't.
    pub shown: Vec<(Shown, bool, Option<&'static str>)>,
    pub current_branch_only: bool,
    pub first_parent_only: bool,
    pub overview: bool,
    pub status_bar: bool,
    pub remember_moves: bool,
    /// Some node is away from its place in the layout.
    pub displaced: bool,
    pub drag: DragModel,
    pub direction: Direction,
    /// The version of a newer release, while one is out.
    pub newer: Option<String>,
    /// The Git menu's sections, built by `git_menu`.
    pub git: Vec<Entry>,
}

/// `text` as menus on `platform` write it: Title Case on macOS (as Apple's guidelines have it:
/// every word but articles, conjunctions and prepositions of four letters or fewer, and the
/// first and last always), else as given, in sentence case. Words with a capital letter after
/// their first (HEAD, WebP) and parterre's name stay as they are.
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
                // Show/hide: each half.
                word.split('/').map(capital).collect::<Vec<_>>().join("/")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// [`case`] of `template`, its `{}`s then filled with `names` in turn, as they are.
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

/// The access keys of a menu's entries on Windows and Linux (Alt+F for File, then O for Open):
/// for each label, the index of its underlined character, unique in the menu ignoring case.
/// Chosen as KDE chooses them for menus that name none: the first letters of words first, in
/// order, then any other letter or digit of what is still without one. `None` for an entry
/// that isn't chosen (separators, headings) and for a label with no free letter left.
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

/// The letter of `label`'s access key `key`, as typed: lower case.
pub fn access_letter(label: &str, key: Option<usize>) -> Option<String> {
    let c = label.chars().nth(key?)?;
    Some(c.to_lowercase().collect())
}

/// The menus of the bar, as `state` has them.
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
        graph_only(state, state.git.clone()),
    ));
    menus.push(titled(Kind::Layout, "Layout", layout_menu(state)));
    if mac {
        menus.push(titled(Kind::Window, "Window", window_menu()));
    }
    let mut help = titled(Kind::Help, "Help", help_menu(state));
    help.accent = !mac && state.newer.is_some();
    menus.push(help);
    if state.locked {
        for menu in &mut menus {
            lock(&mut menu.entries);
        }
    }
    menus
}

/// Greys out every item but Quit, while a modal dialog waits for its answer.
fn lock(entries: &mut [Entry]) {
    for entry in entries {
        match entry {
            Entry::Item(item) if !matches!(item.command, Command::Quit) => item.enabled = false,
            Entry::Submenu(submenu) => lock(&mut submenu.entries),
            _ => {}
        }
    }
}

/// While another window is in front, what only the graph does is greyed out.
fn graph_only(state: &State, mut entries: Vec<Entry>) -> Vec<Entry> {
    if state.front == Front::Graph {
        return entries;
    }
    // Fetch is every window's: it fetches from the window in front.
    fn grey(entries: &mut [Entry]) {
        for entry in entries {
            match entry {
                Entry::Item(item) if !matches!(item.command, Command::Fetch) => {
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

/// *Download ‹version›*, in the accent colour, while a newer release is out (#258).
fn download(state: &State) -> Option<Entry> {
    let version = state.newer.as_ref()?;
    let mut item = Item::new(format!("Download {version}"), Command::Download);
    item.accent = true;
    Some(item.into())
}

fn file_menu(state: &State) -> Vec<Entry> {
    let p = state.platform;
    let mac = p == Platform::Mac;
    let graph = state.front == Front::Graph;
    let mut recent: Vec<Entry> = state
        .recent
        .iter()
        .map(|path| {
            let (name, place) = super::name_and_place(path);
            let mut item = if mac {
                // Names alone, with the folder above where two are the same.
                let clash = state
                    .recent
                    .iter()
                    .filter(|other| super::name_and_place(other).0 == name)
                    .count()
                    > 1;
                let label = if clash {
                    let above = std::path::Path::new(&place)
                        .file_name()
                        .map_or(place.clone(), |n| n.to_string_lossy().into_owned());
                    format!("{name} — {above}")
                } else {
                    name
                };
                let mut item = Item::new(label, Command::OpenRecent(path.clone()));
                item.icon = Some(path.clone());
                item
            } else {
                Item::new(name, Command::OpenRecent(path.clone()))
            };
            item.tip = Some(path.display().to_string());
            if !mac {
                // Where the shortcut would be: the folder it is in, telling same names apart.
                item.detail = Some(place);
            }
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
    // In a text field, its own undo, as the keys would be without the menu bar.
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
    // Copy is the selected commit's hash, or what is chosen in a text field or another window.
    let copy = item(p, "Copy", Command::Copy)
        .key(keys::COPY)
        .enabled(state.text_focus || !graph || state.selected);
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
    // In another window, the zoom items set its text size.
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
    // Apple's way: the item says what it does. Elsewhere a check mark.
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
            .enabled(state.displaced)
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
            selected: true,
            auto_reload: true,
            detail: Simplification::Decorated,
            shown: Shown::ALL.iter().map(|&s| (s, true, None)).collect(),
            current_branch_only: false,
            first_parent_only: false,
            overview: false,
            status_bar: true,
            remember_moves: false,
            displaced: false,
            drag: DragModel::Adapt,
            direction: Direction::NewestTop,
            newer: None,
            git: vec![
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
        // Nothing free: none.
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
        s.newer = Some("0.8.0".into());
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
        let Entry::Submenu(recent) = &find(&mac, Kind::File).entries[1] else {
            panic!("no Open Recent");
        };
        assert_eq!(
            labels(&recent.entries),
            ["app — a", "app — b", "|", "Clear Menu"]
        );
        let linux = build(&state(Platform::Linux));
        let Entry::Submenu(recent) = &find(&linux, Kind::File).entries[1] else {
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
        // The window's own: reload and text size.
        assert!(item_labelled(view, "Reload").enabled);
        assert!(item_labelled(view, "Zoom In").enabled);
        assert!(!item_labelled(view, "Zoom to Fit").enabled);
        assert!(!item_labelled(view, "Go to HEAD").enabled);
        let git = &find(&menus, Kind::Git).entries;
        assert!(!item_labelled(git, "Show log").enabled);
        // Every window fetches; a submenu's items grey with it.
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
