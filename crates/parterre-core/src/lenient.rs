//! Reading settings written by another version of parterre, one setting at a time.
//!
//! Settings are kept as JSON objects. [`read`] puts each setting a file has onto a base value
//! where the result still reads, so one setting a newer version gave a value this one doesn't
//! know (a new theme, say), or one of the wrong type, keeps the base's and the rest still load.
//! Settings the file lacks keep the base's, and those the base doesn't know are left out.
//! [`write_back`] then saves without losing what was left out, for the newer version to find.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

/// A setting [`read`] left out.
#[derive(Clone, Debug, PartialEq)]
pub struct Skipped {
    /// Its keys from the top of the file, e.g. `["log_window", "layout"]`.
    pub path: Vec<String>,
    /// What was used instead: the base's value, or `None` for a setting the base doesn't know.
    pub used: Option<Value>,
}

impl Skipped {
    /// The path with dots, e.g. `log_window.layout`.
    pub fn name(&self) -> String {
        self.path.join(".")
    }

    /// The same, under `key`: for a part of a larger file.
    pub fn under(mut self, key: &str) -> Skipped {
        self.path.insert(0, key.to_owned());
        self
    }
}

/// `file` read onto `base`, and what was left out of it.
#[derive(Clone, Debug)]
pub struct Read<T> {
    pub value: T,
    pub skipped: Vec<Skipped>,
}

/// Reads the settings in `file` onto `base`, one at a time. Nested objects are read setting by
/// setting; any other value, a list for one, is a single setting. A `file` that isn't an object
/// leaves the base as it is.
pub fn read<T: Serialize + DeserializeOwned + Clone>(base: &T, file: &Value) -> Read<T> {
    let Ok(mut merged) = serde_json::to_value(base) else {
        return Read {
            value: base.clone(),
            skipped: Vec::new(),
        };
    };
    let mut settings = Vec::new();
    let mut skipped = Vec::new();
    if let (Some(base), Some(file)) = (merged.as_object(), file.as_object()) {
        collect(base, file, &mut Vec::new(), &mut settings, &mut skipped);
    }
    for (path, value) in settings {
        let mut trial = merged.clone();
        let slot = get_mut(&mut trial, &path).expect("collected from the base");
        let used = std::mem::replace(slot, value.clone());
        if T::deserialize(&trial).is_ok() {
            merged = trial;
        } else {
            skipped.push(Skipped {
                path,
                used: Some(used),
            });
        }
    }
    let value = T::deserialize(&merged).unwrap_or_else(|_| base.clone());
    Read { value, skipped }
}

/// The settings in `file` that `base` has, with their paths, and those it doesn't as skipped.
fn collect<'a>(
    base: &Map<String, Value>,
    file: &'a Map<String, Value>,
    path: &mut Vec<String>,
    settings: &mut Vec<(Vec<String>, &'a Value)>,
    skipped: &mut Vec<Skipped>,
) {
    for (key, value) in file {
        path.push(key.clone());
        match (base.get(key), value) {
            (None, _) => skipped.push(Skipped {
                path: path.clone(),
                used: None,
            }),
            (Some(Value::Object(base)), Value::Object(file)) => {
                collect(base, file, path, settings, skipped);
            }
            (Some(_), _) => settings.push((path.clone(), value)),
        }
        path.pop();
    }
}

/// The setting at `path` in `value`.
pub fn get_mut<'a>(value: &'a mut Value, path: &[String]) -> Option<&'a mut Value> {
    path.iter()
        .try_fold(value, |value, key| value.as_object_mut()?.get_mut(key))
}

/// The setting at `path` in `value`.
pub fn get<'a>(value: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |value, key| value.as_object()?.get(key))
}

/// What to save in place of `file`, which was read with `skipped` left out: `now`, with the
/// settings it doesn't know put back from `file`, and the skipped values put back where `now`
/// still has what was used instead of them.
pub fn write_back(now: Value, file: &Value, skipped: &[Skipped]) -> Value {
    let mut out = now;
    let keep: Vec<(&[String], &Value)> = skipped
        .iter()
        .filter(|s| s.used.as_ref() == get(&out, &s.path))
        .filter_map(|s| Some((s.path.as_slice(), get(file, &s.path)?)))
        .collect();
    for (path, value) in keep {
        if let Some(slot) = get_mut(&mut out, path) {
            *slot = value.clone();
        }
    }
    add_unknown(&mut out, file);
    out
}

/// Adds the settings of `file` that `out` doesn't have, at any depth.
fn add_unknown(out: &mut Value, file: &Value) {
    let (Some(out), Some(file)) = (out.as_object_mut(), file.as_object()) else {
        return;
    };
    for (key, value) in file {
        match out.get_mut(key) {
            Some(mine) => add_unknown(mine, value),
            None => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    #[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
    enum Theme {
        #[default]
        System,
        Dark,
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    struct Window {
        size: [f32; 2],
        fold: bool,
    }

    impl Default for Window {
        fn default() -> Self {
            Window {
                size: [800.0, 600.0],
                fold: true,
            }
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    struct Settings {
        theme: Theme,
        text_size: f32,
        rules: Vec<String>,
        window: Window,
    }

    fn names(skipped: &[Skipped]) -> Vec<String> {
        skipped.iter().map(Skipped::name).collect()
    }

    #[test]
    fn a_setting_that_does_not_read_keeps_the_base_and_the_rest_load() {
        let file = json!({
            "theme": "HighContrast",
            "text_size": 1.5,
            "rules": ["a", "b"],
            "window": {"size": [1.0, 2.0], "fold": "yes"},
        });
        let read = read(&Settings::default(), &file);
        assert_eq!(
            read.value,
            Settings {
                theme: Theme::System,
                text_size: 1.5,
                rules: vec!["a".into(), "b".into()],
                window: Window {
                    size: [1.0, 2.0],
                    fold: true,
                },
            }
        );
        assert_eq!(names(&read.skipped), ["theme", "window.fold"]);
    }

    #[test]
    fn missing_settings_keep_the_base_and_unknown_ones_are_left_out() {
        let base = Settings {
            theme: Theme::Dark,
            ..Settings::default()
        };
        let file = json!({"text_size": 2.0, "window": {"tabs": 3}, "sounds": true});
        let read = read(&base, &file);
        assert_eq!(read.value.theme, Theme::Dark);
        assert_eq!(read.value.text_size, 2.0);
        assert_eq!(read.value.window, Window::default());
        assert_eq!(names(&read.skipped), ["sounds", "window.tabs"]);
        assert!(read.skipped.iter().all(|s| s.used.is_none()));
    }

    #[test]
    fn a_file_that_is_not_an_object_leaves_the_base() {
        let read = read(&Settings::default(), &json!([1, 2]));
        assert_eq!(read.value, Settings::default());
        assert!(read.skipped.is_empty());
    }

    #[test]
    fn a_list_is_one_setting() {
        let file = json!({"rules": ["a", 3]});
        let read = read(&Settings::default(), &file);
        assert!(read.value.rules.is_empty());
        assert_eq!(names(&read.skipped), ["rules"]);
    }

    #[test]
    fn writing_back_keeps_what_a_newer_version_wrote() {
        let file = json!({
            "theme": "HighContrast",
            "text_size": 1.5,
            "window": {"size": [1.0, 2.0], "tabs": 3},
            "sounds": true,
        });
        let read = read(&Settings::default(), &file);
        let mut now = read.value.clone();
        now.text_size = 2.0;
        let out = write_back(serde_json::to_value(&now).unwrap(), &file, &read.skipped);
        // Unchanged here, the unknown theme goes back; known settings are this version's.
        assert_eq!(out["theme"], "HighContrast");
        assert_eq!(out["text_size"], 2.0);
        assert_eq!(out["sounds"], true);
        assert_eq!(out["window"]["tabs"], 3);
        assert_eq!(out["window"]["fold"], true);

        // Chosen here since, the theme is this version's.
        now.theme = Theme::Dark;
        let out = write_back(serde_json::to_value(&now).unwrap(), &file, &read.skipped);
        assert_eq!(out["theme"], "Dark");
    }

    #[test]
    fn paths_name_a_part_of_a_larger_file() {
        let read = read(&Settings::default(), &json!({"theme": 1}));
        let skipped: Vec<_> = read.skipped.into_iter().map(|s| s.under("x.y")).collect();
        assert_eq!(skipped[0].path, ["x.y", "theme"]);
        let file = json!({"x.y": {"theme": "Neon"}});
        let now = json!({"x.y": {"theme": "System"}});
        let skipped = [Skipped {
            path: vec!["x.y".into(), "theme".into()],
            used: Some(json!("System")),
        }];
        assert_eq!(
            write_back(now, &file, &skipped),
            json!({"x.y": {"theme": "Neon"}})
        );
    }
}
