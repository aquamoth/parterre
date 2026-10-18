mod common;

use common::TestRepo;
use parterre_core::Oid;
use parterre_core::changed_files::{ChangedFile, FileStatus};
use parterre_core::file_diff::{Content, DiffOptions, FileDiff, FileDiffSpec, LoadedDiff, Note};
use parterre_core::git::Git;

fn load(r: &TestRepo, commit: &str, path: &str) -> (ChangedFile, LoadedDiff) {
    let git = Git::new(r.path());
    let oid = Oid::from_hex(commit).unwrap();
    let files = git.changed_files(&oid).unwrap();
    let file = files
        .into_iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{path} not among the changed files"));
    let parent = r.git(&["rev-parse", &format!("{commit}^")]);
    let spec = FileDiffSpec::of_commit(oid, Oid::from_hex(&parent), &file);
    (file, git.load_file_diff(&spec).unwrap())
}

fn texts(loaded: &LoadedDiff) -> (&str, &str) {
    match &loaded.content {
        Content::Text { old, new, .. } => (old, new),
        other => panic!("expected text, got {other:?}"),
    }
}

/// Long enough for git to see a small edit as a rename.
const RUST: &str = "fn main() {\n    let a = 1;\n    let b = 2;\n    let c = a + b;\n    println!(\"hi\");\n    println!(\"{c}\");\n}\n";

fn base() -> TestRepo {
    let mut r = TestRepo::new();
    r.write("a.txt", b"one\ntwo\nthree\n");
    r.write("gone.txt", b"bye\n");
    r.write("run.sh", b"echo hi\n");
    r.write("old_name.rs", RUST.as_bytes());
    r.write("latin1.txt", b"Caf\xe9\n");
    r.write("img.bin", b"\x00\x01\x02\x03binary");
    r.commit_all("base");
    r
}

#[test]
fn a_modified_file_reads_both_versions() {
    let mut r = base();
    r.write("a.txt", b"one\n2\nthree\n");
    let c = r.commit_all("edit");
    let (_, loaded) = load(&r, &c, "a.txt");
    assert_eq!(texts(&loaded), ("one\ntwo\nthree\n", "one\n2\nthree\n"));
    let (old, new) = texts(&loaded);
    let d = FileDiff::new(old, new, DiffOptions::default());
    assert_eq!((d.added, d.removed), (1, 1));
    assert!(d.notes(&loaded, Default::default()).is_empty());
}

#[test]
fn a_rename_with_an_edit_reads_the_old_path_in_the_parent() {
    let mut r = base();
    r.git(&["mv", "old_name.rs", "new_name.rs"]);
    r.write(
        "new_name.rs",
        RUST.replace("\"hi\"", "\"hello\"").as_bytes(),
    );
    let c = r.commit_all("rename");
    let (file, loaded) = load(&r, &c, "new_name.rs");
    assert_eq!(file.status, FileStatus::Renamed);
    assert_eq!(loaded.spec.old.as_ref().unwrap().path, "old_name.rs");
    let (old, new) = texts(&loaded);
    assert!(old.contains("\"hi\"") && new.contains("\"hello\""));
}

#[test]
fn added_and_deleted_files_have_an_empty_side() {
    let mut r = base();
    r.write("new.txt", b"fresh\n");
    std::fs::remove_file(r.path().join("gone.txt")).unwrap();
    let c = r.commit_all("add and delete");
    let (_, added) = load(&r, &c, "new.txt");
    assert!(added.spec.old.is_none());
    assert_eq!(texts(&added), ("", "fresh\n"));
    let (_, deleted) = load(&r, &c, "gone.txt");
    assert!(deleted.spec.new.is_none());
    assert_eq!(texts(&deleted), ("bye\n", ""));
}

#[test]
fn a_mode_change_is_noted_with_unchanged_content() {
    let mut r = base();
    r.git(&["update-index", "--chmod=+x", "run.sh"]);
    let c = r.commit("make executable");
    let (file, loaded) = load(&r, &c, "run.sh");
    assert_eq!(file.modes, [0o100644, 0o100755]);
    let (old, new) = texts(&loaded);
    let d = FileDiff::new(old, new, DiffOptions::default());
    assert_eq!(
        d.notes(&loaded, Default::default()),
        [
            Note::Unchanged,
            Note::Mode {
                old: 0o100644,
                new: 0o100755
            }
        ]
    );
}

#[test]
fn invalid_utf8_is_escaped_and_counted() {
    let mut r = base();
    r.write("latin1.txt", b"Caf\xe9!\n");
    let c = r.commit_all("latin-1 edit");
    let (_, loaded) = load(&r, &c, "latin1.txt");
    assert_eq!(texts(&loaded), ("Caf\\xE9\n", "Caf\\xE9!\n"));
    let Content::Text { invalid_bytes, .. } = loaded.content else {
        unreachable!()
    };
    assert_eq!(invalid_bytes, 2);
}

#[test]
fn binary_files_give_their_sizes() {
    let mut r = base();
    r.write("img.bin", b"\x00\x01\x02\x03binary, longer");
    let c = r.commit_all("binary edit");
    let (file, loaded) = load(&r, &c, "img.bin");
    assert!(file.is_binary());
    assert_eq!(
        loaded.content,
        Content::Binary {
            old_size: Some(10),
            new_size: Some(18)
        }
    );
}

#[test]
fn textconv_filters_apply_and_are_named() {
    let mut r = base();
    r.write(".gitattributes", b"*.up diff=upper\n");
    r.git(&["config", "diff.upper.textconv", "tr a-z A-Z <"]);
    // Textconv is given the working-tree form of the file, and Git for Windows' own config
    // turns on core.autocrlf, which would give CRLF there. (Both versions get the same.)
    r.git(&["config", "core.autocrlf", "false"]);
    r.write("note.up", b"shout\n");
    r.commit_all("base note");
    r.write("note.up", b"shout louder\n");
    let c = r.commit_all("edit note");
    let (_, loaded) = load(&r, &c, "note.up");
    assert_eq!(texts(&loaded), ("SHOUT\n", "SHOUT LOUDER\n"));
    assert_eq!(loaded.textconv.as_deref(), Some("upper: tr a-z A-Z <"));
}

#[test]
fn a_submodule_gives_its_commits() {
    let mut r = base();
    let first = r.git(&["rev-parse", "HEAD"]);
    r.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{first},sub"),
    ]);
    let c = r.commit("add submodule");
    let (file, loaded) = load(&r, &c, "sub");
    assert_eq!(file.modes, [0, 0o160000]);
    assert_eq!(
        loaded.content,
        Content::Submodule {
            old: None,
            new: Oid::from_hex(&first)
        }
    );
}
