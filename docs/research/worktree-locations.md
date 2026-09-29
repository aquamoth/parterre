# Where tools put new worktrees

Research note for [#138](https://github.com/aquamoth/parterre/issues/138): *where do tools that
create git worktrees put them by default, and why? Above all t3code.* It feeds the decision
*Where new worktrees go* on [#137](https://github.com/aquamoth/parterre/issues/137), whose
tentative choice is `parterre.worktreeRoot` in git config, global or per repository.

Sources are official docs, source code at pinned commits, and issues and PRs in the tools' own
repositories. A statement that is my own conclusion is marked **(derived)**. A statement I could
not check is marked **(unverified)**. Researched 2026-09-29.

## TL;DR

- **t3code** puts every worktree in `~/.t3/worktrees/<repo folder>/<branch, "/"→"-">`. There is
  no location setting. The folder moves only with the whole T3 home (`T3CODE_HOME` or
  `--base-dir`). Users keep asking for a configurable location, citing direnv/nix and the sibling
  `<repo>.worktrees/` layout. The maintainers have closed four PRs for it because of cleanup and
  collision risk, and one per-server setting PR is still open.
- **The most common default among git clients is a sibling folder `<repo>.worktrees/<name>`**
  (VS Code, GitLens, and reportedly GitKraken Desktop). lazygit offers the folder that existing
  worktrees already use. AI agent tools (t3code, Codex, Conductor) use a central app folder, and
  Claude Code uses a folder inside the repository.
- **git has no default and no config key for a location.** `git worktree add` needs a path and
  refuses a folder that exists and isn't empty.
- **No tool stores the location in git config.** Each uses its own settings: VS Code settings per
  folder (GitLens), lazygit's config (global or per repository), app settings (Codex).
  `parterre.worktreeRoot` would be new, but it is a legal git config key and gives global and
  per-repository levels for free.

## t3code

Source: [pingdotgg/t3code](https://github.com/pingdotgg/t3code) at
[`d2c9281`](https://github.com/pingdotgg/t3code/tree/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb)
(`main`, 2026-09-29). Every source link below is a permalink to that commit.

### Default location

`<T3 home>/worktrees/<repo folder name>/<branch with "/" replaced by "-">`.

- **T3 home** is `~/.t3` ([`apps/server/src/os-jank.ts#L105-L111`][os-jank]). It moves only with
  the `--base-dir` flag or the `T3CODE_HOME` environment variable
  ([`apps/server/src/cli/config.ts#L40-L42`][cli-flag], [`#L130`][cli-env],
  [`#L316-L330`][cli-resolve]).
- The worktrees folder is derived from it, next to T3's other state (`userdata/`, `caches/`):
  `worktreesDir: join(baseDir, "worktrees")`
  ([`apps/server/src/config.ts#L152`][config-dir]). It is created at startup
  ([`#L178`][config-mkdir]).
- The path is built in `createWorktree`
  ([`apps/server/src/vcs/GitVcsDriverCore.ts#L3065-L3074`][create]):

  ```ts
  const targetBranch = input.newRefName ?? input.refName;
  const sanitizedBranch = targetBranch.replace(/\//g, "-");
  const repoName = path.basename(input.cwd);
  const worktreePath = input.path ?? path.join(worktreesDir, repoName, sanitizedBranch);
  ```

  then runs `git worktree add [-b <new>] <path> <ref>`.
- Every caller that creates a new worktree passes `path: null`, so the default always applies: a
  new thread ([`apps/server/src/ws.ts#L1499-L1506`][ws-create]) and a PR opened as a thread
  ([`apps/server/src/git/GitManager.ts#L2578-L2583`][pr-create]). The only caller with a path
  recreates a thread's worktree that was deleted, at its recorded path
  ([`apps/server/src/orchestration/Layers/ProviderCommandReactor.ts#L504-L514`][recreate]).
  The RPC `vcs.createWorktree` does accept an optional `path`
  ([`packages/contracts/src/git.ts#L140-L146`][contract]).

### Folder names

- The folder is named after the branch at creation time, with `/` turned into `-`. Nothing else is
  escaped.
- A new thread first gets a temporary branch `t3code/<8 hex>`
  ([`packages/shared/src/git.ts#L13-L20`][tmp-branch], [`#L95-L105`][tmp-build]), so its folder
  is `t3code-<8 hex>`. After the first turn, the branch is renamed to `t3code/<generated name>`
  ([`ProviderCommandReactor.ts#L190-L211`][gen-name], [`#L921-L949`][rename]). The folder is not
  renamed. For example this note was written from `~/.t3/worktrees/parterre/t3code-f72e56e2`, on
  branch `t3code/plan-git-tree-management`.
- A worktree for a PR is named after its local PR branch, `t3code/pr-<N>/<suffix>`
  ([`GitManager.ts#L285`][pr-branch]), so `t3code-pr-<N>-<suffix>`.

### Collisions

- There is no collision handling in `createWorktree`. If the folder exists and isn't empty,
  `git worktree add` fails and t3code reports "git worktree add failed"
  ([`GitVcsDriverCore.ts#L3079-L3085`][create-run]).
- The random 8-hex branch token makes collisions unlikely for new threads **(derived)**.
- **(derived)** Two things can still collide: branches `a/b` and `a-b` map to the same folder, and
  two repositories with the same folder name (two clones called `app`) share
  `worktrees/app/`.
- A folder deleted by hand leaves git's admin entry behind, which makes `git worktree add` refuse
  that path. t3code runs `git worktree prune` first when it recreates a worktree
  ([`ProviderCommandReactor.ts#L504-L514`][recreate]) and after a failed remove
  ([`GitVcsDriverCore.ts#L3462-L3471`][remove]).

### Where the setting lives

- **There is no worktree-location setting.** The location follows from T3 home, which is a
  process-level flag or environment variable for the whole T3 server, not per repository. It is
  not stored in git config.
- T3's other worktree settings (`worktreeCleanup`, `newWorktreesStartFromOrigin`,
  `worktreeSubmodules`) are in its own `settings.json` under `~/.t3/userdata/`
  ([`config.ts#L149`][settings-path]). They resolve as project override, then environment
  setting, then the repository's `t3.json`, then the built-in default
  ([`docs/user/project-settings.md#L53-L63`][doc-settings]). No location key exists among them.

### Reasoning, from issues and PRs

The fixed location is contested. Users have asked for a configurable location in two issues and
six PRs. The maintainers closed four of those PRs, and two are still open.

What users want, and why:

- [#1878](https://github.com/pingdotgg/t3code/issues/1878) *configurable worktree location*:
  worktrees as children of the main project folder, because folder-based tooling (direnv, nix
  flakes) configured on parent directories doesn't reach `~/.t3/worktrees`. A comment adds
  Windows: Dev Drives (ReFS) and drives excluded from antivirus scanning.
- [#6413](https://github.com/pingdotgg/t3code/issues/6413) *Per-project worktree location*:
  the same tooling argument (direnv, nix, mise, docker mounts, monorepo tools), and "I prefer to
  keep all my worktrees in a single location in a sibling `<repo>.worktrees/` folder which mainly
  comes from VS Code … but also other Git clients follow this format as well". It notes that
  `T3CODE_HOME` is "too blunt" because it moves all T3 state, not just worktrees.
- [#10810](https://github.com/pingdotgg/t3code/issues/10810) (open) suggests
  `$XDG_STATE_HOME/t3/worktrees` on Linux, because managed worktrees are "persistent
  machine/project-specific application state rather than cache", while still wanting them
  configurable "because they can contain user changes".

What the maintainers said:

- [PR #1926](https://github.com/pingdotgg/t3code/pull/1926) offered four modes (global default,
  inside the project, sibling of the project, custom template). Closed 2026-07-20: "configurable
  worktree location is not planned in this form against the current workspace lifecycle."
- [PR #4439](https://github.com/pingdotgg/t3code/pull/4439) offered a path template
  (`{worktreesDir}/{repoName}/{branch}` by default, `{repoRoot}/.worktrees/{branch}` for
  repository-local). Closed 2026-08-28: "Configurable worktree placement changes path ownership,
  cleanup, revival, and collision behavior. That risk is too large for a location preference
  while [worktree inventory and cleanup] is being handled."
- [PR #6427](https://github.com/pingdotgg/t3code/pull/6427) (per-project location) was closed the
  same day: "We are not adding this new worktree configuration path through the current backlog."
- [PR #9681](https://github.com/pingdotgg/t3code/pull/9681) (per-checkout location, stored on the
  project record) was closed 2026-09-19 because the server layers it touches are being rewritten.
- [PR #10589](https://github.com/pingdotgg/t3code/pull/10589) (open) adds a per-server
  `worktreeBaseDirectory` setting. A maintainer rebased it on 2026-09-21 and asked only that it
  use the normal settings scope toggles. It refuses the home directory, or any folder containing
  it, as the worktree root. [PR #12009](https://github.com/pingdotgg/t3code/pull/12009) (open) is
  a similar environment-level setting.

A non-obvious reason from those PRs: t3code's diff review only accepts a working directory inside
the project or inside `~/.t3/worktrees`, so a new location has to widen a security check
([#6413](https://github.com/pingdotgg/t3code/issues/6413), [PR #10589](https://github.com/pingdotgg/t3code/pull/10589)).
Parterre has no such check **(derived)**.

[os-jank]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/apps/server/src/os-jank.ts#L105-L111
[cli-flag]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/apps/server/src/cli/config.ts#L40-L42
[cli-env]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/apps/server/src/cli/config.ts#L130
[cli-resolve]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/apps/server/src/cli/config.ts#L316-L330
[config-dir]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/apps/server/src/config.ts#L152
[config-mkdir]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/apps/server/src/config.ts#L178
[settings-path]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/apps/server/src/config.ts#L149
[create]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/apps/server/src/vcs/GitVcsDriverCore.ts#L3065-L3074
[create-run]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/apps/server/src/vcs/GitVcsDriverCore.ts#L3079-L3085
[remove]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/apps/server/src/vcs/GitVcsDriverCore.ts#L3462-L3471
[ws-create]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/apps/server/src/ws.ts#L1499-L1506
[pr-create]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/apps/server/src/git/GitManager.ts#L2578-L2583
[pr-branch]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/apps/server/src/git/GitManager.ts#L285
[recreate]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/apps/server/src/orchestration/Layers/ProviderCommandReactor.ts#L504-L514
[gen-name]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/apps/server/src/orchestration/Layers/ProviderCommandReactor.ts#L190-L211
[rename]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/apps/server/src/orchestration/Layers/ProviderCommandReactor.ts#L921-L949
[contract]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/packages/contracts/src/git.ts#L140-L146
[tmp-branch]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/packages/shared/src/git.ts#L13-L20
[tmp-build]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/packages/shared/src/git.ts#L95-L105
[doc-settings]: https://github.com/pingdotgg/t3code/blob/d2c9281b8112dc3b2991642c4bdb985e4b08b9bb/docs/user/project-settings.md#L53-L63

## git itself

Source: git at
[`a018953`](https://github.com/git/git/tree/a018953688f1b10bddf91bff8747068f5f4746a4) and
[git-worktree(1)](https://git-scm.com/docs/git-worktree).

- **No default location.** `git worktree add <path> [<commit-ish>]` requires a path. The docs'
  example uses a sibling folder, `git worktree add -b emergency-fix ../temp master`.
- **No config key for a location.** The only `worktree.*` keys are `worktree.guessRemote` and
  `worktree.useRelativePaths`
  ([`Documentation/config/worktree.adoc`](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/Documentation/config/worktree.adoc)).
- **Naming:** with only a path, the new branch is named after the path's last folder. It is created
  from HEAD if it doesn't exist and checked out if it does. If that branch is checked out in
  another worktree, git refuses unless `--force` is given.
- **Collisions:** git stops with `'<path>' already exists` if the path exists and isn't empty. An
  empty folder is fine
  ([`builtin/worktree.c#L314-L323`](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/worktree.c#L314-L323)).
  A path that is registered but missing needs `--force`, or `--force --force` if it is locked.
  The admin folder `.git/worktrees/<name>` gets a number appended if the name is taken
  ([`#L492-L512`](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/builtin/worktree.c#L492-L512)).

## VS Code (built-in git extension)

Source: microsoft/vscode at `251bcf5f`, `extensions/git/src/`.

- **Default:** `<parent of repo>/<repo>.worktrees/<name>`
  ([`repository.ts#L1959-L1968`](https://github.com/microsoft/vscode/blob/251bcf5f/extensions/git/src/repository.ts#L1959-L1968)).
  If the repository is itself a linked worktree, the default is a plain sibling
  `<parent>/<name>`
  ([`commands.ts#L3662-L3668`](https://github.com/microsoft/vscode/blob/251bcf5f/extensions/git/src/commands.ts#L3662-L3668)).
  The user gets an editable, prefilled input box and a folder picker.
- **Naming:** the branch name, with the `git.branchPrefix` setting removed and `/` turned into
  `-`, so `feature/foo` becomes `feature-foo`
  ([`commands.ts#L3536-L3539`](https://github.com/microsoft/vscode/blob/251bcf5f/extensions/git/src/commands.ts#L3536-L3539)).
- **Collisions:** if a registered worktree already has that path, VS Code appends `-1`, `-2`, …
  ([`repository.ts#L1970-L1978`](https://github.com/microsoft/vscode/blob/251bcf5f/extensions/git/src/repository.ts#L1970-L1978)).
  A non-empty folder that isn't a worktree fails in git, and a dialog offers to open it.
- **Where the setting lives:** there is no setting. The last root used is remembered in the
  extension's own storage under `worktreeRoot:<repo root>`
  ([`repository.ts#L712`](https://github.com/microsoft/vscode/blob/251bcf5f/extensions/git/src/repository.ts#L712)).
  It seems to be written only when a value already exists, so in practice it may never be set
  **(unverified)**.
- **Reasoning:** the first version (PR
  [#255945](https://github.com/microsoft/vscode/pull/255945)) always asked for a path. The
  `.worktrees` default came in PR [#257603](https://github.com/microsoft/vscode/pull/257603),
  without a stated reason.
- VS Code's agent sessions also use `<repo>.worktrees`
  ([`src/vs/platform/agentHost/common/worktreePaths.ts#L10-L21`](https://github.com/microsoft/vscode/blob/251bcf5f/src/vs/platform/agentHost/common/worktreePaths.ts#L10-L21)).
  Its comment says the fixed folder name is what the workspace-trust check looks for.

## GitLens and GitKraken

Source: gitkraken/vscode-gitlens at `8b8814e1`.

- **GitLens setting:** `gitlens.worktrees.defaultLocation`, default `null`, scope `resource`, so it
  can be set per user, per workspace or per folder in VS Code's settings. Next to it,
  `gitlens.worktrees.promptForLocation` defaults to `true`
  ([`package.json#L3358-L3371`](https://github.com/gitkraken/vscode-gitlens/blob/8b8814e1/package.json#L3358-L3371)).
  The value can use `~`, `${userHome}`, `${workspaceFolder}` and `${workspaceFolderBasename}`
  ([`src/env/node/git/cliGitProvider.ts#L226-L246`](https://github.com/gitkraken/vscode-gitlens/blob/8b8814e1/src/env/node/git/cliGitProvider.ts#L226-L246)).
- **Default:** with `null`, GitLens starts from the folder above the repository and recommends
  `<that folder>/<repo>.worktrees/`. If the user picks a folder inside the repository, it uses
  `<repo>/../<repo>.worktrees/` instead, so a worktree never ends up inside the main one
  ([`src/commands/git/worktree/create.ts#L509-L552`](https://github.com/gitkraken/vscode-gitlens/blob/8b8814e1/src/commands/git/worktree/create.ts#L509-L552)).
- **Naming:** the branch name, with `/` becoming nested folders.
- **Collisions:** no suffix. "Folder already exists and is not empty" offers *Open Folder*
  ([`create.ts#L423-L439`](https://github.com/gitkraken/vscode-gitlens/blob/8b8814e1/src/commands/git/worktree/create.ts#L423-L439)).
- **GitKraken Desktop:** its
  [worktree docs](https://help.gitkraken.com/gitkraken-desktop/worktrees/) name no default
  location or setting. A user report says the default is `<parent>\<repo>.worktrees\<name>`, in a
  field that can't be edited **(unverified)**.

## lazygit

Source: jesseduffield/lazygit at `a3fae725`.

- **Default:** after the name, a *Worktree location* menu offers the parent folders of the
  existing linked worktrees, then `worktree.defaultPath`, then the repository's parent folder, but
  only if neither of the first two exists. *Other…* takes a typed path
  ([`pkg/gui/controllers/helpers/worktree_helper.go#L189-L219`](https://github.com/jesseduffield/lazygit/blob/a3fae725/pkg/gui/controllers/helpers/worktree_helper.go#L189-L219),
  [`#L383-L426`](https://github.com/jesseduffield/lazygit/blob/a3fae725/pkg/gui/controllers/helpers/worktree_helper.go#L383-L426)).
- **Naming:** the branch name, with spaces turned into `-` and `/` becoming nested folders.
- **Collisions:** git's own error. Branches checked out elsewhere are greyed out.
- **Where the setting lives:** `worktree.defaultPath` in lazygit's `config.yml`. A relative path is
  resolved against the repository root, so `../worktrees` is beside it and `.worktrees` inside it,
  and `~` is expanded
  ([`docs/Config.md#L536-L543`](https://github.com/jesseduffield/lazygit/blob/a3fae725/docs/Config.md#L536-L543)).
  It can be global, per repository in `<repo>/.git/lazygit.yml`, or for a group of repositories
  in a `.lazygit.yml` in a parent folder
  ([`docs/Config.md#L19`](https://github.com/jesseduffield/lazygit/blob/a3fae725/docs/Config.md#L19)).
- **Reasoning:** PR [#5741](https://github.com/jesseduffield/lazygit/pull/5741) (merged
  2026-07-03, fixing #3230, #4708 and #5664). Typed paths were error-prone, and relative paths were
  ambiguous when run from inside another worktree. So the folders already used by worktrees are the
  main cue, and `defaultPath` covers the time before any worktree exists.

## Other tools

Only official docs; not checked in source.

- **Claude Code:** `<repo>/.claude/worktrees/<name>`, on branch `worktree-<name>`. The docs advise
  adding the folder to `.gitignore`. An existing name reopens that worktree. There is no location
  setting, only a `WorktreeCreate` hook that can replace creation
  ([docs](https://code.claude.com/docs/en/worktrees)).
- **Codex app:** `$CODEX_HOME/worktrees`, on a detached HEAD "without polluting your branches".
  *Settings › Worktrees › Worktree root* is global or per project. It keeps the 15 most recent
  ([docs](https://learn.chatgpt.com/docs/environments/git-worktrees)).
- **Conductor:** `~/conductor/workspaces/<repo>/<workspace>`, named after cities, with a `-vN`
  suffix on reuse ([docs](https://conductor.build/docs)).
- **GitKraken Kepler:** *Default Worktrees Folder*, `~/kepler/worktrees`, global with a
  per-repository override ([docs](https://help.gitkraken.com/kepler/settings/)).
- **JetBrains IDEs (2026.2):** a dialog with a *Location* field, which warns against a location
  inside the project folder
  ([docs](https://www.jetbrains.com/help/idea/use-git-worktrees.html)).
- **Cursor:** no location documented. `.cursor/worktrees.json` holds setup commands only.

## Comparison

| Tool | Default location | Folder name | Existing folder | Setting stored in |
|---|---|---|---|---|
| git | none, path required | given | error if not empty | nowhere |
| t3code | `~/.t3/worktrees/<repo>/` | branch, `/`→`-` | git's error | T3 home only (flag, env var) |
| VS Code | `<repo>.worktrees/` beside the repo | branch minus prefix, `/`→`-` | `-1`, `-2` if registered | none (last root remembered) |
| GitLens | `<repo>.worktrees/` beside the repo | branch, `/` nested | error, offers to open | VS Code settings, per folder |
| lazygit | where existing worktrees are, else beside the repo | branch, `/` nested | git's error | lazygit config, global or per repo |
| Codex | `$CODEX_HOME/worktrees` | generated | not documented | app settings, global or per project |
| Claude Code | `<repo>/.claude/worktrees/` | given | reopens it | none |

## What this means for parterre

All **(derived)**.

- **No tool stores the location in git config.** Each uses its own settings. `parterre.worktreeRoot`
  would be new, but git allows any `<section>.<key>`, so it's legal, and `git config` at
  `--global` and repository level gives the two levels #137 wants. lazygit and Codex show that
  "global, with a per-repository override" is a common need.
- **The sibling `<repo>.worktrees/` is the de facto convention** among git clients (VS Code,
  GitLens, reportedly GitKraken), and #6413 cites it as what "other Git clients follow". It keeps
  worktrees next to the repository, so folder-based tools (direnv, mise, nix) still apply,
  outside the main working tree, so `git status` stays clean, and it needs no `.gitignore`.
- **A central app folder** (t3code, Codex, Conductor) suits tools that treat worktrees as
  disposable, managed state. #137 says parterre's worktrees are never hidden or temporary, which
  fits the sibling layout better. t3code's own users ask for the sibling layout for these reasons.
- **Inside the repository** (Claude Code) needs a `.gitignore` entry and shows as untracked
  otherwise. JetBrains and GitLens steer away from it.
- **lazygit's cue is cheap and fits "git does the work":** if a repository already has linked
  worktrees under one parent folder, offer that folder.
- **Naming:** `/`→`-` (t3code, VS Code) gives flat folders but lets `a/b` and `a-b` collide.
  Nested folders (GitLens, lazygit) avoid that but create folders that aren't worktrees. Whichever
  is chosen, git refuses a non-empty folder, so parterre can show git's error or suggest a `-2`
  like VS Code.
- **Prefilled and editable** is what VS Code, GitLens, lazygit and JetBrains all do, matching
  "overridable at creation".
