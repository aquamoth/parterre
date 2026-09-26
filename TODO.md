# TODO

## Open questions (HITL)

_Decisions I made on my own that you may want to overrule. Try them with `parterre` on
`~/Source/repos/Cosmo/Apps`; most are one click in the toolbar or menus._

_Numbers are never changed or reused, even after an item is deleted. Next number: 26._

1. **Default look: "Modern" or "Classic"?** *Settings → Appearance → Style* switches.
   - **Classic** is TortoiseGit: straight edges, every edge drawn separately, rows as wide as
     needed.
   - **Modern** (current default) uses curved edges and bundles edges that run into the same
     commit into one trunk. It also splits rows wider than 1800 px so sibling branches stack.

   On Apps, ~160 remote branches hang off a few commits. In Classic that gives rows many
   thousands of pixels wide with fans of near-horizontal lines. Modern reads like a tree.
   Which do you want by default? Answered 2026-09-26: Modern.
2. **Rearranging by hand.** Reworked from your notes of 2026-09-24:
   - A dropped node is no longer pinned. Wherever things come to rest becomes their new
     resting shape, so moved nodes keep giving way to later drags like any other node.
   - Three drag modes, in the toolbar, the *Drag* menu and on keys `1` `2` `3`:
     - **Adapt** (default): neighbours follow along their edges (*pull* slider), and nodes
       that come near are pushed aside like weak magnets (*push*). Nodes side by side in a row
       are pushed ahead along it; lifting a node out of the row lets it pass them.
     - **Free**: only the selected nodes move; the edges to them stretch.
     - **Subtree**: the selected nodes and everything that grows out of them. Hovering shows
       what would move.
   - Switching back to Adapt keeps every node where it is, and from then on the springs hold
     the new offsets between neighbours.
   - Selecting: click; Ctrl+click toggles; Shift+click adds; Shift+drag the background selects
     a rectangle; right-click → *Select subtree*. Dragging a selected node moves the whole
     selection.
   - Undo and redo with Ctrl+Z / Ctrl+Shift+Z; `R` (reset) can be undone too.
   - Edges re-route as you drag (your notes of 2026-09-24, second round): an edge at a node
     you moved takes a new route through the gaps between the rows it crosses. It bends only
     where it must, so it loses bends when its nodes come together and goes around the nodes
     in between when they move apart. Edges a moved node comes to cover make way too.

   Decisions you may want to overrule:
   - **Subtree follows first parents.** A commit's subtree is everything whose first-parent
     line leads back to it: its branch, and the branches forking off that. A merge that pulls
     the branch in belongs to the line it merged into, so it stays put. Taking every descendant
     instead would include everything merged later, often most of the graph.
   - **What moved stays moved.** Neighbours that Adapt pulls or pushes keep their new places
     after the drop. The alternative is for them to spring back, so that only the dragged node
     keeps its new place.
   - **No blue dots** any more (your notes of 2026-09-24, third round). Nodes you moved can
     still be returned to the layout (right-click, or `R` for all).
   - **Magnets act between nodes.** During a drag, edges make way only for nodes in their own
     row; once the node is dropped on them they re-route around it.
   - **Which edges re-route.** Edges at nodes you moved re-route as soon as they change.
     Edges that only gave way keep the layout's route (bent along) unless pulled more than
     40 px out of shape. Re-routing those too made whole fans lose their bundled trunks when a
     shared parent moved a pixel.
   - **Re-routed edges switch at once,** without animating from the old route to the new.
     They also leave the trunks that bundled edges share.
   - **Reversed edges** (third round, which overrules the second): edges always leave the
     child's bottom and enter the parent's top. An edge whose parent is dragged above its
     child is routed round both nodes, from just below the child to just above the parent,
     so the reversal shows as a loop.
   - **Children above parents** (third round). In Adapt, dragging a node up pushes its
     children up ahead of it, and dragging it down pushes its parents down, with at least
     24 px between boxes. Only the dragged nodes can end up past a parent. Free and Subtree
     move nothing else, so there the order can break, and a reversal left at rest is kept
     when the graph later adapts around it. OK?
   - **Free and Subtree allow overlaps,** and Adapt leaves them alone: it only keeps nodes
     from coming closer than they rest.
   - **Defaults:** pull 0.3 (a neighbour moves about half as far as the dragged node, the next
     one a quarter), push 0.5 (nodes start pushing each other 32 px apart), wobble 0.4. *Pull*
     replaces the old *reach* setting and starts at its new default.
   - **The old prototypes are gone.** Spider web and Strings became Adapt; Rigid became Free.
   - **Mode switching** uses keys and buttons only. Shift and Ctrl already mean selection;
     another modifier (such as holding Space) could give a one-off Free drag.
   - **Remembering moves.** *Remember moved nodes* (in the drag options, off by default) keeps
     nodes where they rest, per repository, across runs and relayouts. Should it be on by
     default? Answered 2026-09-26: no, off.
3. **Fidelity quirks in "Labelled commits" (TortoiseGit's default mode).** TortoiseGit uses
   `git log --simplify-by-decoration` and inherits git's simplifications:
   - A `--no-ff` merge whose first parent is an ancestor of its second is folded away.
   - A merge that brings in a history whose root has an *empty tree* (svn imports,
     `--allow-empty` first commits) is folded away.

   parterre copies this exactly; its node set for Apps is identical to git's (266 nodes).
   "Branchings and merges" and "All commits" show the real topology. Keep the fidelity?
4. **Initial view.** As in TortoiseGit, the window opens at 100% with HEAD near the top. On
   big graphs that shows only a small area. Would fit-to-window, or a fixed zoom such as 60%,
   be better? Answered 2026-09-26: keep 100% on the current branch, but reopen at the zoom and
   position last stored for the repository (planned).
5. **Layer spacing.** Gaps between rows grow when long sideways edges cross them. TortoiseGit
   (OGDF) does the same, capped at 300 px. This keeps edges steep but makes the graph taller.
   Tune it under *Settings → Advanced*. Happy with the default?
6. **Stash** is shown, as in TortoiseGit, as a single edge to its base commit. The index and
   untracked-files snapshot commits are hidden.
7. **`origin/HEAD`**-style symbolic refs are hidden, because they duplicate `origin/main`.
   TortoiseGit shows them.
8. **Other refs** (`refs/t3/*` in Apps) are hidden by default; ☰ → Show → Other refs shows them.
9. **HEAD marker.** Like TortoiseGit, only the current branch's row is highlighted (red). A
   detached HEAD gets its own red "HEAD" row, which TortoiseGit doesn't have.
10. **No git actions.** Answered 2026-09-26: yes, towards functional parity with TortoiseGit's
    workflows from the revision graph and the log (its node menu was the starting inventory).
    The roadmap, its boundary rules and the open decisions live in the map *Roadmap to
    TortoiseGit parity: revision graph and log*
    ([#25](https://github.com/aquamoth/parterre/issues/25)). Show log came first, diffs come
    next; *Browse repository* and the menu-bar Git menu are out.
11. **Performance at 100k commits.** I measured this on a synthetic repository with 100k
    commits, 2,490 refs and 1,846 merges:
    - Loading takes 0.6 s.
    - "Labelled commits" (3.6k nodes) lays out in 0.15 s, "Branchings and merges" (7.3k nodes)
      in 0.2 s.
    - "All commits" takes 1.5–2 s on a background thread, because long-lived branches create
      1.5M bend points. Peak memory is then about 740 MB (was 2.8 s and 900 MB before the
      layout and the drag net stored their neighbour lists flat, 2026-09-26). About 200 MB of
      that is the window itself, as for any repository; most of the rest is the drag net.
    - Dragging runs at 8 ms per frame.

    Is 2 s and 740 MB for the all-commits view of a 100k repo acceptable, or worth more
    work? (Apps, at 15k commits, needs 0.2 s and about 210 MB, 150 MB in "Labelled commits".)
12. **Releases** (`docs/releasing.md`). Decisions you may want to overrule:
    - **Version in the UI:** besides `--version`, the ☰ menu ends with a greyed
      `parterre 0.3.0 (a1b2c3d)` line, for users who start parterre from a file manager or
      Start menu and never see a terminal.
    - **Assets:** one archive per target (`.tar.gz`, `.zip` on Windows) holding the binary,
      the README, `LICENSE`, `NOTICE` and `THIRD-PARTY-NOTICES.html`, plus `SHA256SUMS`. macOS
      gets both Apple silicon and Intel builds, the Intel one cross-compiled and therefore not
      test-run in the workflow.
    - **Linux baseline** (your decision of 2026-09-25): built in an Ubuntu 22.04 container, so
      the binary needs glibc 2.35 or newer (Debian 12, Ubuntu 22.04 and later).
    - **Commit detection** is a `build.rs` running `git`, with no dependencies. Without git it
      falls back to a bare `X.Y.Z-dev`. A clean build of exactly the released sources shows
      the plain version (your decision of 2026-09-25): the release workflow, a clean checkout
      of the tag, or the crate from crates.io, whose commit comes from `.cargo_vcs_info.json`.
    - **Dirty** means uncommitted changes under `crates/`, `.cargo/`, the Cargo files or
      `rust-toolchain.toml`, the files that go into the binary. Edits to docs don't count.
13. **Hiding and colouring branches by name** (your request of 2026-09-25). Neither is in
    TortoiseGit. *Hide branches* (the toolbar's filter options, or *Settings → Filters*) takes
    wildcards such as `pipeline/*, release/*`; *Settings → Branch colours* holds rules such as
    `feature/*` → purple (first match wins). On
    Apps, hiding `pipeline/*, release/*` takes the graph from 167 to 97 nodes. Only one of the
    64 branches stays: `origin/pipeline/8/15749`, which three prototype and spike branches grow
    out of. Decisions you may want to overrule:
    - **Leaves only, as you asked.** A hidden branch that a shown branch's history contains
      keeps its node and its label. That includes a branch sitting on a commit of `main` that
      never got commits of its own. The alternative would drop such labels too.
    - **Branches only.** Tags, stash and other refs never match (tags have their own toggle).
    - **Matching:** `origin/release/1` matches both `release/*` and `origin/release/*`.
      `*` crosses slashes, `?` is one character, and case doesn't matter.
    - **The current branch** is never hidden, and stays red when a colour rule matches it.
    - **Remote branches get the same colour** as local ones. A paler shade for remotes would
      keep TortoiseGit's local/remote distinction (paler orange against green).
    - **Colours stay as picked in the dark theme.** The built-in colours are
      lightness-inverted there instead.
    - **Global, not per repository**, like the other settings. Both lists start empty.
    - **The filter options stay open** when you click inside them, so their text fields can
      be clicked into. A click outside or Esc closes them. Menus close on any click.
    - **`--hide` and `--branch-color`** replace the saved list or rules, like `--filter`.
      Like every command-line option, the change is saved when the window closes.
14. **Edge ends in Classic.** On your request, edges now always leave a node's bottom centre
    (towards its parents) and enter the top centre (from its children), in both looks, and
    arrowheads are 13 px instead of TortoiseGit's 8. TortoiseGit instead clips each edge where
    it meets the box border, so edges can end on any side. Should Classic keep TortoiseGit's
    clipping?
15. **App icon.** Decided 2026-09-25: the revision graph planted as a parterre, seen from
    above, on the dark theme's slate (variant C1). The prototype with every candidate, the
    verdicts and a head-to-head of the last two is on the branch `prototype/app-icon`
    (`packaging/icon-prototype/index.html`). Nothing is taken from the publisher's name. macOS
    26 could also take a dark appearance; nothing else can, so one icon serves everywhere.
16. **Toolbar, menu and settings** (reorganised 2026-09-26 after the prototype on the branch
    `prototype/menus`). Calls I made that the prototype didn't settle:
    - **Left out of the ☰ menu:** "Select subtree of selection" and "Return selection to
      layout". The right-click menu has both, and acts on the selection the node belongs to.
    - **Added to the ☰ menu:** *Find* (`Ctrl+F`) in the toolbar's order, and the version at
      the foot (question 12).
    - **Only in the ☰ menu and *Settings → Graph*:** stash, other refs and "tags make nodes".
      The toolbar's filter options keep the four filters you change most.
    - **Physics sliders are always enabled** (*Settings → Advanced*). They only affect Adapt,
      as their tooltips say; before, they were greyed out in the other modes.
    - **The toolbar no longer wraps.** In narrow windows the find field shrinks instead, and
      drops its `Ctrl+F` hint.
17. **Windows installer** (#15, `docs/building.md` → "Windows installer"). Tested on Windows 11:
    per-user and machine-wide installs, uninstalls, upgrades, same-version upgrades and a
    refused downgrade. Calls #15 didn't settle:
    - **MSI rather than MSIX** (your question of 2026-09-26). MSIX must be signed, and winget
      refuses unsigned ones; it installs per-user only, so Chocolatey's machine-wide install
      has no counterpart; and an Explorer entry (#11) would need a COM shell extension instead
      of a few registry keys. MSIX would bring clean sandboxed uninstalls and Store updates.
      Worth another look only with code signing.
    - **No installer UI.** A plain MSI shows only a progress bar, which suits winget and
      Chocolatey. Someone downloading it from GitHub sees no welcome or finish page. Adding one
      takes WiX's `WixToolset.UI.wixext` extension.
    - **Registry key** `Software\Trustfall AB\parterre` (in HKCU or HKLM), used only as the
      components' key paths, which Windows Installer needs under a user's profile.
    - **The Start menu entry** opens an empty window that asks for a repository, since #12
      (question 18). Before, started outside a repository, parterre showed nothing.
    - **Two ICE checks are suppressed:** ICE57, which doesn't understand dual-purpose packages,
      and ICE61, which warns about the same-version upgrades we want.
    - **Per-user and machine-wide don't replace each other.** Windows Installer only upgrades
      within one scope, so a user who installs per-user and later machine-wide (or the other
      way round) gets two entries in *Settings → Apps*. Known MSI behaviour, not tested.
18. **Opening folders** (your request of 2026-09-26). Without a path, parterre opens the
    current directory's repository, or else an empty window asking for one. The ☰ menu starts
    with *Open folder…* (`Ctrl+O`), *Recent folders* and *Close folder* (`Ctrl+W`), not in
    the toolbar. Decisions you may want to overrule:
    - **A path given that is not a repository** still ends with an error in a terminal, as
      before. Without one (Explorer's menu, a shortcut, a desktop entry) the empty window
      opens and shows the error instead, since #11 (question 21).
    - **The empty window also has an *Open folder…* button and the five most recent
      folders.** That is more than the message you asked for; the menu has the same.
    - **Recent folders:** the ten newest, each shown by name with the folder it is in (two
      `Apps` repositories stay apart). The open one is left out. The list also takes
      repositories opened from the command line or the current directory.
    - **A recent folder that fails to open leaves the list**, with the reason in the status
      bar, so that deleted repositories don't linger. A drive that is only unplugged loses
      its entries too.
    - **The folder picker** is the platform's own: Windows' dialog, macOS's, and on Linux the
      XDG desktop portal, or zenity where there is no portal (the `rfd` crate, without GTK).
      It starts in the folder around the open or most recent repository. Any folder inside a
      repository opens that repository.
    - **Items that need a repository** (undo, reload, export, close) are greyed out while none
      is open. The toolbar stays as it is.
19. **Log window layouts** (#40). #29 didn't settle:
    - **The picker** is four icon segments drawing each layout's panes, named in their
      tooltips, like the toolbar's drag modes. No keyboard shortcut for switching (the
      prototype's ← → keys were prototype chrome).
    - **Reset** is an icon button right of the picker. It moves only the current layout's
      dividers back, and is greyed out while they are where they start.
    - **In the settings** the same picker is a row "Layout" under a heading "Log window" at the
      bottom of *Appearance* (below the fold: the page scrolls), with the layout's name beside
      it. A page of its own for one row seemed too much.
    - **Smallest pane:** 8 % of the height, 15 % of the width (the panes are tables, which need
      room across). Starting positions are the prototype's.
    - **Narrow panes get narrower columns:** a commit list under 720 points wide gets the
      prototype's narrower author and date columns (as in its layouts B and D), and a
      changed-files table whose path would get under 260 points gets narrower columns with
      shorter headings ("Ext.", "Added", "Removed"; the full name in the tooltip). Decided by
      width, not by layout, so a narrow window in layout A gets them too.
20. **Log window** (#39, layout A). Calls #27–#29 didn't settle:
    - **Ref badges follow the graph's ref kinds.** The log shows badges (and names the range
      with refs) only of the kinds the graph shows: hide remote branches, other refs or the
      stash in the graph and they go from the log too. The alternative is every ref, always,
      which on Apps would add the `refs/t3/*` checkpoints.
    - **Esc in the filter field** only leaves the field; a second Esc closes the window.
    - **F5 in the log window** reloads the whole repository, graph included, as F5 in the
      graph does; the log re-runs its query and keeps the selected commit.
    - **Show log while the window is open** replaces its contents and asks the window manager
      to raise it (Wayland may ignore that). Sort and filter of the changed files, and the
      divider positions, carry over to the new log.
    - **Size on first open:** 1100 × 760; after that, the size it last had.
    - **No keyboard focus for the list:** the arrow keys, Page Up/Down, Home and End move the
      selected commit whenever the filter field doesn't have the keyboard.
21. **Explorer context menu** (#11). *Revision Graph* on a folder and on the
    background of an open one, registered by the MSI. Clicked through in Explorer on a folder
    (it opened the graph); on a folder's background only its keys and command were checked.
    Named *Revision Graph*, without the "(parterre)" #11 had, on your call of 2026-09-26: the
    icon says whose it is. Calls #11 didn't settle:
    - **Windows 11 shows it under *Show more options*.** At the top level it would need an
      `IExplorerCommand` handler and package identity (MSIX or a sparse package), as #11 said.
    - **Every folder gets the entry**, in or out of a repository: a plain registry verb
      can't ask git. Outside one, parterre opens with the error and *Open folder…*.
      TortoiseGit's shell extension can hide it; that takes a COM handler.
    - **Errors in a window only when there is no terminal** (stderr isn't one). From a
      terminal a bad path still prints the error and exits, and so does `--export`. Run with
      stderr redirected to a file, a bad path now opens the window too; `--screenshot` runs
      still fail. Other startup failures (no OpenGL, as over some remote desktops, or a
      panic) still reach only stderr, so from Explorer nothing would show.
    - **One folder at a time:** with several folders selected the entry is missing, rather
      than opening a window for each.
    - **Not on drives** (`Drive\shell`): right-clicking `C:` in *This PC* has no entry; the
      background of an open drive does. Easy to add if repositories at a drive's root matter.
    - **Not optional:** every install gets it; the MSI has no UI to leave it out.
    - **Linux:** not done. `MimeType=inode/directory` in the desktop entry would list
      parterre under *Open With* for folders, but some desktops then make it the default
      folder handler (VS Code had that bug), so it needs trying on GNOME and KDE first.

22. **Reloading automatically** (from the planned list, 2026-09-26). TortoiseGit reloads
    only on F5; parterre now also reloads by itself when the branches, tags or HEAD change, as
    after a commit, checkout or fetch in another program. Decisions you may want to overrule:
    - **On by default** (confirmed 2026-09-26). ☰ → *Reload automatically* and
      *Settings → Graph* turn it off.
    - **How it notices:** every second it looks at the files git keeps refs in (`HEAD`,
      `packed-refs`, `refs/`, reftable), without running git and without a file-watching
      crate. Only when they change, and have stayed unchanged for 0.3 s (so a rebase is loaded
      once, at its end), is the repository loaded again, on a worker thread. If refs and HEAD
      turn out the same (`git gc`, `git pack-refs`), nothing happens.
    - **Not during a drag:** a reload waits until the node is dropped.
    - **Moved nodes survive a reload,** also on F5, even with *Remember moved nodes* off. Before,
      F5 put every node back into the layout. Undo history does not survive.
    - The status bar says "Reloaded: the refs changed".

23. **PNG export.** ☰ → *Export* → *SVG…* or *PNG…* opens the system's save dialog, as
    TortoiseGit's "Save graph as..." does. Calls you may want to overrule:
    - **A submenu** rather than a file-type list in the save dialog (your request of
      2026-09-26: not two *Export* items). Such a list only works on Windows: rfd merges the
      types into one allowed list on macOS, which shows no list, and on Linux it never says
      which type was picked, while GNOME doesn't change the name's extension to match. A name
      without the right extension gets it added. The dialog starts in the folder exported to
      last, else next to the repository.
    - **Current zoom,** as in TortoiseGit, times the display scale (2 on a HiDPI screen), so
      the PNG looks as the window does. Zoomed out, labels under 4 px are left out, as on
      screen. SVG stays at 100%. `--export out.png` draws at 100%, or at `--zoom`. The status
      bar says the size and zoom after saving.
    - **Limits:** at most 100 megapixels and 65,535 px a side. A bigger graph is scaled down
      to fit, and the status bar says so, instead of failing (TortoiseGit says "not enough
      memory" when Windows can't make the bitmap). On Apps, "Labelled commits" fits at 100%
      in both looks (Classic: 24,232 × 3,745 px, 2.4 s); "Branchings and merges" comes out at
      61%, and "All commits" at 10%, too small to read. Drawing holds the whole image in
      memory (300 MB at the limit). Writing the PNG in bands would lift the limit, but needs
      the `png` crate directly (already built, as `image` uses it).
    - **Background:** the theme's, opaque. No transparent PNG.
    - **WebP** too (your request of 2026-09-26), which TortoiseGit doesn't write: lossless
      only (the encoder has no lossy mode), the same pixels as PNG in a file about 40%
      smaller (Apps: 1.0 against 1.8 MB). WebP allows at most 16,383 px a side, so Apps in
      the Classic look comes out at 68%. Adds the `image-webp` crate, +320 KB.
    - **Other formats:** no JPEG (+255 KB; blurry fringes round the labels, often bigger
      than the PNG), BMP or GIF, which TortoiseGit also writes. `--export` refuses other
      extensions; before, it wrote SVG whatever the name. A name without an extension still
      gets SVG.

24. **Log search** (wanted 2026-09-26, "super-useful"; to discuss, not decided yet). Filters in
    the log window by date range, author and text, and perhaps a log of the whole repository
    opened without a node. Ruled out of the first log window
    ([#27](https://github.com/aquamoth/parterre/issues/27)); the log query is shaped so these
    can be added as new fields and callers. Which searches, and when?

25. **Pull requests on GitHub, slice 1** (research §12 and §14). Not in TortoiseGit. Tried on
    a commits-only clone of `cli/cli`: 63 open pull requests, 25 of them from its own branches,
    shown; the 38 from forks need slice 2. Calls you may want to overrule:
    - **Asked for as t3code does** (your call of 2026-09-26, after looking at how t3code
      gets its pull requests), so that many users don't weigh on GitHub:
      - **Only signed in.** With the token of a signed-in `gh` (`gh auth token`), or not at
        all: without one, GitHub is never asked. `GH_TOKEN` and `git credential fill` (§6) are
        left for later.
      - **Per branch, only fetched ones.** One GraphQL request per 100 branches of `origin`
        fetched here, with a `pullRequests(headRefName:)` connection of 100 per branch on
        `origin` and on its parent at once, and only the fields shown. As in t3code (`gh pr
        list --head`), the branch name is matched by GitHub and the head repository here,
        since another fork's `main` isn't ours. Only a fetched branch's pull request can be
        shown, so nothing showable is missed. No branch fetched: no request. Your fork of
        t3code: 1 request, 0.6 s, 1 point; asking the parent for all its open pull requests by
        REST took 15 requests, 30 MB and 23 s. `cli/cli` (255 branches): 3 requests.
      - **Cached per repository** for a minute when it had pull requests, five when not, as
        t3code. Asked again only after that, and only when the repository is opened again
        or its refs change (a fetch, a push). F5 and turning them on always ask. No polling
        (t3code polls every 30 s).
      - **Backing off after failures,** 20 s doubling to 15 min, keeping the last list shown.
        The button's tooltip says what went wrong last.
      - **A budget:** once fewer than a tenth of the hour's points are left (t3code's
        reserve), or GitHub says to wait (`Retry-After`), nothing is asked until the limit
        resets, for any repository.
    - **On by default, and quiet** (your call of 2026-09-26): nothing in the status bar
      unless you asked. §12 had the button greyed out until the list had loaded; instead:
      - **`origin` not on GitHub** (git alone tells): the button and ☰ → *Show → Pull
        requests* are greyed out, with a tooltip saying why. Never a message.
      - **`gh` missing or not signed in:** on by default they fail quietly, and the button
        looks off, as nothing can be shown; its tooltip says why. They appear by themselves
        once `gh` is signed in, at the next opening or ref change.
      - **You turn them on** (the button, the menu or *Settings → Graph*) **and it fails:** a
        dialog says what is wrong and what to do: for a missing `gh` with a button to its
        installation page, for signing in with one to copy `gh auth login`. It doesn't rely
        on the status bar, which may be hidden. On success the status bar counts them.
      - Loads parterre makes by itself (opening, refs changing, F5) never write to the
        status bar, even on success; the tooltip keeps the last error.

      Settings saved while they were off by default keep them off. On for one repository is
      on for all, like the other settings. `--export` never asks GitHub.
    - **Which repository:** `origin` only, as §12 says; gh's `gh-resolved` and remote ranking
      (§7) aren't used. A renamed repository is followed under its new name.
    - **Base branch shown** means: a remote-tracking branch of the base branch, in any remote
      pointing at the pull request's repository, or a local branch whose upstream that is.
      So with remote branches hidden, `main` tracking `origin/main` still counts.
      A fork's own pull requests into its parent have their base branch in the parent, so
      they show only if a remote (such as `upstream`) points at the parent: a clone with only
      `origin` doesn't show them. Treating `origin/main` as the parent's `main` would be a
      guess.
    - **The label:** a row below the node's refs with the number right-aligned (your request
      of 2026-09-26) after the pull-request glyph, pale blue, drafts pale grey (both
      lightness-inverted in the dark theme). Hovering underlines the number, as a link. A
      commit whose only label is a pull request shows no hash, as with a ref. Several on one
      commit get a row each.
    - **Clicking the number opens the pull request** and also selects the node; Ctrl or Shift
      clicks only select, and a double-click opens it once and no log. Hovering shows a hand
      cursor and the title, author, draft state and `branch into base` (`owner:branch` for a
      fork's). The node menu gets *Open pull request #N* under *Show log*, greyed out on
      nodes without one while pull requests are shown.
    - **The browser** is started with `xdg-open`, `open` or `explorer.exe`, as §14 says, and
      only for `https://github.com/` pages that parterre builds itself from the repository
      and number (not the API's `html_url`). Not tried by hand on any platform.
    - **Not in find or the log window:** find doesn't match pull requests' numbers or titles,
      and the log shows no pull requests. Exports (SVG, PNG, WebP) draw the labels as shown,
      glyph included.
    - **Cost:** `ureq` with rustls and ring, plus `serde_json`: 19 crates on Linux (20 on
      Windows, 23 on macOS) and 2.0 MB (the stripped Linux binary goes from 13.4 to 15.5 MB).
      ureq is behind the `github` cargo feature, on by default; without it the button stays,
      and loading says the build has no GitHub support. ring compiles C and assembly, so building
      parterre (`cargo install` too) now needs a C compiler for the target: there already on
      the CI runners, and on Linux almost always; the Windows cross-check from Linux needs
      MinGW-w64 or `--no-default-features` (`docs/building.md`).
    - **Certificates come from the system** (`rustls-platform-verifier`), not from Mozilla's
      list bundled in, as ureq does by default. Same size, and it works behind company
      proxies that inspect TLS with their own certificate authority. The bundled list is
      licensed CDLA-Permissive-2.0, which `packaging/about.toml` doesn't accept. That file
      now lists the release targets, as the platform verifier needs the list on wasm32 only.
      Only tried on Linux.

## Planned

- [ ] Reopen a repository at the zoom and position it last had, stored per repository like
      the remembered moves; without a stored view, 100% with the current branch near the top,
      as today (question 4, decided 2026-09-26).
- [ ] Text size with `Ctrl`+wheel in every window besides the graph: the log window, the
      diff windows and the settings, as the graph zooms with it (wanted 2026-09-26). Probably
      also `Ctrl`+`+`/`-`/`0`. Open: one size shared by all these windows or one each, and
      whether it is remembered with the settings.
- [ ] File diffs from the log window, being charted in the map *Roadmap to TortoiseGit
      parity: revision graph and log* ([#25](https://github.com/aquamoth/parterre/issues/25)):
      read-only, one diff window per file, diffed by `imara-diff` (lines and words; decided in
      [#44](https://github.com/aquamoth/parterre/issues/44); what the window shows and does in
      [#45](https://github.com/aquamoth/parterre/issues/45)). Blame is a stretch goal.
  - [ ] Optional, deferred: *Open in external diff tool*, handing both versions to the user's
        configured diff tool, as TortoiseGit does by default. Not behind the built-in view;
        decided in [#44](https://github.com/aquamoth/parterre/issues/44).
  - [ ] Optional, if users ask: a choice of diff engine, e.g. git's own patch (honouring
        `diff.algorithm`) beside the default `imara-diff`
        ([#44](https://github.com/aquamoth/parterre/issues/44)).
  - [ ] Optional: honour git's `encoding` attribute (the one gitk uses) to decode non-UTF-8
        files, e.g. with `encoding_rs` (+191 KiB, 4 crates). Until then, invalid UTF-8 shows
        as `\xNN` ([#44](https://github.com/aquamoth/parterre/issues/44)).
  - [x] File diff core, diff window, selection and copying lines
        ([#51](https://github.com/aquamoth/parterre/issues/51),
        [#52](https://github.com/aquamoth/parterre/issues/52),
        [#53](https://github.com/aquamoth/parterre/issues/53)). Deliberate deviations from
        TortoiseGit (TortoiseGitMerge), decided in #45 and #46:
    - unchanged stretches are folded by default (TortoiseGitMerge's "Collapse" is off);
    - line endings count by default, and "Ignore whitespace changes" ignores them too, with a
      note saying so (TortoiseGitMerge ignores line endings by default, silently);
    - changed words pair each removed line with the most similar added line, and the
      pairing can be switched (TortoiseGitMerge compares lines by position only);
    - the change marks live in an overview strip on the right, not a locator bar on the
      left; it scrolls on click, as the locator bar does.
  - [ ] Stretch goal: wrap long lines, as a toggle in the diff window's toolbar. Until then
        long lines scroll sideways ([#45](https://github.com/aquamoth/parterre/issues/45)).
  - [x] Free text selection in either pane, copied as in the file (wanted 2026-09-26).
  - [ ] Optional: find in a diff window (Ctrl+F)
        ([#45](https://github.com/aquamoth/parterre/issues/45)).
  - [ ] Another day: open a diff from outside parterre, e.g. right-click an edited file in the
        file manager and diff it with its previous commit
        ([#45](https://github.com/aquamoth/parterre/issues/45)).
- [ ] Wayland freeze workaround (#38, see Done). **Check regularly, and on every eframe
      upgrade, whether the upstream fix has shipped:**
      <https://github.com/emilk/egui/pull/8631> (bug:
      <https://github.com/emilk/egui/issues/5145>). Once it is in a released eframe, remove
      the frame cap and turn vsync back on.
- [ ] Toolbar merged into the title bar, with ☰, the repository name and the window buttons in
      one row (wanted 2026-09-26, postponed as too big a change for now). Native on macOS
      (content under a transparent title bar, the traffic lights stay). Elsewhere parterre
      would draw its own title bar: moving, resizing and double-click to maximise by hand; no
      Windows 11 snap-layout popup; on GNOME no compositor shadow. See the "Title bar: merged"
      toggle in the prototype on the branch `prototype/menus`.
- [ ] Distribution, as decided in `docs/distribution.md`: crates.io (#13), Windows MSI (#15),
      winget (#16), Chocolatey (#17), .deb and .rpm (#18), Snap (#19), publishing behind one
      approval (#20), Flathub later (#21). The MSI (#15) is built by CI and attached to
      releases, and parterre finds Git for Windows when git isn't on PATH (question 17).
- [x] Explorer context menu (#11; see question 21).
- [ ] macOS `.app` bundle, so the Dock shows `packaging/icon/parterre.icns`; the release ships
      a bare binary, which gets the generic icon.
- [ ] Open GitHub PRs in the graph (`docs/research/github-forks-and-pull-requests.md`, §12,
      §14). TortoiseGit has no such feature.
  - [x] Slice 1: PR-icon tags on nodes whose commit is a PR head, opening the PR in the browser;
        a toolbar toggle, disabled without a GitHub connection; only PRs of `origin` (plus
        the fork's own PRs into its parent) whose base branch is visible. No fetching.
  - [ ] Slice 2: fetch other PR heads commits-only into a private cache; greyed-out nodes,
        dashed edges.
  - [ ] Low priority: check slice 1 by hand on macOS and Windows (only Linux was tried).
        What is drawn is the same everywhere (egui draws it all); what runs outside it isn't:
    - **macOS, likely a bug:** started from the Dock or Finder, parterre doesn't get the
      shell's `PATH`, so a Homebrew `gh` (`/opt/homebrew/bin`, `/usr/local/bin` on Intel)
      isn't found and pull requests say "GitHub CLI not found". Likely fix: also look in
      those folders, as `git/program.rs` does for Git for Windows. Check: start from the
      Dock with `gh` signed in, see the labels.
    - **Windows:** no console window flashing when `gh auth token` runs (`CREATE_NO_WINDOW`
      is set); a parterre started before `gh` was installed keeps the old `PATH` (the same
      case `git/program.rs` handles for git); started from the Start menu and from Explorer's
      *Revision Graph*.
    - **Both:** clicking a number opens the browser (`open`, `explorer.exe`), and GitHub's
      certificate is accepted through the system's store (Keychain, schannel) by
      `rustls-platform-verifier`.
  - [ ] Later, only when requested: pull requests of Azure DevOps origins. Findings and
        estimate (about the core half of slice 1, no new crates; sign in through Git
        Credential Manager with `git credential fill`): the research doc, §15.
- [ ] Menus that overflow the window on Windows, like TortoiseGit's native ones: each menu (and
      submenu) as a borderless egui viewport placed in screen coordinates, kept on the monitor
      by sliding up from its bottom edge. Needs a Windows agent to build and try it; watch
      for the main window losing focus while a menu is open, and find the monitor's work area
      for multi-monitor setups. Hover-to-open submenus, closing on outside clicks and the
      keyboard then work across windows, so egui's menu logic has to be redone. For now menus
      stay inside the window and scroll (`menu::fit_window`). Not planned: Wayland (winit 0.30
      has no xdg_popup, and a client can't place its own windows), macOS (no agent to test
      on), X11 (possible with override-redirect windows, but few users).

## Done

- [x] License: GPL-3.0-only plus section 7 attribution terms, an About dialog showing them,
      and `THIRD-PARTY-NOTICES.html` (cargo-about) in the CI artifacts.
- [x] Workspace scaffold, lints, release profile, docs (`docs/architecture.md`,
      `docs/building.md`).
- [x] Research into how TortoiseGit's revision graph works (`docs/research/`).
- [x] Git loading through the git CLI (about 100 ms for 15k commits).
- [x] Revision-graph reduction in TortoiseGit's modes, with integration tests; matches
      `git log --simplify-by-decoration` exactly on Apps.
- [x] Layered layout:
  - network-simplex ranking, median crossing reduction, L1 coordinates
  - variable layer spacing
  - splitting of over-wide rows
  - optional edge bundling
  - four directions
- [x] Window: TortoiseGit colours and node geometry, light and dark themes, straight or
      curved edges with arrows, pan and zoom, fit, go to HEAD, search, tooltips, context
      menu, overview map, persisted settings, status bar.
- [x] Draggable nodes with spider-web physics, in three models. The net's shape minimises
      spring and anchor energy over displacements; only the dragged node's neighbourhood is
      simulated. Pinning and reset.
- [x] Windows type-check (`cargo check --target x86_64-pc-windows-gnu --no-default-features`;
      with the `github` feature it needs MinGW-w64, see `docs/building.md`).
- [x] Native Windows build (MSVC) with a statically linked C runtime; tests, clippy and
      screenshots pass on Windows.
- [x] Terminal output from the Windows release build: attach to the parent console, release it
      before an interactive window opens. `unsafe_code` is `deny` (was `forbid`) so this one
      call can opt out.
- [x] Crossing reduction with transposition and 12 restarts, as OGDF does: 16–30% fewer
      crossings.
- [x] Layout on a background thread, with the view kept anchored on the same commit.
- [x] SVG export from the menu, plus headless `--export out.svg`.
- [x] Filters: current branch only, and a ref-name filter.
- [x] Full commit messages in tooltips, loaded on demand.
- [x] Window icon drawn in code; Linux `.desktop` entry; pre-commit hook (fmt and clippy).
- [x] App icon (`parterre-core::icon`): the window icon, the SVG, PNGs, the `.ico` embedded in
      the Windows `.exe` and the `.icns` are all generated from one drawing.
- [x] Version information in the Windows `.exe` (*Properties → Details*): product name,
      versions, copyright and Trustfall AB as the company.
- [x] Hovering an edge lists the commits collapsed into it. Help → Legend explains the colours.
- [x] Independent code review. Fixed:
  - a crash when reloading after deleting a branch or tag
  - loading failures on odd characters in subjects or names
  - lost labels on tags of tags
  - swapped nodes after a reset
  - long edges when splitting rows
  - a race between refs and log during reload
  - debug-build panics on cyclic input

  Its randomised tests are now permanent property tests.
- [x] Demo repository script (`scripts/make-demo-repo.sh`) and a README screenshot.
- [x] Rearranging by hand (question 2):
  - drops become the new resting shape instead of pins
  - drag modes Adapt (springs and weak magnets), Free and Subtree
  - multi-selection with rectangle selection, and "Select subtree"
  - undo and redo; remembered positions per repository
  - edges re-route through the gaps between rows as nodes are moved
- [x] Tag-driven releases (question 12): pushing `vX.Y.Z` builds Linux, Windows and macOS
      archives and publishes a GitHub Release. The build fails unless the tag matches
      `Cargo.toml`. `--version` and the ☰ menu read `0.3.0 (a1b2c3d)` for releases and
      `0.3.0-dev+a1b2c3d` for every other build.
- [x] Hiding branches by wildcard, leaves only, and colours by branch name (question 13).
      Available in the menus and as `--hide` and `--branch-color`. The status bar counts the
      hidden branches, and the Legend lists the colour rules.
- [x] Direction made visible:
  - edges leave the bottom of a node and enter the top; an edge turned around loops round its
    nodes
  - bigger arrowheads
  - Adapt keeps children above parents (about 1 ms more per frame with 8000 particles awake)
  - click an edge to keep it highlighted, also in the overview; the status bar says where it
    leads
  - the blue dot is gone
- [x] Context menu restyled after current desktop menus: rounded, soft shadow, roomier rows
      with a rounded highlight, shortcuts on the right, unavailable items greyed out rather
      than left out. "Follow system" now follows the desktop's light or dark mode on Linux
      too, switching as soon as the desktop does (XDG desktop portal, via `gdbus`); winit
      reports no system theme there, so it used to be dark always.
- [x] Title bar: on GNOME (Wayland desktops that leave it to the app) winit's Adwaita-style
      title bar with the window title and round buttons, instead of a plain dark bar. The
      title bar follows parterre's light or dark theme, also on Windows and macOS.
- [x] Opening and closing folders from the ☰ menu, with recent folders (#12, question 18).
      Without a path, the current directory's repository or an empty window that asks for one.
- [x] Toolbar, ☰ menu and settings reorganised (question 16): icon tools for what to show, the
      ref toggles with filter options, find, zoom, HEAD, the overview map and the drag modes
      with their options; everything again in the ☰ menu, in the toolbar's order; the rest in
      a settings window that leaves the graph visible and applies changes at once. The status
      bar can be hidden, and no longer shows the layout time or the drag mode's description.
- [x] A menu or popover taller than the window scrolls, with a visible thin scroll bar, instead
      of being cut off at the bottom; one that fits but not below its button slides up to the
      window's bottom edge, as before.
- [x] Show log window, as planned in the map *Roadmap to TortoiseGit parity: revision graph
      and log* ([#25](https://github.com/aquamoth/parterre/issues/25)). Deliberate deviation
      from TortoiseGit (decided in #28): when the second of two selected nodes is an ancestor
      of the first, the two are swapped instead of showing an empty list.
  - [x] Layout A (stacked) and its entry points: *Show log* first in the node menu, `L` and
        double-click ([#39](https://github.com/aquamoth/parterre/issues/39); see question 20).
  - [x] Layouts B, C and D, the layout picker (in the window's header and in *Settings →
        Appearance*) and reset, and the layout and divider positions per layout saved with the
        settings ([#40](https://github.com/aquamoth/parterre/issues/40); see question 19).
- [x] Wayland freeze ([#38](https://github.com/aquamoth/parterre/issues/38)). On Wayland the
      whole app froze when one of its windows was minimized while another was open; it
      happened with Settings already. Worked around (see `frame_pacing.rs`, and
      `docs/research/wayland-viewport-freeze.md` on the branch
      `research/wayland-viewport-freeze`): on Wayland only, vsync off and frames capped at about
      8 ms. Verified by hand on GNOME Wayland on 2026-09-26, with the log and diff windows.
- [x] Settings window without minimize and maximize buttons. It is a dialog, and maximizing it
      breaks its layout. Asked of winit, which (0.30) does this on Windows and macOS only. On
      Linux it ignores the request: there the window loses only its maximize button, because it
      can't be resized (winit's own Wayland title bar, as on GNOME, leaves maximize out, and X11
      window managers get a "not maximizable" hint), and minimize stays. Not checked by hand on
      any platform.
- [x] Reloading automatically when a commit, checkout or fetch outside parterre changes the
      refs or HEAD (question 22); TortoiseGit reloads only on F5. The ref files are looked at
      every second, without running git; moved nodes and the selection survive a reload.
- [x] Short hashes in the graph as long as git makes them for the repository (`core.abbrev`,
      9 on Apps), like the log window: node labels, tooltips, the status bar and the SVG
      export. Deliberate deviation from TortoiseGit, which always shows 8.
- [x] A reset no longer leaves an edge with a route of its own. An edge re-routed round a node
      was looked at again only when the node's new place touched the new route, so a node that
      jumped clear in one frame, or that covered the layout route away from the new one, left
      it routed. Now the node's old place counts too, and the layout route as well as the new
      one. `physics_random_drags` takes `PHYSICS_SEED` and `PHYSICS_ITERS`.
- [x] Less memory for all-commits views of huge repositories (compact adjacency): the layout's
      and the drag net's neighbour lists are stored flat, which took the 100k-commit
      all-commits view from 900 to 740 MB and its layout from about 3 s to 2 s. What is left
      is mostly the drag net (about 170 bytes for each of 1.6M particles); question 11.
- [x] Open pull requests on GitHub as labels on their head commits, slice 1 (question 25):
      a toolbar toggle (☰ → *Show* and *Settings → Graph* too), click or the node menu to open
      one in the browser, `--pull-requests`. Only commits already fetched; no fetching yet.
- [x] PNG and WebP export (question 23): ☰ → *Export* → *PNG…* or *WebP…*, and
      `--export out.png` or `out.webp` (with `--zoom`). Drawn by the window's own painting
      code, rasterised without a GPU, so labels look as on screen. Every export now uses the
      system's save dialog instead of a path field.
