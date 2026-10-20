//! Loads a repository, reduces it to a TortoiseGit-style revision graph and lays that out, free
//! of any GUI toolkit.

pub mod banner;
pub mod blame;
pub mod branches;
pub mod changed_files;
pub mod cherry_pick;
pub mod clone;
pub mod columns;
pub mod compare;
pub mod conflicts;
pub mod file_diff;
pub mod file_history;
pub mod find;
pub mod git;
pub mod glyphs;
pub mod icon;
pub mod layout;
pub mod lenient;
pub mod list_keys;
pub mod log;
pub mod log_graph;
pub mod log_layout;
pub mod merge;
pub mod merge_tool;
pub mod merged;
pub mod oid;
mod operation;
pub mod pattern;
pub mod physics;
pub mod rebase;
pub mod recent;
pub mod remote;
pub mod repo;
pub mod reset;
pub mod revert;
pub mod revgraph;
pub mod route;
pub mod shell_path;
pub mod text;
pub mod text_size;
pub mod upstream;
pub mod watch;
pub mod worktree_folder;

pub use oid::Oid;
pub use repo::{Commit, CommitIx, GitRef, Head, Label, RefKind, Repo, Worktree};
