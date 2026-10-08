//! Feature events (#264): which windows, dialogs, menus and actions are used, and the settings
//! every event carries, named as PostHog's naming guide says (lowercase snake case,
//! `object_action`, a fixed event name with the variable part as a property).
//!
//! Every string a property can hold is a name from the fixed sets below, turned into a
//! `&'static str` by a `match`; the other values are numbers and booleans. So no repository
//! data (paths, names, refs, hashes) can reach an event, by construction (#223). No network
//! here; the sending is in `posthog.rs`.

use std::cell::RefCell;
use std::sync::Mutex;

/// An enum of names, with each variant's name and the whole set (`ALL`), for the tests.
macro_rules! names {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident => $text:literal,)* }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$vmeta])* $variant,)*
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant,)*];

            /// As the property says it.
            pub fn name(self) -> &'static str {
                match self {
                    $($name::$variant => $text,)*
                }
            }
        }
    };
}

names! {
    /// A window or dialog, as `$screen`'s `$screen_name`.
    Screen {
        Log => "log",
        Compare => "compare",
        Diff => "diff",
        Blame => "blame",
        Settings => "settings",
        About => "about",
        Shortcuts => "shortcuts",
        Legend => "legend",
        CreateBranch => "create_branch",
        AddWorktree => "add_worktree",
        /// The question before losing work.
        LostWork => "lost_work",
        Reset => "reset",
        Rebase => "rebase",
        Merge => "merge",
        CherryPick => "cherry_pick",
        Revert => "revert",
        RestoreStash => "restore_stash",
        /// The picker of merge tools, when none is usable.
        MergeTool => "merge_tool",
        /// What a git operation printed.
        OperationDetails => "operation_details",
        /// A fetch, pull or push running, with git's output.
        Network => "network",
        /// How to pull a branch that has diverged.
        PullDiverged => "pull_diverged",
        /// The question before a force push.
        ForcePush => "force_push",
        SetUpstream => "set_upstream",
        /// The question before deleting remote branches.
        DeleteRemoteBranch => "delete_remote_branch",
        /// The question before opening many diff windows at once.
        OpenDiffs => "open_diffs",
        PullRequestsError => "pull_requests_error",
        ImportSettings => "import_settings",
        ResetSettings => "reset_settings",
        /// The dialog telling of a newer release, once per version.
        NewRelease => "new_release",
        Clone => "clone",
    }
}

names! {
    /// A menu or toolbar popover, as `menu_view`'s `menu`.
    Menu {
        /// The menu bar's: the application menu (macOS), File, Edit, View, Git, Layout,
        /// Window (macOS) and Help.
        AppMenu => "app_menu",
        FileMenu => "file_menu",
        EditMenu => "edit_menu",
        ViewMenu => "view_menu",
        GitMenu => "git_menu",
        LayoutMenu => "layout_menu",
        WindowMenu => "window_menu",
        HelpMenu => "help_menu",
        Filter => "filter",
        Zoom => "zoom",
        Drag => "drag",
        /// The graph's, on a node.
        Node => "node",
        /// The graph's, beside the nodes.
        Canvas => "canvas",
        /// A commit row's, in the log and the dialogs listing commits.
        Commit => "commit",
        /// A changed file's.
        File => "file",
        /// A blame window's line or history row.
        Blame => "blame",
    }
}

names! {
    /// Something the user starts, as `action_run`'s `action`.
    Action {
        CreateBranch => "create_branch",
        SwitchBranch => "switch_branch",
        SwitchDetached => "switch_detached",
        DeleteBranch => "delete_branch",
        AddWorktree => "add_worktree",
        DeleteWorktree => "delete_worktree",
        GoToWorktree => "go_to_worktree",
        Reset => "reset",
        Rebase => "rebase",
        Merge => "merge",
        CherryPick => "cherry_pick",
        Revert => "revert",
        RestoreStash => "restore_stash",
        /// Keep, delete or take a side of a conflicted file.
        ResolveConflict => "resolve_conflict",
        OpenMergeTool => "open_merge_tool",
        Fetch => "fetch",
        Pull => "pull",
        Push => "push",
        SetUpstream => "set_upstream",
        DeleteRemoteBranch => "delete_remote_branch",
        Clone => "clone",
        OpenRepository => "open_repository",
        CloseRepository => "close_repository",
        Reload => "reload",
        Undo => "undo",
        Redo => "redo",
        Export => "export",
        Fit => "fit",
        GoToHead => "go_to_head",
        ResetPositions => "reset_positions",
        ReturnToLayout => "return_to_layout",
        SelectSubtree => "select_subtree",
        DragNode => "drag_node",
        MarkForComparison => "mark_for_comparison",
        OpenPullRequest => "open_pull_request",
        OpenFileManager => "open_file_manager",
        OpenTerminal => "open_terminal",
        Copy => "copy",
        Find => "find",
        ExportSettings => "export_settings",
        ImportSettings => "import_settings",
        ResetSettings => "reset_settings",
        DownloadUpdate => "download_update",
    }
}

names! {
    /// `theme`.
    Theme {
        System => "system",
        Light => "light",
        Dark => "dark",
    }
}

names! {
    /// `graph_mode`: which commits the graph keeps as nodes.
    GraphMode {
        LabelledCommits => "labelled_commits",
        LabelledForks => "labelled_forks",
        BranchingsAndMerges => "branchings_and_merges",
        AllCommits => "all_commits",
    }
}

names! {
    /// `graph_direction`: where the newest commits are.
    GraphDirection {
        NewestTop => "newest_top",
        NewestBottom => "newest_bottom",
        NewestLeft => "newest_left",
        NewestRight => "newest_right",
    }
}

names! {
    /// `log_layout`.
    LogLayout {
        Stacked => "stacked",
        SideBySide => "side_by_side",
        DetailsBelow => "details_below",
        FilesRight => "files_right",
    }
}

names! {
    /// `drag_model`: what moves with a dragged node.
    DragModel {
        Adapt => "adapt",
        Free => "free",
        Subtree => "subtree",
    }
}

names! {
    /// `diff_form`.
    DiffForm {
        SideBySide => "side_by_side",
        Unified => "unified",
    }
}

names! {
    /// A count in powers of ten, for `commit_count_range` and `node_count_range`.
    Range {
        Zero => "0",
        Ones => "1-9",
        Tens => "10-99",
        Hundreds => "100-999",
        Thousands => "1000-9999",
        TensOfThousands => "10000-99999",
        HundredsOfThousands => "100000-999999",
        Millions => "1000000+",
    }
}

impl Range {
    /// The range `count` is in.
    pub fn of(count: usize) -> Range {
        match count {
            0 => Range::Zero,
            1..=9 => Range::Ones,
            10..=99 => Range::Tens,
            100..=999 => Range::Hundreds,
            1_000..=9_999 => Range::Thousands,
            10_000..=99_999 => Range::TensOfThousands,
            100_000..=999_999 => Range::HundredsOfThousands,
            _ => Range::Millions,
        }
    }
}

/// The most repositories `repository_count` says: the recent list keeps no more.
pub const MAX_REPOSITORIES: usize = 10;

/// A feature used: one of three events, with which feature as its property.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feature {
    /// A window or dialog opens: `$screen`, as PostHog's mobile SDKs capture screen views.
    Screen(Screen),
    /// A menu or popover opens: `menu_view`.
    Menu(Menu),
    /// The user starts something: `action_run`.
    Action(Action),
}

impl Feature {
    /// The event's name.
    pub fn event(self) -> &'static str {
        match self {
            Feature::Screen(_) => "$screen",
            Feature::Menu(_) => "menu_view",
            Feature::Action(_) => "action_run",
        }
    }

    /// The property naming the feature, and its value.
    pub fn property(self) -> (&'static str, &'static str) {
        match self {
            Feature::Screen(s) => ("$screen_name", s.name()),
            Feature::Menu(m) => ("menu", m.name()),
            Feature::Action(a) => ("action", a.name()),
        }
    }
}

/// What every event says of the settings, the screen and the repository: PostHog's super
/// properties, as its SDKs' `register` adds them, at their values when the event is sent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Properties {
    pub theme: Theme,
    /// The size of all text, one of the text size steps.
    pub text_size: f32,
    pub graph_mode: GraphMode,
    pub graph_direction: GraphDirection,
    pub log_layout: LogLayout,
    pub drag_model: DragModel,
    pub diff_form: DiffForm,
    pub auto_reload: bool,
    pub pull_requests: bool,
    /// The display's scale (physical pixels per point), if known.
    pub screen_density: Option<f32>,
    /// How many repositories the recent list holds.
    pub repositories: usize,
    /// The open repository's commits, and the graph's nodes, while one is open.
    pub commits: Option<usize>,
    pub nodes: Option<usize>,
}

/// A property's value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    /// A name from the fixed sets above.
    Name(&'static str),
    Number(f64),
    Count(usize),
    Flag(bool),
}

impl Properties {
    /// The properties as sent: names, and what is unknown left out.
    pub fn pairs(&self) -> Vec<(&'static str, Value)> {
        let mut pairs = vec![
            ("theme", Value::Name(self.theme.name())),
            ("text_size", Value::Number(rounded(self.text_size))),
            ("graph_mode", Value::Name(self.graph_mode.name())),
            ("graph_direction", Value::Name(self.graph_direction.name())),
            ("log_layout", Value::Name(self.log_layout.name())),
            ("drag_model", Value::Name(self.drag_model.name())),
            ("diff_form", Value::Name(self.diff_form.name())),
            ("is_auto_reload_on", Value::Flag(self.auto_reload)),
            ("is_pull_requests_on", Value::Flag(self.pull_requests)),
            (
                "repository_count",
                Value::Count(self.repositories.min(MAX_REPOSITORIES)),
            ),
        ];
        if let Some(density) = self.screen_density.filter(|d| d.is_finite() && *d > 0.0) {
            pairs.push(("$screen_density", Value::Number(rounded(density))));
        }
        if let Some(commits) = self.commits {
            pairs.push(("commit_count_range", Value::Name(Range::of(commits).name())));
        }
        if let Some(nodes) = self.nodes {
            pairs.push(("node_count_range", Value::Name(Range::of(nodes).name())));
        }
        pairs
    }
}

/// To two decimals: `1.25`, not `1.2500000476837158`.
fn rounded(x: f32) -> f64 {
    (f64::from(x) * 100.0).round() / 100.0
}

/// The usage statistics' thread, while they are sent: given every feature recorded.
type Sink = Box<dyn Fn(Feature) + Send>;

static SINK: Mutex<Option<Sink>> = Mutex::new(None);

/// The properties registered last, for every event.
static REGISTERED: Mutex<Option<Properties>> = Mutex::new(None);

thread_local! {
    /// What [`recording`] has recorded on this thread.
    static RECORDED: RefCell<Option<Vec<Feature>>> = const { RefCell::new(None) };
}

/// Records that a feature was used. Sent while usage statistics are ([`crate::Usage`]), and
/// nothing otherwise. Never waits: it is handed to the usage statistics' thread.
pub fn record(feature: Feature) {
    let tested = RECORDED.with(|recorded| {
        let mut recorded = recorded.borrow_mut();
        recorded.as_mut().map(|r| r.push(feature)).is_some()
    });
    if tested {
        return;
    }
    if let Ok(sink) = SINK.lock()
        && let Some(sink) = sink.as_ref()
    {
        sink(feature);
    }
}

/// The settings, screen and repository as they are now, for every event from now on.
pub fn register(properties: Properties) {
    if let Ok(mut registered) = REGISTERED.lock() {
        *registered = Some(properties);
    }
}

/// The properties registered last.
#[cfg(feature = "send")]
pub(crate) fn registered() -> Option<Properties> {
    REGISTERED.lock().ok().and_then(|r| *r)
}

/// While usage statistics are sent: features go to `sink`. `None` stops it.
pub(crate) fn sink(sink: Option<Sink>) {
    if let Ok(mut current) = SINK.lock() {
        *current = sink;
    }
}

/// For tests: runs `f`, and gives what it recorded on this thread besides its result. What is
/// recorded meanwhile on this thread is kept here and never sent.
pub fn recording<R>(f: impl FnOnce() -> R) -> (R, Vec<Feature>) {
    let outer = RECORDED.with(|r| r.replace(Some(Vec::new())));
    let result = f();
    let recorded = RECORDED.with(|r| r.replace(outer)).unwrap_or_default();
    (result, recorded)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every feature there is.
    fn features() -> Vec<Feature> {
        let screens = Screen::ALL.iter().map(|&s| Feature::Screen(s));
        let menus = Menu::ALL.iter().map(|&m| Feature::Menu(m));
        let actions = Action::ALL.iter().map(|&a| Feature::Action(a));
        screens.chain(menus).chain(actions).collect()
    }

    /// Names as PostHog's guide has them: lowercase snake case, no spaces, and nothing that
    /// could be a path, a ref or a hash.
    fn is_plain_name(name: &str) -> bool {
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    }

    fn assert_unique_plain_names(names: &[&'static str]) {
        let unique: HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "{names:?}");
        for name in names {
            assert!(is_plain_name(name), "{name:?}");
        }
    }

    #[test]
    fn three_event_names_each_with_a_fixed_property() {
        let mut events = HashSet::new();
        let mut properties = HashSet::new();
        for feature in features() {
            events.insert(feature.event());
            properties.insert(feature.property().0);
        }
        let mut events: Vec<_> = events.into_iter().collect();
        events.sort_unstable();
        assert_eq!(events, ["$screen", "action_run", "menu_view"]);
        let mut properties: Vec<_> = properties.into_iter().collect();
        properties.sort_unstable();
        assert_eq!(properties, ["$screen_name", "action", "menu"]);
    }

    #[test]
    fn every_feature_name_is_a_plain_snake_case_name() {
        assert_unique_plain_names(&Screen::ALL.iter().map(|s| s.name()).collect::<Vec<_>>());
        assert_unique_plain_names(&Menu::ALL.iter().map(|m| m.name()).collect::<Vec<_>>());
        assert_unique_plain_names(&Action::ALL.iter().map(|a| a.name()).collect::<Vec<_>>());
        assert_unique_plain_names(&Theme::ALL.iter().map(|x| x.name()).collect::<Vec<_>>());
        assert_unique_plain_names(&GraphMode::ALL.iter().map(|x| x.name()).collect::<Vec<_>>());
        let directions: Vec<_> = GraphDirection::ALL.iter().map(|x| x.name()).collect();
        assert_unique_plain_names(&directions);
        assert_unique_plain_names(&LogLayout::ALL.iter().map(|x| x.name()).collect::<Vec<_>>());
        assert_unique_plain_names(&DragModel::ALL.iter().map(|x| x.name()).collect::<Vec<_>>());
        assert_unique_plain_names(&DiffForm::ALL.iter().map(|x| x.name()).collect::<Vec<_>>());
    }

    #[test]
    fn counts_go_in_powers_of_ten() {
        let cases = [
            (0, "0"),
            (1, "1-9"),
            (9, "1-9"),
            (10, "10-99"),
            (999, "100-999"),
            (1_000, "1000-9999"),
            (54_321, "10000-99999"),
            (100_000, "100000-999999"),
            (1_000_000, "1000000+"),
            (usize::MAX, "1000000+"),
        ];
        for (count, range) in cases {
            assert_eq!(Range::of(count).name(), range, "{count}");
        }
        // Every range is reached, and the ranges are in order.
        let reached: Vec<_> = [0, 1, 10, 100, 1_000, 10_000, 100_000, 1_000_000]
            .map(Range::of)
            .to_vec();
        assert_eq!(reached, Range::ALL);
    }

    pub(crate) fn properties() -> Properties {
        Properties {
            theme: Theme::Dark,
            text_size: 1.25,
            graph_mode: GraphMode::BranchingsAndMerges,
            graph_direction: GraphDirection::NewestTop,
            log_layout: LogLayout::SideBySide,
            drag_model: DragModel::Adapt,
            diff_form: DiffForm::Unified,
            auto_reload: true,
            pull_requests: false,
            screen_density: Some(1.5),
            repositories: 3,
            commits: Some(12_345),
            nodes: Some(321),
        }
    }

    #[test]
    fn the_properties_are_names_numbers_and_flags() {
        let pairs = properties().pairs();
        let keys: Vec<_> = pairs.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            [
                "theme",
                "text_size",
                "graph_mode",
                "graph_direction",
                "log_layout",
                "drag_model",
                "diff_form",
                "is_auto_reload_on",
                "is_pull_requests_on",
                "repository_count",
                "$screen_density",
                "commit_count_range",
                "node_count_range",
            ]
        );
        let value = |key| pairs.iter().find(|(k, _)| *k == key).unwrap().1;
        assert_eq!(value("theme"), Value::Name("dark"));
        assert_eq!(value("text_size"), Value::Number(1.25));
        assert_eq!(value("is_auto_reload_on"), Value::Flag(true));
        assert_eq!(value("repository_count"), Value::Count(3));
        assert_eq!(value("$screen_density"), Value::Number(1.5));
        assert_eq!(value("commit_count_range"), Value::Name("10000-99999"));
        assert_eq!(value("node_count_range"), Value::Name("100-999"));
    }

    #[test]
    fn what_is_unknown_is_left_out_and_counts_stay_coarse() {
        let p = Properties {
            screen_density: Some(f32::NAN),
            repositories: 250,
            commits: None,
            nodes: None,
            ..properties()
        };
        let pairs = p.pairs();
        assert!(pairs.iter().all(|(k, _)| !k.ends_with("_range")));
        assert!(pairs.iter().all(|(k, _)| *k != "$screen_density"));
        let count = pairs
            .iter()
            .find(|(k, _)| *k == "repository_count")
            .unwrap();
        assert_eq!(count.1, Value::Count(MAX_REPOSITORIES));
    }

    #[test]
    fn a_test_sink_keeps_what_is_recorded_on_its_thread() {
        let fit = Feature::Action(Action::Fit);
        let main = Feature::Menu(Menu::FileMenu);
        let (inner, outer) = recording(|| {
            record(fit);
            let ((), inner) = recording(|| record(main));
            // Another thread's test sink is its own.
            let other = std::thread::spawn(move || recording(|| record(main)).1);
            assert_eq!(other.join().unwrap(), [main]);
            inner
        });
        assert_eq!(inner, [main]);
        assert_eq!(outer, [fit]);
    }
}
