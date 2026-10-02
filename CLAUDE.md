# parterre – notes for agents

Standalone TortoiseGit-style revision graph viewer. Rust workspace, egui/eframe GUI.

- `crates/parterre-core` must stay free of GUI dependencies; put anything testable there.
- `crates/parterre` is the app; keep rendering and interaction there.
- Behavioural reference for what TortoiseGit does: `docs/research/tortoisegit-revision-graph.md`.
- Work is tracked in GitHub issues (`gh issue`), not in files: open questions and decisions
  for the human to review are labelled `question`, planned work `enhancement`, and defects
  `bug`. Refer to them by number (#68).

Commands (Rust from `~/.cargo/bin`):

- `cargo test --workspace` – tests; integration tests create throwaway repos with the git CLI.
  The test helpers' git ignores the system config; parterre's own git reads it, and Git for
  Windows sets `core.autocrlf` there, so files parterre checks out end in CRLF on Windows.
  Compare them with `common::read_text`, and check with
  `GIT_CONFIG_SYSTEM="$PWD/.github/autocrlf.gitconfig" cargo test --workspace` (CI does, #182).
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all` before committing.
- `cargo run --release -p parterre-core --example stats -- <repo>` – graph sizes and timings.
- `cargo run --release -- <repo> --screenshot out.png` – render one frame to a PNG (for checking
  visuals without a human).
