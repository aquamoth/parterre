//! Core of parterre: load a repository, reduce it to a TortoiseGit-style revision graph,
//! and lay that graph out. Nothing in this crate depends on a GUI toolkit. The pull-request
//! client and the syntax colour are crates of their own (#214).

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
pub mod log;
pub mod log_graph;
pub mod log_layout;
pub mod merge;
pub mod merge_tool;
pub mod oid;
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
