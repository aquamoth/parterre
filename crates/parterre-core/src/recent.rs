use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const MAX_RECENT: usize = 10;

/// The repositories opened most recently, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Recent(Vec<PathBuf>);

impl Recent {
    pub fn add(&mut self, path: &Path) {
        let path = normalize(path);
        self.remove(&path);
        self.0.insert(0, path);
        self.0.truncate(MAX_RECENT);
    }

    pub fn remove(&mut self, path: &Path) {
        self.0.retain(|p| !same_path(p, path));
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Path> {
        self.0.iter().map(PathBuf::as_path)
    }
}

/// git reports `C:/src/repo` on Windows, which becomes `C:\src\repo`. Also drops a trailing
/// separator.
fn normalize(path: &Path) -> PathBuf {
    path.components().collect()
}

/// As far as can be told without asking the file system: Windows paths ignore case and the
/// kind of separator.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let (a, b) = (normalize(a), normalize(b));
    if cfg!(windows) {
        a.to_string_lossy()
            .to_lowercase()
            .eq(&b.to_string_lossy().to_lowercase())
    } else {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(recent: &Recent) -> Vec<&Path> {
        recent.iter().collect()
    }

    #[test]
    fn newest_first_without_duplicates() {
        let mut recent = Recent::default();
        for p in ["/a", "/b", "/c", "/b"] {
            recent.add(Path::new(p));
        }
        assert_eq!(
            paths(&recent),
            [Path::new("/b"), Path::new("/c"), Path::new("/a")]
        );
    }

    #[test]
    fn keeps_the_newest_ten() {
        let mut recent = Recent::default();
        for i in 0..15 {
            recent.add(&PathBuf::from(format!("/repo{i}")));
        }
        assert_eq!(recent.iter().count(), MAX_RECENT);
        assert_eq!(recent.iter().next(), Some(Path::new("/repo14")));
        assert_eq!(recent.iter().last(), Some(Path::new("/repo5")));
    }

    #[test]
    fn a_trailing_separator_is_the_same_path() {
        let mut recent = Recent::default();
        recent.add(Path::new("/a/"));
        recent.add(Path::new("/a"));
        assert_eq!(paths(&recent), [Path::new("/a")]);
        recent.remove(Path::new("/a/"));
        assert!(recent.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_ignore_case_and_separators() {
        let mut recent = Recent::default();
        recent.add(Path::new("C:/Source/Repo"));
        let first = |r: &Recent| r.iter().next().unwrap().to_string_lossy().into_owned();
        assert_eq!(first(&recent), r"C:\Source\Repo");
        recent.add(Path::new(r"c:\source\repo"));
        assert_eq!(recent.iter().count(), 1);
        assert_eq!(first(&recent), r"c:\source\repo");
    }
}
