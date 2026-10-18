//! Settings as versioned JSON documents, stored between runs and exported to share. Each is
//! read one setting at a time (see [`parterre_core::lenient`]), so files written by an older or
//! a newer parterre load all they can.
//!
//! A document is an object with
//! - `version`: [`FORMAT_VERSION`]. It goes up when a setting changes its meaning, and reading
//!   converts what an older version wrote. Settings added or removed leave it alone: missing
//!   ones get their defaults, unknown ones are left out.
//!
//!   An older parterre saving the stored document keeps a newer version's number, and writes
//!   back the settings it couldn't read; those it could, it writes as it understood them (#369).
//!   So a setting whose meaning changes gets a new name, or a value the older version can't
//!   read, rather than being converted from what it was: under the newer number, it isn't
//!   converted again.
//! - `parterre`: the version of parterre that wrote it, for people reading it.
//! - `settings`: parterre's own settings, exported without the computer's own.
//! - `repository`: the settings of the repository shown (exported), or `repositories`: those of
//!   every repository, by [`RepoSettings::key`] (stored).

use std::collections::BTreeMap;

use parterre_core::lenient::{self, Skipped, get, get_mut};
use serde_json::{Map, Value, json};

use parterre_core::Repo;

use crate::settings::{RepoSettings, Settings};

pub const FORMAT_VERSION: u64 = 1;

pub const STORAGE_KEY: &str = "parterre-settings";

const MACHINE_SETTINGS: [&[&str]; 7] = [
    &["log_window", "size"],
    &["log_window", "dividers"],
    &["diff_window", "size"],
    &["compare_window", "size"],
    &["blame_window", "size"],
    &["blame_window", "history_height"],
    &["settings_window", "size"],
];

/// The stored settings of every repository, and what the stored document had that this version
/// left out, to write it back.
#[derive(Debug, Default)]
pub struct Stored {
    /// By [`RepoSettings::key`]. A repository without settings of its own has the defaults.
    pub repositories: BTreeMap<String, RepoSettings>,
    document: Value,
    skipped: Vec<Skipped>,
}

impl Stored {
    /// `None` if `text` isn't a stored document.
    pub fn read(text: &str) -> Option<(Settings, Stored)> {
        let document: Value = serde_json::from_str(text).ok()?;
        document.get("version")?;
        let mut skipped = Vec::new();
        let settings = read_part(&document, "settings", &Settings::default(), &mut skipped);
        let mut repositories = BTreeMap::new();
        if let Some(stored) = document.get("repositories").and_then(Value::as_object) {
            for (key, value) in stored {
                let read = lenient::read(&RepoSettings::default(), value);
                let under = |s: Skipped| s.under(key).under("repositories");
                skipped.extend(read.skipped.into_iter().map(under));
                repositories.insert(key.clone(), read.value);
            }
        }
        let stored = Stored {
            repositories,
            document,
            skipped,
        };
        Some((settings, stored))
    }

    /// The document to store, keeping what a newer version wrote.
    pub fn write(&self, settings: &Settings) -> String {
        let repositories: Map<String, Value> = self
            .repositories
            .iter()
            .map(|(key, repo)| (key.clone(), to_value(repo)))
            .collect();
        let mut now = header();
        let version = self.document.get("version").and_then(Value::as_u64);
        if let Some(newer) = version.filter(|&v| v > FORMAT_VERSION) {
            now["version"] = newer.into();
        }
        now["settings"] = to_value(settings);
        now["repositories"] = Value::Object(repositories);
        lenient::write_back(now, &self.document, &self.skipped).to_string()
    }

    /// Forgets every repository's settings, and what the stored document had besides.
    pub fn reset(&mut self) {
        *self = Stored::default();
    }

    pub fn settings_of(&self, repo: &Repo) -> RepoSettings {
        let key = RepoSettings::key(repo);
        self.repositories.get(&key).cloned().unwrap_or_default()
    }

    /// The defaults aren't kept.
    pub fn keep(&mut self, key: String, repo: RepoSettings) {
        if repo == RepoSettings::default() {
            self.repositories.remove(&key);
        } else {
            self.repositories.insert(key, repo);
        }
    }
}

#[derive(Debug, Default)]
pub struct Loaded {
    pub settings: Settings,
    pub stored: Stored,
    /// The stored settings didn't read: these are the defaults.
    pub corrupt: bool,
}

/// The defaults without stored settings.
pub fn load(storage: Option<&dyn eframe::Storage>) -> Loaded {
    let Some(text) = storage.and_then(|storage| storage.get_string(STORAGE_KEY)) else {
        return Loaded::default();
    };
    match Stored::read(&text) {
        Some((settings, stored)) => Loaded {
            settings,
            stored,
            corrupt: false,
        },
        None => Loaded {
            corrupt: true,
            ..Loaded::default()
        },
    }
}

/// The exported file: parterre's settings and, with `with_repository`, the settings of the
/// repository shown, which `settings` holds as in the app.
pub fn export(settings: &Settings, with_repository: bool) -> String {
    let mut file = header();
    let mut value = to_value(settings);
    for path in local_paths() {
        remove_path(&mut value, &path);
    }
    file["settings"] = value;
    if with_repository {
        file["repository"] = to_value(&RepoSettings::of(&settings.graph));
    }
    serde_json::to_string_pretty(&file).unwrap_or_default() + "\n"
}

/// What an exported file had, read onto the settings in the app.
#[derive(Debug)]
pub struct Imported {
    /// parterre's settings, with those of the computer and of the repository shown kept.
    pub settings: Option<Settings>,
    pub repository: Option<RepoSettings>,
    /// Settings left out: unknown here, or with values that don't read. Named from the top of
    /// the file, `settings.theme` or `repository.ref_filter`.
    pub skipped: Vec<String>,
    /// Written in a newer format than this parterre reads.
    pub newer: bool,
}

/// Reads an exported file onto `current`, the settings in the app: settings missing from the
/// file get their defaults.
pub fn import(text: &str, current: &Settings) -> Result<Imported, String> {
    let mut file: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let Some(version) = file.get("version").and_then(Value::as_u64) else {
        return Err("not a parterre settings file".into());
    };
    let has_part = |key: &str| file.get(key).is_some_and(Value::is_object);
    let (has_settings, has_repository) = (has_part("settings"), has_part("repository"));
    if !has_settings && !has_repository {
        return Err("not a parterre settings file".into());
    }
    let mut skipped = Vec::new();
    let settings = has_settings.then(|| {
        let mut base = to_value(&Settings::default());
        let now = to_value(current);
        for path in local_paths() {
            if let (Some(value), Some(slot)) = (get(&now, &path), get_mut(&mut base, &path)) {
                *slot = value.clone();
            }
            remove_path(&mut file["settings"], &path);
        }
        let base = serde_json::from_value(base).unwrap_or_else(|_| current.clone());
        read_part(&file, "settings", &base, &mut skipped)
    });
    let repository = has_repository
        .then(|| read_part(&file, "repository", &RepoSettings::default(), &mut skipped));
    Ok(Imported {
        settings,
        repository,
        skipped: skipped.iter().map(Skipped::name).collect(),
        newer: version > FORMAT_VERSION,
    })
}

/// The settings not exported as parterre's: the computer's, and the repository settings among
/// the graph options.
fn local_paths() -> Vec<Vec<String>> {
    let repository = to_value(&RepoSettings::default());
    let repository = repository.as_object().into_iter().flat_map(Map::keys);
    MACHINE_SETTINGS
        .iter()
        .map(|path| path.iter().map(|&key| key.to_owned()).collect())
        .chain(repository.map(|key| vec!["graph".to_owned(), key.clone()]))
        .collect()
}

/// The part `key` of `file` read onto `base`, adding what was left out to `skipped`.
fn read_part<T>(file: &Value, key: &str, base: &T, skipped: &mut Vec<Skipped>) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned + Clone,
{
    let read = lenient::read(base, file.get(key).unwrap_or(&Value::Null));
    skipped.extend(read.skipped.into_iter().map(|s| s.under(key)));
    read.value
}

fn header() -> Value {
    json!({"version": FORMAT_VERSION, "parterre": crate::version()})
}

fn to_value(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap_or_default()
}

fn remove_path(value: &mut Value, path: &[String]) {
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
        assert_eq!(stored.repositories.len(), 1, "the defaults aren't kept");
        assert_eq!(stored.repositories["/src/app"], filtered());
        let file: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(file["version"], FORMAT_VERSION);
        assert_eq!(file["parterre"], crate::version());
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
        // Under its own version, or the newer one would convert it again (#369).
        assert_eq!(out["version"], 2);
        assert_eq!(out["parterre"], crate::version());
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
    fn loads_the_stored_document_or_the_defaults() {
        let settings = Settings {
            theme: ThemeChoice::Dark,
            ..Settings::default()
        };
        let mut stored = Stored::default();
        let repo = Repo::new(
            "/src/app".into(),
            Vec::new(),
            Vec::new(),
            parterre_core::Head::Branch {
                name: "main".into(),
                target: None,
            },
        );
        stored.keep(RepoSettings::key(&repo), filtered());
        let mut storage = Memory::default();
        eframe::Storage::set_string(&mut storage, STORAGE_KEY, stored.write(&settings));
        let Loaded {
            settings: back,
            stored,
            corrupt,
        } = load(Some(&storage));
        assert_eq!(back, settings);
        assert_eq!(stored.settings_of(&repo), filtered());
        assert!(!corrupt);

        // Nothing stored, or an automated run: the defaults.
        for storage in [Some(&Memory::default() as &dyn eframe::Storage), None] {
            let Loaded {
                settings: back,
                stored,
                corrupt,
            } = load(storage);
            assert_eq!(back, Settings::default());
            assert!(stored.repositories.is_empty());
            assert!(!corrupt);
        }
    }

    #[test]
    fn corrupt_settings_load_as_the_defaults_and_say_so() {
        for text in [
            "{\"version\": 1, \"settings\": {\"theme\": \"Da",
            "",
            "[1, 2]",
            "{}",
        ] {
            let mut storage = Memory::default();
            eframe::Storage::set_string(&mut storage, STORAGE_KEY, text.into());
            let Loaded {
                settings,
                stored,
                corrupt,
            } = load(Some(&storage));
            assert_eq!(settings, Settings::default(), "{text}");
            assert!(stored.repositories.is_empty());
            assert!(corrupt, "{text}");
        }
    }

    #[test]
    fn exports_have_the_settings_and_the_filters_of_the_repository_shown() {
        let mut settings = Settings {
            theme: ThemeChoice::Dark,
            ..Settings::default()
        };
        filtered().apply(&mut settings.graph);
        let file: Value = serde_json::from_str(&export(&settings, true)).unwrap();
        assert_eq!(file["version"], FORMAT_VERSION);
        let exported = &file["settings"];
        assert_eq!(exported["theme"], "Dark");
        assert!(exported["log_window"].get("size").is_none());
        assert!(exported["log_window"].get("dividers").is_none());
        assert!(exported["log_window"].get("layout").is_some());
        assert!(exported["blame_window"].get("history_height").is_none());
        assert!(exported["graph"].get("hide_branches").is_none());
        assert!(exported["graph"].get("show_tags").is_some());

        let repo: RepoSettings = serde_json::from_value(file["repository"].clone()).unwrap();
        assert_eq!(repo, filtered());

        // Without a repository shown, there are no filters to export.
        let file: Value = serde_json::from_str(&export(&settings, false)).unwrap();
        assert!(file.get("repository").is_none());
        assert_eq!(file["settings"]["theme"], "Dark");
    }

    #[test]
    fn importing_replaces_what_the_file_has_and_keeps_the_rest_here() {
        let theirs = Settings {
            theme: ThemeChoice::Dark,
            show_overview: true,
            ..Settings::default()
        };
        let file = export(&theirs, false);

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
