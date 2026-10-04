# parterre – notes for agents

Standalone TortoiseGit-style revision graph viewer. Rust workspace, egui/eframe GUI.

- `crates/parterre-core` must stay free of GUI dependencies; put anything testable there.
- `crates/parterre` is the app; keep rendering and interaction there.
- `crates/parterre-forge` (pull requests, `github` feature), `crates/parterre-highlight`
  (tree-sitter, `syntax` feature) and `crates/parterre-telemetry` (update check and PostHog,
  `send` feature) wrap external stacks: their dependencies live in those manifests only, so an
  engine is swapped by replacing the crate. `crates/parterre-util` holds
  the std-only cancellation handles the others share; keep it that small (#214).
- Behavioural reference for what TortoiseGit does: `docs/research/tortoisegit-revision-graph.md`.
- Screenshots live in `docs/images/<major>.<minor>/`; published packages link to them, so never
  move or delete one (`docs/releasing.md`).
- Work is tracked in GitHub issues (`gh issue`), not in files: open questions and decisions
  for the human to review are labelled `question`, planned work `enhancement`, and defects
  `bug`. Refer to them by number (#68).

Commands (Rust from `~/.cargo/bin`):

- `cargo test --workspace` – tests; integration tests create throwaway repos with the git CLI.
  The test helpers' git ignores the system config; parterre's own git reads it, and Git for
  Windows sets `core.autocrlf` there, so files parterre checks out end in CRLF on Windows.
  Compare them with `common::read_text`, and check with
  `GIT_CONFIG_SYSTEM="$PWD/.github/autocrlf.gitconfig" cargo test --workspace` (CI does, #182).
  A test must not write under `.git/refs` by hand: Git 3.0 makes new repositories reftable
  (#228). To check once, run the tests with git built `WITH_BREAKING_CHANGES=YesPlease` first
  on `PATH`; CI doesn't (#251).
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all` before committing.
- `cargo run --release -p parterre-core --example stats -- <repo>` – graph sizes and timings.
- Automation, for checking visuals without a human (`docs/automation.md`):
  - `cargo run -- <repo> --screenshot out.png` – the window as a PNG.
  - `--script FILE` – drive the window: `open` any window or dialog, click by on-screen text or
    `node:REF`, type, and take screenshots cropped to a dialog or menu.
  - `--record out.gif|out.mp4` – record it, for PR previews.
  - `scripts/screenshots.sh /tmp/shots` – every window, dialog and menu of a demo repo as PNGs.
