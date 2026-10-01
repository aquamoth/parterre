//! Where a new worktree goes: the folder suggested for it, as VS Code suggests one, and whether
//! that folder would sit unignored inside a repository's working tree.
//!
//! The suggestion is `<parent>/<repo>.worktrees/<name>` from the main worktree, and a sibling
//! `<parent>/<name>` from a linked one. Nothing is stored: the root is worked out each time.

use std::path::{Component, Path, PathBuf};

use crate::git::{Git, GitError};

/// The folder new worktrees go in by default.
pub fn default_root(main: &Path, open: &Path) -> PathBuf {
    if !same_path(main, open) {
        return open.parent().unwrap_or(open).to_owned();
    }
    let name = main
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repository".into());
    main.parent()
        .unwrap_or(main)
        .join(format!("{name}.worktrees"))
}

/// Path separators, and the characters Windows allows in no file name.
pub fn forbidden(c: char) -> bool {
    matches!(c, '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*') || c.is_control()
}

/// A folder name for a branch: `feature/x` becomes `feature-x`.
pub fn folder_name(branch: &str) -> String {
    branch
        .trim()
        .chars()
        .map(|c| if forbidden(c) { '-' } else { c })
        .collect()
}

/// A branch name made from a commit's subject: its first four words, lower case, joined by `-`.
pub fn subject_slug(subject: &str) -> String {
    subject
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(4)
        .collect::<Vec<_>>()
        .join("-")
}

/// Why a typed folder name can't be used, regardless of what's on disk.
pub fn name_error(name: &str) -> Option<&'static str> {
    let name = name.trim();
    if name.is_empty() {
        Some("Enter a folder name.")
    } else if name == "." || name == ".." {
        Some("Enter a folder name other than . or ..")
    } else if name.chars().any(forbidden) {
        Some("A folder name can't contain / \\ < > : \" | ? or *.")
    } else {
        None
    }
}

/// A folder git would refuse to add a worktree in: a file, a folder with something in it, or
/// the folder of a worktree git still knows (`registered`), even one that's gone.
pub fn taken(path: &Path, registered: &[PathBuf]) -> bool {
    registered.iter().any(|r| same_path(r, path))
        || path.is_file()
        || path.read_dir().is_ok_and(|mut d| d.next().is_some())
}

/// `base`, or `base-1`, `base-2`… when that's taken in `root`.
pub fn free_name(root: &Path, base: &str, registered: &[PathBuf]) -> String {
    if !taken(&root.join(base), registered) {
        return base.to_owned();
    }
    (1..)
        .map(|k| format!("{base}-{k}"))
        .find(|name| !taken(&root.join(name), registered))
        .expect("an unbounded search finds a free name")
}

/// Whether two paths name the same folder: as written, or once symlinks are resolved.
pub fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// A folder inside a repository's working tree, where git status there would list it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inside {
    /// The top of that working tree.
    pub top: PathBuf,
    /// What *Exclude it* adds to that repository's `info/exclude`: the folder's first component
    /// below `top`, as `/name/`.
    pub pattern: String,
}

/// The working tree `folder` would be inside, unless it's ignored there. Asked of the nearest
/// folder that exists, so it works before the folder does.
pub fn inside_repository(folder: &Path) -> Option<Inside> {
    let existing = folder.ancestors().find(|a| a.is_dir())?;
    let top = Git::new(existing)
        .query(&["rev-parse", "--show-toplevel"])
        .ok()
        .flatten()
        .map(|s| PathBuf::from(s.trim()))?;
    let top = std::fs::canonicalize(&top).unwrap_or(top);
    let canonical = std::fs::canonicalize(existing).ok()?;
    let rest = folder.strip_prefix(existing).ok()?;
    let full = canonical.join(rest);
    let relative = full.strip_prefix(&top).ok()?;
    let mut parts = relative.components().filter_map(|c| match c {
        Component::Normal(p) => Some(p.to_string_lossy().into_owned()),
        _ => None,
    });
    let first = parts.next()?;
    let relative = relative.to_string_lossy().replace('\\', "/");
    let ignored = Git::new(&top)
        .query(&["check-ignore", "-q", "--", &format!("{relative}/")])
        .ok()
        .flatten()
        .is_some();
    (!ignored).then(|| Inside {
        top,
        pattern: format!("/{first}/"),
    })
}

/// Adds the pattern to that repository's `info/exclude`, which every worktree of it shares.
pub fn exclude(inside: &Inside) -> Result<(), GitError> {
    let path = Git::new(&inside.top).run(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "info/exclude",
    ])?;
    let path = PathBuf::from(path.trim());
    let read = |e| GitError::Read {
        path: path.clone(),
        source: e,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(read)?;
    }
    let mut text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(read(e)),
    };
    if text.lines().any(|l| l.trim() == inside.pattern) {
        return Ok(());
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&inside.pattern);
    text.push('\n');
    std::fs::write(&path, text).map_err(read)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_is_beside_the_main_worktree_or_a_sibling_of_a_linked_one() {
        let main = Path::new("/src/parterre");
        assert_eq!(
            default_root(main, main),
            Path::new("/src/parterre.worktrees")
        );
        assert_eq!(
            default_root(main, Path::new("/src/parterre.worktrees/fix")),
            Path::new("/src/parterre.worktrees")
        );
    }

    #[test]
    fn folder_names_replace_separators_and_what_windows_forbids() {
        assert_eq!(folder_name("feature/x"), "feature-x");
        assert_eq!(folder_name("a<b>c|d\"e"), "a-b-c-d-e");
        assert_eq!(folder_name(" topic "), "topic");
    }

    #[test]
    fn a_subject_becomes_its_first_four_words() {
        assert_eq!(subject_slug("Parse numbers"), "parse-numbers");
        assert_eq!(
            subject_slug("Fix a typo in the docs, again"),
            "fix-a-typo-in"
        );
        assert_eq!(subject_slug("…"), "");
    }

    #[test]
    fn empty_dot_and_separators_are_not_folder_names() {
        assert!(name_error("").is_some());
        assert!(name_error("  ").is_some());
        assert!(name_error(".").is_some());
        assert!(name_error("..").is_some());
        assert!(name_error("a/b").is_some());
        assert!(name_error("a:b").is_some());
        assert_eq!(name_error("fix-typo"), None);
    }

    #[test]
    fn a_taken_folder_gets_a_number() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(free_name(root, "fix", &[]), "fix");
        // An empty folder is free; git adds a worktree there.
        std::fs::create_dir(root.join("fix")).unwrap();
        assert_eq!(free_name(root, "fix", &[]), "fix");
        std::fs::write(root.join("fix/notes.txt"), "x").unwrap();
        assert_eq!(free_name(root, "fix", &[]), "fix-1");
        std::fs::write(root.join("fix-1"), "a file").unwrap();
        // A registered worktree whose folder is gone still takes its name.
        let gone = root.join("fix-2");
        assert_eq!(free_name(root, "fix", &[gone]), "fix-3");
    }
}
