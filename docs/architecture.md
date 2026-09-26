# Architecture

parterre is a Cargo workspace with two crates:

```
crates/parterre-core   GUI-free; everything testable lives here
  git.rs               run `git log` / `git for-each-ref`, parse into a Repo; changed files
                       of a commit (`git diff-tree`)
  repo.rs              Repo snapshot: commits (with parent indices), refs, HEAD, git's hash
                       length
  log.rs               log query: tips and exclusions → commits in `git log --date-order`
                       order, from the snapshot alone
  log_layout.rs        the log window's four fixed layouts and their divider positions
  changed_files.rs     changed-file types, `diff-tree -z` parser, files-before-folders order,
                       the log window's file filter and column sort
  text.rs              URLs in commit messages, paths cut at the start, thousands separators
  revgraph.rs          reduce the commit DAG to a revision graph (TortoiseGit's rules)
  pattern.rs           branch-name wildcards, for hiding and colouring branches
  forge.rs             open pull requests: the model, where each is shown (head commit and
                       base-branch refs), remotes and upstreams from git
    github.rs          github.com remotes; one GraphQL request per 100 fetched branches of
                       origin (and its parent), signed in with `gh auth token`, within a
                       rate-limit budget; HTTPS through ureq behind the `github` feature
  recent.rs            the recently opened repositories
  watch.rs             fingerprint of the files git keeps refs in, for reloading by itself
  glyphs.rs            toolbar and menu icons as SVG path data, and a path flattener
  layout/              layered (Sugiyama) layout
    rank.rs            layer assignment (network simplex / longest path / chronological),
                       plus splitting of over-wide layers
    layered.rs         dummy items for long edges, optional edge bundling
    order.rs           crossing minimisation (median sweeps, exact crossing count)
    position.rs        coordinates within layers (L1 via isotonic regression)
    mod.rs             pipeline, variable layer spacing, direction/rotation
  physics.rs           rearranging by hand: springs, weak magnets, drag modes, undo
  route.rs             routing edges afresh around rearranged nodes

crates/parterre        the binary (eframe/egui)
  build.rs             asks git for the commit and sets the version string
  main.rs              CLI (clap), window setup
  version.rs           release/dev version strings (runs in build.rs; see docs/releasing.md)
  app.rs               canvas interaction, search, status bar, windows, opening folders
    toolbar.rs         the toolbar, its popovers and the ☰ menu
    settings_window.rs the settings: pages of rows, applied as you change them
    auto_reload.rs     a worker thread that reloads when the refs change
    pull_requests.rs   loads open pull requests on a worker thread while they are shown, cached
                       per repository, with back-off
    log_window.rs      the log window (Show log): an immediate viewport with three panes
                       (commits, details, changed files) that one of four fixed layouts
                       arranges, picked in its header; changed files come from git on a
                       worker thread
  scene.rs             node contents and sizes + layout + physics net, hit testing
  render.rs            painting nodes, edges, arrows, overview
  export.rs            SVG export, and PNG and WebP export: render.rs painted in tiles by
                       an offscreen egui context, sized to stay within 100 megapixels
  raster.rs            software rasteriser for egui's meshes (for PNG and WebP, with no GPU
                       or window)
  view.rs              pan/zoom transform
  theme.rs             TortoiseGit colours (light, and dark via lightness inversion)
  system_theme.rs      light or dark desktop preference on Linux (XDG portal)
  menu.rs              the look of menus and popovers, menu items
  browser.rs           opens github.com pages with the platform's opener
  widgets.rs           icon buttons, segmented buttons, switches, text fields
  settings.rs          persisted settings and the Classic/Modern looks
  automation.rs        --screenshot / --demo-drag / --demo-menu / --demo-open / --demo-log
                       scripted runs (the log window's layout: --log-layout)
```

## Data flow

1. **Load** (`git.rs`): one `git log --all` (notes excluded) with a compact
   `\x1f`/`\x1e`-separated format, one `git for-each-ref`, and HEAD queries. About 100 ms for
   15k commits. The snapshot holds *all* commits so view options never need git again.
2. **Reduce** (`revgraph.rs`): pick visible refs → reachable commits → decide which commits
   are nodes in one parents-first pass, recording for each hidden commit the node that
   represents it. Edges go from each node to the representatives of its parents.
   - Only refs that pass the filters start history. Branches matching the *Hide branches*
     wildcards don't, but they still label commits that other refs reach. So a hidden branch
     vanishes only if no shown branch contains it.
   - Open pull requests, while shown, label their head commits the same way: they never
     start history, and show only where one of their base branch's refs is shown. As labels
     they make their heads nodes, like tags. They are loaded from GitHub on a worker thread
     (`forge`), separately from the snapshot, which stays what git has.
   - *Labelled commits* reproduces `git log --simplify-by-decoration`, including
     `simplify_merges` (redundant parents dropped) and empty-tree roots (TREESAME).
     The node sets are identical on the 15k-commit Apps repository and on 400 random
     repositories, and the edges match on 300 of them.
     - One deliberate exception: git hides an empty-tree root even when it carries a label;
       parterre shows it.
   - *Branchings and merges* reproduces TortoiseGit's chain collapse.
3. **Measure** (`scene.rs`): node boxes use TortoiseGit's geometry: one row per ref, or an
   8-digit hash; 20 px side margins and 5 px top and bottom margins; monospace 12 px.
4. **Lay out** (`layout/`): rank → split wide layers → layered graph with dummies (optionally
   bundled per parent) → crossing minimisation → L1 coordinates → variable layer gaps →
   rotate to the chosen direction.
5. **Simulate** (`physics.rs`): nodes and bend points become particles, each with a rest
   position (at first the layout). Springs along edges keep the offsets between rest
   positions; nodes near each other push apart like weak magnets, and neighbours in a row keep
   their order; weak anchors hold each particle to its rest position.
   - **Drag modes:** *Adapt* lets the rest of the graph give way; *Free* and *Subtree* move
     only the dragged nodes (Subtree adds their first-parent descendants) and stretch the
     edges to them.
   - **Shape:** each frame of an adaptive drag, the target shape is relaxed with Gauss-Seidel
     over displacements. Between sweeps, edge segments are put back in history order
     (children above parents, with a gap) by one pass along the flow and one against it, and
     overlapping boxes are pushed apart. Only the dragged nodes, and edges left reversed at
     rest, can break the order.
   - **Motion:** particles follow the target through damped springs.
   - **Drop:** whatever moved rests where it is from then on, so moved nodes keep giving way
     to later drags instead of being pinned. Drops, resets and returns to the layout are
     undoable.
   - Only the dragged nodes' neighbourhood (up to 8000 particles) is simulated, and the
     simulation sleeps when still.
   - **Routing** (`route.rs`): edges whose layout route no longer fits are routed afresh.
     That means edges at nodes moved by hand, edges pulled far out of shape, and edges a moved
     node covers. The router groups the boxes in between into rows, picks a gap in each so
     that sideways moves happen where there is room, and pulls the route taut through them
     (funnel algorithm). Anything still in the way is walked around corner by corner. An
     edge whose parent has been moved before its child is routed round both nodes, from just
     below the child to just above the parent.
6. **Paint** (`render.rs`): edges then nodes, culled to the viewport; text is skipped below
   4 px. Edges leave a node from the side facing its parents and enter from the side facing
   its children (bottom and top, newest on top), unlike TortoiseGit, which clips them to the
   box border wherever they hit it. An edge turned around therefore shows as a loop: it
   follows its route round the nodes, or else detours round the side of the boxes.

## Testing

- Unit tests next to the code, plus integration tests (`crates/parterre-core/tests/`) that
  build throwaway repositories with the git CLI.
- Property tests (`properties_*.rs`) run random DAGs, repositories and drags against the
  invariants:
  - every parent is below its children
  - coordinates are finite and inside the bounds
  - network simplex is optimal on tiny graphs (checked by brute force)
  - every edge ends on a node
  - the net comes to rest and stays there; it returns home after a reset
  - after an adaptive drag, every child is still above its parents
- `cargo test --release -p parterre-core --test properties_layout -- --ignored --nocapture`
  prints timings for large, awkward inputs.

## Why these choices

- **Rust + egui/eframe**: native speed, one codebase for Linux, Windows and macOS, and an
  immediate-mode canvas that makes custom drawing and dragging simple. No system development
  packages are needed to build on Linux (winit/glutin load Wayland/X11/GL at runtime).
- **git CLI instead of a git library**: always available where parterre is useful, honours
  every repo configuration, fast enough (see above), and keeps the build free of C
  dependencies.
- **Own layout instead of a graph-layout crate**: git-specific needs (first-parent weighting,
  TortoiseGit parity, layer splitting, bundling, anytime network simplex) and full control
  over performance. 15k nodes lay out in about 200 ms.
