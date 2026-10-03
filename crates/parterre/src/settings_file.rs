//! Settings as versioned JSON documents: stored between runs, and exported to share them with
//! other computers or a team. Each is read one setting at a time (see
//! [`parterre_core::lenient`]), so files written by an older or a newer parterre load all
//! they can.
//!
//! A document is an object with
//! - `version`: [`VERSION`], the format's. It goes up when a setting changes its meaning, and
//!   reading converts what an older version wrote. Settings added or removed leave it alone:
//!   missing ones get their defaults, unknown ones are left out.
//! - `parterre`: the version of parterre that wrote it, for people reading it.
//! - `settings`: parterre's own settings (exported without what belongs to the computer:
//!   window sizes and dividers).
//! - `repository`: the settings of one repository (in an exported file), or `repositories`:
//!   those of every repository, by its main worktree (stored).

use std::collections::BTreeMap;

use parterre_core::lenient::{self, Skipped, get, get_mut};
use serde_json::{Map, Value, json};

use parterre_core::Repo;

use crate::settings::{RepoSettings, Settings};

/// The format's version.
pub const VERSION: u64 = 1;

/// The storage key of the stored document. (`settings::STORAGE_KEY` keeps the settings in the
/// format parterre used before, for older versions to read.)
pub const STORAGE_KEY: &str = "parterre-settings";

/// Settings that belong to the computer, not exported: window sizes and dividers.
const MACHINE: [&[&str]; 6] = [
    &["log_window", "size"],
    &["log_window", "dividers"],
    &["diff_window", "size"],
    &["compare_window", "size"],
    &["blame_window", "size"],
    &["blame_window", "history_height"],
];

/// The stored settings of every repository, and what the stored document had that this version
/// left out, to write it back.
#[derive(Debug, Default)]
pub struct Stored {
    /// By [`RepoSettings::key`]. A repository without settings of its own has the defaults.
    pub repositories: BTreeMap<String, RepoSettings>,
    file: Value,
    skipped: Vec<Skipped>,
}

impl Stored {
    /// Reads a stored document; `None` if `text` isn't one.
    pub fn read(text: &str) -> Option<(Settings, Stored)> {
        let file: Value = serde_json::from_str(text).ok()?;
        file.get("version")?;
        let mut skipped = Vec::new();
        let settings = part(&file, "settings", &Settings::default(), &mut skipped);
        let mut repositories = BTreeMap::new();
        if let Some(stored) = file.get("repositories").and_then(Value::as_object) {
            for (key, value) in stored {
                let read = lenient::read(&RepoSettings::default(), value);
                let under = |s: Skipped| s.under(key).under("repositories");
                skipped.extend(read.skipped.into_iter().map(under));
                repositories.insert(key.clone(), read.value);
            }
        }
        let stored = Stored {
            repositories,
            file,
            skipped,
        };
        Some((settings, stored))
    }

    /// The document to store, with what a newer version wrote kept.
    pub fn write(&self, settings: &Settings) -> String {
        let repositories: Map<String, Value> = self
            .repositories
            .iter()
            .map(|(key, repo)| (key.clone(), to_value(repo)))
            .collect();
        let mut now = header();
        now["settings"] = to_value(settings);
        now["repositories"] = Value::Object(repositories);
        lenient::write_back(now, &self.file, &self.skipped).to_string()
    }

    /// Forgets every repository's settings, and what the stored document had besides.
    pub fn reset(&mut self) {
        *self = Stored::default();
    }

    /// The settings of `repo`. One without settings of its own has the defaults, or the
    /// `legacy` filters if there are any: they go to the first repository asked for.
    pub fn settings_of(&self, repo: &Repo, legacy: &mut Option<RepoSettings>) -> RepoSettings {
        match self.repositories.get(&RepoSettings::key(repo)) {
            Some(settings) => settings.clone(),
            None => legacy.take().unwrap_or_default(),
        }
    }

    /// Keeps `repo` as the settings of the repository `key`. The defaults aren't kept.
    pub fn keep(&mut self, key: String, repo: RepoSettings) {
        if repo == RepoSettings::default() {
            self.repositories.remove(&key);
        } else {
            self.repositories.insert(key, repo);
        }
    }
}

/// Loads the stored settings: parterre's, every repository's, and the filters an older
/// parterre kept for all repositories, if it was the last to save.
pub fn load(storage: Option<&dyn eframe::Storage>) -> (Settings, Stored, Option<RepoSettings>) {
    let Some(storage) = storage else {
        return Default::default();
    };
    if let Some(read) = storage
        .get_string(STORAGE_KEY)
        .and_then(|s| Stored::read(&s))
    {
        return (read.0, read.1, None);
    }
    match eframe::get_value::<Settings>(storage, crate::settings::STORAGE_KEY) {
        Some(settings) => {
            let legacy = RepoSettings::of(&settings.graph);
            let legacy = (legacy != RepoSettings::default()).then_some(legacy);
            (settings, Stored::default(), legacy)
        }
        None => Default::default(),
    }
}

/// What to export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// parterre's own settings.
    Settings,
    /// The settings of the repository shown.
    Repository,
}

/// The exported file of `part`. `settings` holds the repository settings of the repository
/// shown, as in the app.
pub fn export(part: Part, settings: &Settings) -> String {
    let mut file = header();
    match part {
        Part::Settings => {
            let mut value = to_value(settings);
            for path in local_paths() {
                remove(&mut value, &path);
            }
            file["settings"] = value;
        }
        Part::Repository => file["repository"] = to_value(&RepoSettings::of(&settings.graph)),
    }
    serde_json::to_string_pretty(&file).unwrap_or_default() + "\n"
}

/// What an exported file had, read onto the settings in the app.
#[derive(Debug)]
pub struct Imported {
    /// parterre's settings, with those of the computer and of the repository shown kept.
    pub settings: Option<Settings>,
    /// The settings of a repository.
    pub repository: Option<RepoSettings>,
    /// Settings left out: unknown here, or with values that don't read.
    pub skipped: Vec<String>,
    /// Written by a newer format than this parterre reads.
    pub newer: bool,
}

/// Reads an exported file onto `current`, the settings in the app: settings missing from the
/// file get their defaults.
pub fn import(text: &str, current: &Settings) -> Result<Imported, String> {
    let mut file: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let version = file.get("version").and_then(Value::as_u64);
    let parts = ["settings", "repository"].map(|key| file.get(key).is_some_and(Value::is_object));
    if version.is_none() || parts == [false, false] {
        return Err("not a parterre settings file".into());
    }
    let mut skipped = Vec::new();
    let settings = parts[0].then(|| {
        // What the computer and the repository shown have stays as it is.
        let mut base = to_value(&Settings::default());
        let now = to_value(current);
        for path in local_paths() {
            if let (Some(value), Some(slot)) = (get(&now, &path), get_mut(&mut base, &path)) {
                *slot = value.clone();
            }
            remove(&mut file["settings"], &path);
        }
        let base = serde_json::from_value(base).unwrap_or_else(|_| current.clone());
        part(&file, "settings", &base, &mut skipped)
    });
    let repository =
        parts[1].then(|| part(&file, "repository", &RepoSettings::default(), &mut skipped));
    Ok(Imported {
        settings,
        repository,
        skipped: skipped.iter().map(Skipped::name).collect(),
        newer: version.is_some_and(|v| v > VERSION),
    })
}

/// The paths of the settings that aren't exported as parterre's: the computer's, and the
/// repository settings among the graph options.
fn local_paths() -> Vec<Vec<String>> {
    let repository = to_value(&RepoSettings::default());
    let repository = repository.as_object().into_iter().flat_map(Map::keys);
    MACHINE
        .iter()
        .map(|path| path.iter().map(|&key| key.to_owned()).collect())
        .chain(repository.map(|key| vec!["graph".to_owned(), key.clone()]))
        .collect()
}

/// The part `key` of `file` read onto `base`, adding what was left out to `skipped`.
fn part<T>(file: &Value, key: &str, base: &T, skipped: &mut Vec<Skipped>) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned + Clone,
{
    let read = lenient::read(base, file.get(key).unwrap_or(&Value::Null));
    skipped.extend(read.skipped.into_iter().map(|s| s.under(key)));
    read.value
}

fn header() -> Value {
    json!({"version": VERSION, "parterre": crate::VERSION})
}

fn to_value(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap_or_default()
}

fn remove(value: &mut Value, path: &[String]) {
    let Some((last, parent)) = path.split_last() else {
        return;
    };
    if let Some(parent) = get_mut(value, parent).and_then(Value::as_object_mut) {
        parent.remove(last);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeChoice;

    fn filtered() -> RepoSettings {
        RepoSettings {
            current_branch_only: true,
            first_parent_only: false,
            ref_filter: "main".into(),
            hide_branches: "pipeline/*".into(),
        }
    }

    #[test]
    fn stored_settings_and_repositories_survive_a_round_trip() {
        let mut settings = Settings {
            theme: ThemeChoice::Dark,
            ..Settings::default()
        };
        settings.log_window.size = [500.0, 400.0];
        let mut stored = Stored::default();
        stored.keep("/src/app".into(), filtered());
        stored.keep("/src/lib".into(), RepoSettings::default());
        let text = stored.write(&settings);

        let (back, stored) = Stored::read(&text).unwrap();
        assert_eq!(back, settings);
        // The defaults aren't kept.
        assert_eq!(stored.repositories.len(), 1);
        assert_eq!(stored.repositories["/src/app"], filtered());
        let file: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(file["version"], VERSION);
        assert_eq!(file["parterre"], crate::VERSION);
    }

    #[test]
    fn a_newer_stored_document_loads_what_it_can_and_keeps_the_rest() {
        let text = r#"{
            "version": 2,
            "settings": {"theme": "HighContrast", "text_size": 1.25, "sounds": true},
            "repositories": {"/src/app": {"ref_filter": "main", "hide_tags": "v*"}}
        }"#;
        let (settings, stored) = Stored::read(text).unwrap();
        assert_eq!(settings.theme, ThemeChoice::default());
        assert_eq!(settings.text_size, 1.25);
        assert_eq!(stored.repositories["/src/app"].ref_filter, "main");

        // Saved again here, what this version didn't know is still there for the newer one.
        let out: Value = serde_json::from_str(&stored.write(&settings)).unwrap();
        assert_eq!(out["settings"]["theme"], "HighContrast");
        assert_eq!(out["settings"]["sounds"], true);
        assert_eq!(out["repositories"]["/src/app"]["hide_tags"], "v*");
        assert_eq!(out["repositories"]["/src/app"]["ref_filter"], "main");
    }

    #[test]
    fn reset_forgets_every_repository_and_what_a_newer_version_wrote() {
        let text = r#"{"version": 1, "settings": {"sounds": true},
            "repositories": {"/src/app": {"ref_filter": "main"}}}"#;
        let (_, mut stored) = Stored::read(text).unwrap();
        stored.reset();
        let out: Value = serde_json::from_str(&stored.write(&Settings::default())).unwrap();
        assert_eq!(out["repositories"], json!({}));
        assert!(out["settings"].get("sounds").is_none());
    }

    #[test]
    fn only_documents_are_read_as_stored_settings() {
        assert!(Stored::read("(theme: Dark)").is_none());
        assert!(Stored::read(r#"{"theme": "Dark"}"#).is_none());
    }

    #[derive(Default)]
    struct Memory(std::collections::HashMap<String, String>);

    impl eframe::Storage for Memory {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn upgrading_gives_the_filters_kept_for_every_repository_to_the_first_one_shown() {
        let mut old = Settings {
            theme: ThemeChoice::Dark,
            ..Settings::default()
        };
        filtered().apply(&mut old.graph);
        let mut storage = Memory::default();
        eframe::set_value(&mut storage, crate::settings::STORAGE_KEY, &old);
        let (settings, stored, legacy) = load(Some(&storage));
        assert_eq!(settings.theme, ThemeChoice::Dark);
        assert!(stored.repositories.is_empty());
        assert_eq!(legacy, Some(filtered()));

        // Once stored as a document, that is what is read: the older format is only written
        // for older versions.
        let repo = Repo::new(
            "/src/app".into(),
            Vec::new(),
            Vec::new(),
            parterre_core::Head::Branch {
                name: "main".into(),
                target: None,
            },
        );
        let (mut stored, mut legacy) = (stored, legacy);
        stored.keep(
            RepoSettings::key(&repo),
            stored.settings_of(&repo, &mut legacy),
        );
        assert_eq!(legacy, None);
        eframe::Storage::set_string(&mut storage, STORAGE_KEY, stored.write(&settings));
        let (_, stored, legacy) = load(Some(&storage));
        assert_eq!(legacy, None);
        assert_eq!(stored.settings_of(&repo, &mut None), filtered());

        // Nothing stored, or an automated run: the defaults.
        let (settings, stored, legacy) = load(None);
        assert_eq!(settings, Settings::default());
        assert!(stored.repositories.is_empty() && legacy.is_none());
    }

    #[test]
    fn exported_settings_leave_out_the_computer_and_the_repository() {
        let mut settings = Settings {
            theme: ThemeChoice::Dark,
            ..Settings::default()
        };
        filtered().apply(&mut settings.graph);
        let file: Value = serde_json::from_str(&export(Part::Settings, &settings)).unwrap();
        assert_eq!(file["version"], VERSION);
        assert!(file.get("repository").is_none());
        let exported = &file["settings"];
        assert_eq!(exported["theme"], "Dark");
        assert!(exported["log_window"].get("size").is_none());
        assert!(exported["log_window"].get("dividers").is_none());
        assert!(exported["log_window"].get("layout").is_some());
        assert!(exported["blame_window"].get("history_height").is_none());
        assert!(exported["graph"].get("hide_branches").is_none());
        assert!(exported["graph"].get("show_tags").is_some());

        let file: Value = serde_json::from_str(&export(Part::Repository, &settings)).unwrap();
        assert!(file.get("settings").is_none());
        let repo: RepoSettings = serde_json::from_value(file["repository"].clone()).unwrap();
        assert_eq!(repo, filtered());
    }

    #[test]
    fn importing_replaces_what_the_file_has_and_keeps_the_rest_here() {
        let theirs = Settings {
            theme: ThemeChoice::Dark,
            show_overview: true,
            ..Settings::default()
        };
        let file = export(Part::Settings, &theirs);

        let mut mine = Settings {
            arrows: crate::settings::Arrows::None,
            ..Settings::default()
        };
        mine.log_window.size = [500.0, 400.0];
        filtered().apply(&mut mine.graph);
        let imported = import(&file, &mine).unwrap();
        let settings = imported.settings.unwrap();
        assert!(imported.repository.is_none());
        assert!(imported.skipped.is_empty());
        assert!(!imported.newer);
        // Theirs.
        assert_eq!(settings.theme, ThemeChoice::Dark);
        assert!(settings.show_overview);
        assert_eq!(settings.arrows, crate::settings::Arrows::default());
        // Mine: this computer's, and the repository shown's.
        assert_eq!(settings.log_window.size, [500.0, 400.0]);
        assert_eq!(RepoSettings::of(&settings.graph), filtered());
    }

    #[test]
    fn importing_an_older_or_newer_file_takes_what_it_can() {
        // Older: before a setting existed. Newer: a theme this version doesn't know, a
        // setting it doesn't have, and window sizes, which aren't taken.
        let text = r#"{
            "version": 7,
            "settings": {
                "theme": "HighContrast",
                "show_overview": true,
                "sounds": true,
                "log_window": {"size": [1.0, 1.0], "layout": "FilesRight"}
            },
            "repository": {"hide_branches": "pipeline/*", "first_parent_only": "yes"}
        }"#;
        let mine = Settings::default();
        let imported = import(text, &mine).unwrap();
        let settings = imported.settings.unwrap();
        assert_eq!(settings.theme, ThemeChoice::default());
        assert!(settings.show_overview);
        assert_eq!(settings.log_window.size, mine.log_window.size);
        assert_eq!(
            settings.log_window.layout,
            parterre_core::log_layout::LogLayout::FilesRight
        );
        let repo = imported.repository.unwrap();
        assert_eq!(repo.hide_branches, "pipeline/*");
        assert!(!repo.first_parent_only);
        assert!(imported.newer);
        assert_eq!(
            imported.skipped,
            [
                "settings.sounds",
                "settings.theme",
                "repository.first_parent_only"
            ]
        );
    }

    #[test]
    fn other_files_are_not_imported() {
        let mine = Settings::default();
        assert!(import("(theme: Dark)", &mine).is_err());
        assert!(import(r#"{"theme": "Dark"}"#, &mine).is_err());
        assert!(import(r#"{"version": 1}"#, &mine).is_err());
        assert!(import(r#"{"version": 1, "settings": 3}"#, &mine).is_err());
    }

    #[test]
    fn repository_settings_are_the_graph_options_of_the_same_names() {
        // Exports leave these out of parterre's settings by name.
        let repo = to_value(&RepoSettings::default());
        let graph = to_value(&parterre_core::revgraph::GraphOptions::default());
        for key in repo.as_object().unwrap().keys() {
            assert!(graph.get(key).is_some(), "{key}");
        }
        let mut graph = parterre_core::revgraph::GraphOptions::default();
        filtered().apply(&mut graph);
        assert_eq!(RepoSettings::of(&graph), filtered());
    }
}
