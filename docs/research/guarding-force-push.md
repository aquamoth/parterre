# Guarding a force push, and fetching

Research for [#140](https://github.com/aquamoth/parterre/issues/140), part of the map
[#137](https://github.com/aquamoth/parterre/issues/137). Hard constraint: git 2.34 (Ubuntu 22.04).

Claims come from git's documentation and source at tag `v2.34.0`, git's release notes, and
TortoiseGit's source. Behaviour was checked with throwaway repos and a bare "remote" on
**git 2.34.1** (Ubuntu 22.04 package `1:2.34.1-1ubuntu1.17`, in a container) and **git 2.53.0.windows.3**.
Both versions gave the same results; only the wording of some `hint:` lines differs.

## Sources

| Short name | What | Link |
|---|---|---|
| PUSH | `git-push.txt` at v2.34.0 | https://github.com/git/git/blob/v2.34.0/Documentation/git-push.txt |
| REMOTE.C | `remote.c` at v2.34.0 (lease and reflog check) | https://github.com/git/git/blob/v2.34.0/remote.c |
| TRANSPORT | `transport.c` at v2.34.0 (rejection texts) | https://github.com/git/git/blob/v2.34.0/transport.c |
| FETCH | `fetch-options.txt`, `git-fetch.txt` at v2.34.0 | https://github.com/git/git/blob/v2.34.0/Documentation/fetch-options.txt |
| FETCH.C | `builtin/fetch.c` at v2.34.0 | https://github.com/git/git/blob/v2.34.0/builtin/fetch.c |
| GREMOTE | `git-remote.txt`, `builtin/remote.c` at v2.34.0 | https://github.com/git/git/blob/v2.34.0/Documentation/git-remote.txt |
| CONFIG | `config/remote.txt`, `config/fetch.txt`, `config/push.txt` at v2.34.0 | https://github.com/git/git/blob/v2.34.0/Documentation/config/remote.txt |
| REL | Release notes 1.8.5, 2.17.0, 2.30.0 | https://github.com/git/git/blob/v2.30.0/Documentation/RelNotes/2.30.0.txt |
| TG | TortoiseGit `master` @ `7338078f` (2026-09-27) | https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/ |
| TGDOC | TortoiseGit manual, Push | https://tortoisegit.org/docs/tortoisegit/tgit-dug-push.html |

## Answer in short

- **Push command after a rebase** (branch `<b>` whose upstream is `<remote>/<b>`, same name):

  ```
  git push --porcelain --force-with-lease=refs/heads/<b> --force-if-includes <remote> refs/heads/<b>:refs/heads/<b>
  ```

  The lease stops the push if the remote moved since the last fetch. `--force-if-includes` stops it
  if the remote-tracking branch has a commit that the local branch never contained, which is how
  git spells "someone added commits after my rebase". It works even when an IDE fetched in the
  background, and it works for rebases done outside parterre, because it reads git's own reflog.
  Parterre pins nothing and stores nothing. Needs git 2.30.
- **Don't pin a commit** (`--force-with-lease=<ref>:<sha>`) in the normal case: git turns
  `--force-if-includes` off when the lease is pinned, and neither choice of pin implements the rule
  (details below). A pin is only the fallback when the local and remote branch names differ.
- **TortoiseGit** has two checkboxes: *Force with lease* (plain `--force-with-lease`) and *Force*
  (`--force`). It never pins and never uses `--force-if-includes`.
- **Show the planned force push** with
  `git log --cherry-mark --right-only <b>...<remote>/<b>`: `=` lines were rewritten by the rebase,
  `>` lines will be gone from the branch.
- **Fetch** with `git fetch --all --prune`. `git remote update --prune` runs the same fetches, but it
  is a thin wrapper that only adds `remotes.default` and rejects most fetch options. Never pass
  `--prune-tags`: with several remotes it deletes tags that exist only locally or on other remotes.

## 1. Leases: plain versus pinned

`--force-with-lease` compares the remote ref, at push time, with an expected value, and rejects the
push if they differ ([PUSH L234-L281](https://github.com/git/git/blob/v2.34.0/Documentation/git-push.txt#L234-L281)).
The forms differ only in where the expected value comes from:

| Form | Expected value | Protects |
|---|---|---|
| `--force-with-lease` | our remote-tracking branch, read at push time | every ref being pushed |
| `--force-with-lease=<ref>` | our remote-tracking branch, read at push time | `<ref>` only |
| `--force-with-lease=<ref>:<expect>` | the given commit | `<ref>` only; `<expect>` empty means "must not exist" |

Available since git 1.8.5 ([REL 1.8.5 L183-L184](https://github.com/git/git/blob/v1.8.5/Documentation/RelNotes/1.8.5.txt#L183-L184)).
The docs still call every form except `<ref>:<expect>` "experimental"
([PUSH L275-L278](https://github.com/git/git/blob/v2.34.0/Documentation/git-push.txt#L275-L278)),
but they have behaved the same since 2013.

**How background fetches defeat the plain form.** The plain form trusts `refs/remotes/<remote>/<b>`.
Anything that fetches (an IDE, a cron job, `git fetch --all` in another terminal) moves that ref to
the colleague's new commit, and the lease then matches and the push overwrites it. git's docs say so
directly: the plain form "interacts very badly with anything that implicitly runs `git fetch`"
([PUSH L283-L322](https://github.com/git/git/blob/v2.34.0/Documentation/git-push.txt#L283-L322)).
Verified (scenario S2 below): a colleague pushes `F2` after my rebase, a background fetch runs, and
`git push --force-with-lease origin feature` succeeds and deletes `F2`.

**A pinned lease is immune to background fetches**, because the expected value is fixed on the
command line. Its weakness is the other way round: it only proves that the remote still is what
*I* expected, not that I ever integrated it.

## 2. `--force-if-includes`

**What it checks.** For each ref whose lease uses the remote-tracking branch, git looks for the
remote-tracking tip in the reflog of the *local branch with the same name*. The push is allowed if the
tip equals a reflog entry, or is an ancestor of one. The walk goes newest first and stops at the
first entry older than the newest entry in the remote-tracking branch's own reflog
([REMOTE.C `is_reachable_in_reflog` L2474-L2520](https://github.com/git/git/blob/v2.34.0/remote.c#L2474-L2520),
[PUSH L352-L366](https://github.com/git/git/blob/v2.34.0/Documentation/git-push.txt#L352-L366)).
In words: "since my remote-tracking branch last changed, has my branch ever contained its tip?"
A rebase leaves the pre-rebase tip in the branch's reflog, so an honest rebase passes.

**Version.** Added in git **2.30.0**, together with `push.useForceIfIncludes`
([REL 2.30.0 L24-L28](https://github.com/git/git/blob/v2.30.0/Documentation/RelNotes/2.30.0.txt#L24-L28),
[config/push.txt at v2.30.0](https://github.com/git/git/blob/v2.30.0/Documentation/config/push.txt)).
Available in 2.34. No behaviour changes are listed in the release notes from 2.31 to 2.50.

**How it combines with a lease** ([REMOTE.C `apply_cas` L2535-L2570](https://github.com/git/git/blob/v2.34.0/remote.c#L2535-L2570),
[PUSH L324-L331, L360-L363](https://github.com/git/git/blob/v2.34.0/Documentation/git-push.txt#L324-L331)):

- with `--force-with-lease` or `--force-with-lease=<ref>`: the lease is checked first, then the reflog;
- with `--force-with-lease=<ref>:<expect>`: silently a no-op;
- without any lease: a no-op, so the push is rejected as an ordinary non-fast-forward (verified);
- `push.useForceIfIncludes=true` makes a plain `--force-with-lease` behave the same (verified, S6).

**Failure messages** ([TRANSPORT L673-L682](https://github.com/git/git/blob/v2.34.0/transport.c#L673-L682)).
Verified output on 2.34.1:

```
 ! [rejected]        feature -> feature (stale info)
```
The lease failed: the remote moved since our last fetch. No hint follows.

```
 ! [rejected]        feature -> feature (remote ref updated since checkout)
hint: Updates were rejected because the tip of the remote-tracking
hint: branch has been updated since the last checkout. You may want
hint: to integrate those changes locally (e.g., 'git pull ...')
hint: before forcing an update.
```
The reflog check failed. (The hint is reworded in newer git, the status text is not.)

With `--porcelain` the status line is tab-separated and stable, so parterre should parse that instead:

```
!	refs/heads/feature:refs/heads/feature	[rejected] (stale info)
!	refs/heads/feature:refs/heads/feature	[rejected] (remote ref updated since checkout)
+	refs/heads/feature:refs/heads/feature	e18c56c...392500f (forced update)
```

**Known limits** (all verified):

- **Different local and remote names:** git reads the reflog of the local branch with the *remote*
  branch's name ([REMOTE.C `get_local_ref` L1904-L1918](https://github.com/git/git/blob/v2.34.0/remote.c#L1904-L1918),
  [`check_if_includes_upstream`](https://github.com/git/git/blob/v2.34.0/remote.c#L2525-L2533)).
  Pushing `mine:feature` checks the reflog of `feature`, which doesn't exist, so the push is
  **always rejected**, even when nothing is wrong (S5b).
- **No reflog** (reflog deleted or `core.logAllRefUpdates=false`): always rejected (S10). Fails safe.
- **Merely visiting the remote tip counts:** `reset --hard origin/feature` followed by
  `reset --hard HEAD@{1}` puts the tip in the reflog, and the push then succeeds and drops `F2` (S9).
  Only deliberate reflog manipulation does this; parterre never does.

## 3. Which commit to pin after a rebase

The human's rule: *allow the force push unless someone added commits after the rebase happened.*

| Scenario (verified on 2.34.1 and 2.53) | plain lease | pin: tip when rebase started | pin: tip last shown | lease + `--force-if-includes` |
|---|---|---|---|---|
| S1 Nobody else pushed | pushes | pushes | pushes | pushes |
| S2 Colleague pushed `F2` after my rebase, then a background fetch | **pushes, `F2` lost** | stale info | **pushes, `F2` lost** (but the dialog listed it) | remote ref updated since checkout |
| S3 Colleague pushed `F2` after my rebase, no fetch | stale info | stale info | stale info | stale info |
| S4 Colleague pushed `F2` *before* my rebase, I fetched it but rebased without it | **pushes, `F2` lost** | **pushes, `F2` lost** | **pushes, `F2` lost** (listed) | remote ref updated since checkout |

- **Pin to the tip the rebase started from.** Implements the rule as worded (S2 rejected, S4 allowed).
  But git doesn't record that commit in a usable form: parterre would have to store it at rebase time,
  and a rebase run on the command line or in another tool leaves nothing to pin. In S4 it quietly drops
  a commit the user had fetched and seen but never had on their branch.
- **Pin to the tip last shown in the dialog.** Only guarantees that exactly the listed commits are
  replaced. It does not implement the rule: after a fetch, a colleague's post-rebase commit is simply
  one more line in the list (S2).
- **Lease plus `--force-if-includes`.** Implements the rule, slightly stricter: it rejects anything on
  the remote that the local branch never contained, whether it arrived before or after the rebase
  (S4). This is what git designed the option for ("ensure that what is being force-pushed was created
  after examining the commit at the tip of the remote ref",
  [REL 2.30.0](https://github.com/git/git/blob/v2.30.0/Documentation/RelNotes/2.30.0.txt#L24-L28)).
  It needs no state in parterre and covers rebases done elsewhere.

**Recommendation.** For a branch `<b>` whose upstream is `<remote>/<b>`:

```
git push --porcelain --force-with-lease=refs/heads/<b> --force-if-includes <remote> refs/heads/<b>:refs/heads/<b>
```

- Name the lease and the refspec with full refs, so a tag called `<b>` can't be matched and only this
  ref is forced.
- Right before running it, re-read `<remote>/<b>`. If it moved since the dialog was drawn, redraw the
  dialog instead of pushing, so what is replaced is always what was listed.
- On `stale info`: the remote moved and parterre hasn't fetched it. Offer Fetch; the push dialog will
  then show the new commits.
- On `remote ref updated since checkout`: the remote has commits the branch never had. Say so, list
  them (`git log <b>..<remote>/<b>`), and suggest integrating them first (for example rebase again onto
  `<remote>/<b>`). Don't offer a way round it: parterre has no plain `--force`.
- **If the local branch name differs from its upstream's**, `--force-if-includes` always refuses
  (section 2). Fall back to a pin on the tip shown in the dialog:
  `git push --porcelain --force-with-lease=refs/heads/<rb>:<shown-sha> <remote> refs/heads/<b>:refs/heads/<rb>`.
  This only guarantees "replaces exactly what was listed". Whether that is acceptable, or whether
  parterre should refuse a force push to a differently named upstream, is a decision for the human.

**What TortoiseGit does.** The push dialog has two mutually exclusive checkboxes, *Force with lease*
and *Force* ([TG PushDlg.cpp L607-L620](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/PushDlg.cpp#L607-L620)),
which add a bare `--force-with-lease` or `--force`
([TG AppUtils.cpp L2769-L2774](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L2769-L2774)).
The tooltip says the lease checks "the same commit as the remote tracking branch"
([TG TortoiseProcENG.rc L5076-L5077](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Resources/TortoiseProcENG.rc#L5076-L5077), [TGDOC](https://tortoisegit.org/docs/tortoisegit/tgit-dug-push.html)).
There is no pin and no `--force-if-includes`. After a rebase, the *Push* button opens the ordinary
push dialog with nothing pre-ticked ([TG AppUtils.cpp L2472-L2489](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L2472-L2489)).
A rejected push offers Pull, Fetch and Push again
([TG AppUtils.cpp L2884-L2891](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L2884-L2891)).
So TortoiseGit is exposed to the background-fetch problem in S2.

## 4. Showing a planned force push

The commits the push takes off the remote branch are the ones reachable from the remote tip but not
from the local one:

```
git log --oneline <b>..<remote>/<b>
```

After a rebase this also lists the user's own pre-rebase commits, which aren't lost, only rewritten.
`--cherry-mark` separates the two by patch id:

```
git log --cherry-mark --right-only --format='%m %h %s' <b>...<remote>/<b>
```

Verified output for S2 after the fetch:

```
> F2      <- only on the remote: will be gone from the branch
= F1      <- the same change exists on the rebased branch
```

The `--left-only` side of the same range lists what the push adds (`=` for rewritten commits, `<` for
new ones, including commits from the new base). A commit whose change was altered while resolving a
conflict has a different patch id and shows as `>`, so the list errs towards warning. `--cherry-mark`
has existed since git 1.7.x, well before 2.34.

The dialog should use the same `<remote>/<b>` value that the lease will check, which is why parterre
re-reads it before pushing (section 3).

## 5. Fetching

**`git remote update` is a wrapper around `git fetch`.** In 2.34 it runs
`git fetch [--prune|--no-prune] [-v] --multiple <groups or remotes>`; with no argument it uses the
group `default`, and if `remotes.default` isn't set it runs `git fetch --all` instead
([builtin/remote.c `update` L1454-L1492](https://github.com/git/git/blob/v2.34.0/builtin/remote.c#L1454-L1492),
[GREMOTE L187-L196](https://github.com/git/git/blob/v2.34.0/Documentation/git-remote.txt#L187-L196)).
It only accepts `-p/--prune` (and the global `-v`); `git remote update --prune-tags` fails with
`error: unknown option 'prune-tags'` (verified).

**Differences between the two** (verified):

| | `git fetch --all --prune` | `git remote update --prune` |
|---|---|---|
| Remotes fetched | every remote without `skipFetchAll` | `remotes.default` if set, otherwise the same as `fetch --all` |
| `remote.<name>.skipFetchAll` / `skipDefaultUpdate` | honoured | honoured |
| `remotes.default` | ignored | honoured |
| Other fetch options (`--tags`, `--no-tags`, `--prune-tags`, `-q`, `--dry-run` …) | accepted | rejected |

**`skipDefaultUpdate` versus `skipFetchAll`.** In 2.34 they are the same setting: both keys set the
same field ([REMOTE.C L362-L365](https://github.com/git/git/blob/v2.34.0/remote.c#L362-L365)), and that
field is what both `fetch --all` and `remote update` check
([FETCH.C L1676-L1682](https://github.com/git/git/blob/v2.34.0/builtin/fetch.c#L1676-L1682)). Verified: either
key skips the remote in either command. The 2.34 docs describe them with identical text
([CONFIG remote.txt L35-L44](https://github.com/git/git/blob/v2.34.0/Documentation/config/remote.txt#L35-L44));
current git documents `skipDefaultUpdate` as "a deprecated synonym to `remote.<name>.skipFetchAll`"
([master config/remote.adoc](https://github.com/git/git/blob/master/Documentation/config/remote.adoc)).

**Pruning.** `--prune` deletes remote-tracking branches whose branch is gone from the remote. It never
touches local branches or tags fetched by auto-following
([FETCH L127-L140](https://github.com/git/git/blob/v2.34.0/Documentation/fetch-options.txt#L127-L140),
[git-fetch PRUNING](https://github.com/git/git/blob/v2.34.0/Documentation/git-fetch.txt#L106)).
Precedence: command line, then `remote.<name>.prune`, then `fetch.prune`, else off
([FETCH.C L1914-L1932](https://github.com/git/git/blob/v2.34.0/builtin/fetch.c#L1914-L1932)).
Verified: `fetch.prune=true` makes a plain `git fetch --all` prune; `--prune` on the command line
overrides `remote.origin.prune=false`; `--no-prune` overrides `fetch.prune=true`.
`fetch --all` passes `--prune`/`--no-prune` on to each remote's fetch
([FETCH.C L1724-L1731](https://github.com/git/git/blob/v2.34.0/builtin/fetch.c#L1724-L1731)).

**Pruning tags** (`--prune-tags`, `fetch.pruneTags`, since git 2.17,
[REL 2.17.0](https://github.com/git/git/blob/v2.17.0/Documentation/RelNotes/2.17.0.txt#L32-L33))
makes each remote's fetch mirror its tags into `refs/tags/`, deleting any local tag that remote
doesn't have ([FETCH L142-L150](https://github.com/git/git/blob/v2.34.0/Documentation/fetch-options.txt#L142-L150)).
Tags have one namespace for all remotes, so with several remotes this is destructive. Verified with
three remotes each owning one tag, plus a local-only tag: `git fetch --all --prune --prune-tags` deleted
the local-only tag, then each remote deleted the other remotes' tags in turn, and only the last
remote's tag survived. Parterre must never pass it (tags are out of scope anyway).

**Which one a user would expect.** Recommend **`git fetch --all --prune`** for the Fetch button:

- It is the command people know and type; TortoiseGit's fetch dialog uses `git fetch --all` for
  "All remotes" ([TG AppUtils.cpp L2579-L2590](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L2579-L2590))
  and keeps `git remote update` only in its Sync dialog
  ([TG SyncDlg.cpp L487](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/SyncDlg.cpp#L487)).
- The way users exclude a remote, `skipFetchAll`, works with both commands. Only the rare
  `remotes.default` is lost.
- It accepts the full set of fetch options if parterre ever needs one.
- `--prune` keeps the graph honest: without it, branches deleted on the remote (typically after a pull
  request is merged) stay in the graph as labels. It deletes only remote-tracking refs, which the next
  fetch recreates if the branch comes back.

Trade-off for the human: `--prune` overrides a user's explicit `fetch.prune=false`. TortoiseGit instead
defaults its Prune checkbox to "follow config" and passes nothing
([TG PullFetchDlg.cpp L49, L360-L363](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/PullFetchDlg.cpp#L360-L363)),
which is consistent with the map's "plain `git pull`, let the user's config decide" rule for pull, but
since git's default is no pruning, most users would then see deleted branches linger.

## Method

Each scenario starts fresh: a bare `remote.git` with `main` (M1) and `feature` (F1, pushed by me and
tracking `origin/feature`), a colleague's clone, and a new commit M2 on `main`. "My rebase" is
`git fetch origin main && git rebase origin/main` on `feature`. "Colleague pushes F2" commits on top of
`origin/feature` and pushes. "Background fetch" is `git fetch origin` in my clone. Committer dates were
advanced one minute per step (`GIT_COMMITTER_DATE`), because the reflog check compares timestamps.
Fetch scenarios used three bare remotes (`origin`, `upstream`, `fork`), each with a branch `gone`
deleted after cloning and one tag, plus a local-only tag. Run on git 2.34.1 (Ubuntu 22.04 container)
and git 2.53.0.windows.3 with identical outcomes.
