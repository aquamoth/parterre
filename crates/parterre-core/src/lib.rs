//! Core of parterre: load a repository, reduce it to a TortoiseGit-style revision graph,
//! and lay that graph out. Nothing in this crate depends on a GUI toolkit.

pub mod blame;
pub mod branches;
pub mod changed_files;
pub mod columns;
pub mod compare;
pub mod file_diff;
pub mod file_history;
pub mod find;
pub mod forge;
pub mod git;
pub mod glyphs;
pub mod icon;
pub mod layout;
pub mod log;
pub mod log_graph;
pub mod log_layout;
pub mod oid;
pub mod pattern;
pub mod physics;
pub mod recent;
pub mod repo;
pub mod reset;
pub mod revgraph;
pub mod route;
pub mod text;
pub mod text_size;
pub mod upstream;
pub mod watch;
pub mod worktree_folder;

pub use oid::Oid;
pub use repo::{Commit, CommitIx, GitRef, Head, Label, RefKind, Repo, Worktree};
