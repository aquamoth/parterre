# GitHub forks and pull requests in the graph

Research note for the question: *can parterre draw open PRs and fork branches of a GitHub repository,
in their own colour, when the local clone has no remotes for them? It has to be an optional
extension that never needs `gh` to start.*

Sources are official docs, source code at pinned commits, and experiments I ran. A statement that
is my own conclusion is marked **(derived)**. A statement I could not check is marked
**(unverified)**. A statement I checked by running a command is marked
**(tested 2026-09-26: `command`)**. The experiments ran on the local git 2.43.0, with gh 2.101.0
installed and logged in. "unauth" means no token was sent.

## TL;DR

**Feasible:** yes, and without `gh`, a new remote, or any write to the user's repository.

**Decided scope (2026-09-26):** open PRs only, shown as PR-icon tags on nodes. See §12 and §14.
**Built (slice 1):** differently from §3, §6 and §12.2, after looking at how t3code does it:
signed in only, per fetched branch over GraphQL, cached, within a budget. See `TODO.md`,
question 25.
The points below cover everything that was investigated.

- **PR heads come through git alone.** GitHub publishes `refs/pull/<N>/head` for every PR, open
  or closed. Plain `git fetch <url> <refspec>` fetches them without adding a remote. These refs
  can't tell open PRs from closed ones, and neither can `…/merge`. So titles, authors and the
  open/closed state need one REST call per 100 PRs (`GET /repos/{o}/{r}/pulls?state=open`). That
  call works unauthenticated for public repositories, within 60 requests per hour.
- **Fork branches that are not PRs** need an API to find them (REST `forks` plus `branches`, or a
  single GraphQL query). GraphQL needs a token. There are thousands of stale forks, so the list
  needs filters: recently pushed, ahead of base, and a cap on the count.
- **Fork commits can be fetched through the parent repository's URL by SHA.** Fork networks
  share storage (tested), so one `git fetch` against `origin`'s URL covers PRs and forks alike.
- **Fetched objects go in a separate cache repository**, e.g.
  `$XDG_CACHE_HOME/parterre/<hash>.git`. Its `objects/info/alternates` points at the user's
  object directory, so a fetch downloads only the missing commits. `git log` in the user's repo
  then sees the cache through `GIT_ALTERNATE_OBJECT_DIRECTORIES`. Both steps were tested. Use
  `--filter=tree:0`: the graph reads only commit headers, and `%T` needs no tree object (tested).
  Cost: rust-lang/rust with 1,385 open PRs → 3,862 commits, 1.4 MiB, 4 s (tested).
- **Auth, in order of preference:** `GH_TOKEN`/`GITHUB_TOKEN`, then `gh auth token` if `gh` is
  on PATH, then non-interactive `git credential fill`, then unauthenticated. Tokens are never
  logged.
- **Architecture:** parterre-core gets a GUI-free `forge` module that finds the base repository
  and fetches into the cache. The HTTP client sits behind a cargo feature: `ureq`, with about 34
  extra crates. Alternatively, shell out to `curl`/`gh api` and add no crates. The app loads on
  a background thread, and the graph shows the local repository at once. Remote-only refs arrive
  later with their own `RefKind`s and colours. The whole thing is off by default.
- **Deviation from TortoiseGit:** TortoiseGit has nothing like this (§9). Record that in
  `TODO.md` when it is built.

## Sources (pinned)

| Short name | What | Permalink base |
|---|---|---|
| GIT | git `v2.55.0` @ `e9019fca` (2026-06-29) | https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/ |
| GH | GitHub CLI `v2.101.0` @ `0cf10924` | https://github.com/cli/cli/blob/0cf1092493af067646fc5f3db9421c6a6ec9c938/ |
| GHDOCS | GitHub Docs (unversioned, read 2026-09-26) | https://docs.github.com/en/ |
| GHCLI | gh manual (read 2026-09-26) | https://cli.github.com/manual/ |
| GCM | Git Credential Manager docs, `main` (read 2026-09-26) | https://github.com/git-ecosystem/git-credential-manager/blob/main/docs/ |
| GLDOCS | GitLab docs (read 2026-09-26) | https://docs.gitlab.com/ |
| ADO | Azure DevOps REST 7.1 / Pipelines docs (read 2026-09-26) | https://learn.microsoft.com/en-us/ |
| TRUFFLE | Truffle Security, "Anyone can Access Deleted and Private Repository Data on GitHub", 2024-07-24 | https://trufflesecurity.com/blog/anyone-can-access-deleted-and-private-repo-data-github |
| TGNOTE | This repo's TortoiseGit research note | [tortoisegit-revision-graph.md](tortoisegit-revision-graph.md) |

The GIT docs are for 2.55. The local git is 2.43, and every git behaviour below was tested on it
unless marked otherwise.

---

## 1. How parterre reads a repository today (context)

- Core shells out to `git`. Every call goes through `Git::command()`, which sets
  `GIT_OPTIONAL_LOCKS=0`, `LC_ALL=C` and a null stdin
  ([crates/parterre-core/src/git.rs](../../crates/parterre-core/src/git.rs)).
- `load()` runs `for-each-ref` and then pipes every ref tip and HEAD into
  `git log --stdin -z --format=%H%x00%P%x00%T…`. **(derived)** So extra tips can be appended to
  that stdin list and nothing else in the walk changes. Those commits only have to be readable
  through the object store (§4).
- `%T` is used only to spot empty-tree roots (`EMPTY_TREE_SHA1/256`,
  [revgraph.rs](../../crates/parterre-core/src/revgraph.rs) `treesame_root`). `%T` prints the
  tree id stored in the commit header, so no tree objects are needed
  (**tested 2026-09-26**: `log --format=%T` on a commit fetched with `--filter=tree:0` printed
  its tree id).
- `RefKind` is `LocalBranch | RemoteBranch | Tag | Stash | DetachedHead | Other`
  ([repo.rs](../../crates/parterre-core/src/repo.rs)). `refs/pull/*` already maps to `Other`
  (unit test `classifies_refs`). The app picks colours per kind in `Palette::ref_fill`
  ([theme.rs](../../crates/parterre/src/theme.rs)), and its legend already has a
  `pull/12/head` "Other ref" swatch. `GraphOptions::show_other_refs` hides such refs.
- Startup loads the repository synchronously in `main()` (`load_repo`), and `App::reload`
  does too ([main.rs](../../crates/parterre/src/main.rs),
  [app.rs](../../crates/parterre/src/app.rs)). Layout and commit-message loading already run on
  background threads with `mpsc` channels in `app.rs`.
- Core depends only on `serde` and `thiserror`. The app pulls in 192 crates (`cargo tree -p
  parterre -e normal`), none of them an HTTP or TLS stack.

## 2. PR heads without a remote

**The refs.** Every PR on the base repository has `refs/pull/<N>/head`. Open PRs that merge
cleanly also have `refs/pull/<N>/merge`, GitHub's test merge. GitHub's docs describe fetching
"the reference to the pull request based on its ID number" with `git fetch origin
pull/ID/head:BRANCH_NAME`, and say that "the remote `refs/pull/` namespace is *read-only*"
([GHDOCS checking out PRs locally](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/reviewing-changes-in-pull-requests/checking-out-pull-requests-locally)).
The REST docs speak of a "test merge commit" in `merge_commit_sha`, but no GitHub doc I found
says when `…/merge` is created or deleted
([GHDOCS Get a pull request](https://docs.github.com/en/rest/pulls/pulls#get-a-pull-request)).
**(unverified)** as a documented rule.

**What the refs show in practice** (**tested 2026-09-26**):

- `git ls-remote https://github.com/aquamoth/parterre 'refs/pull/*'` lists `head` for PRs 1–10,
  22–24 and 30, but `merge` only for #30. REST (`?state=all`) says #30 is the one open PR, and
  all the others are closed and merged. So `head` is kept after close.
- On rust-lang/rust, `ls-remote` returned 98,795 `head` and 12,697 `merge` refs (6.9 MB of
  output, 0.76 s). REST listed 1,385 open PRs. Of those, 1,374 have a merge ref and 11 don't: I
  sampled 3 of the 11 and all had `mergeable_state: dirty`, i.e. conflicts. On the other side,
  11,323 merge refs belong to PRs that are *not* open. Most are old, but 66 are above #100000.
  One example is #162036, closed unmerged on 2026-09-06, whose merge ref still exists.
- **(derived)** Neither ref tells open from closed. "Has `merge`" misses conflicted open PRs
  and includes closed ones, so open/closed has to come from the API.

**Fetching without a remote.** git's docs show fetching from a URL without configuring a remote
([GIT git-fetch.adoc#L280-L293](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/Documentation/git-fetch.adoc#L280-L293)).
Tested 2026-09-26:

- `git fetch https://github.com/aquamoth/parterre refs/pull/30/head` fetched the commit and
  wrote `FETCH_HEAD`. `git remote -v` was unchanged.
- The same fetch with refspec `+refs/pull/*/head:refs/parterre/github/pull/*` also **created
  `refs/tags/v0.2.0` and `v0.3.0`** in the clone. Tag auto-following applies to URL fetches,
  and `remote.origin.tagopt=--no-tags` did not stop it, because the URL is not the remote.
  git documents that "any tag that points into the histories being fetched is also fetched"
  unless `--no-tags` is given
  ([GIT git-fetch.adoc#L25](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/Documentation/git-fetch.adoc#L25)).
  **Always pass `--no-tags`.**

**Listing cost.** `ls-remote <url> <pattern>` filters on the client: patterns become `*/<pat>`
tail matches, and only `--tags`/`--branches` send a `ref-prefix` to the server
([GIT builtin/ls-remote.c#L119-L125](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/builtin/ls-remote.c#L119-L125),
[#L157](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/builtin/ls-remote.c#L157)).
**(tested 2026-09-26:** `GIT_TRACE_PACKET=1 git ls-remote … 'refs/pull/*'` sent no
`ref-prefix`**)**. `git fetch` does derive `ref-prefix` from its refspecs
([GIT refspec.c#L257](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/refspec.c#L257),
[builtin/fetch.c#L1920](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/builtin/fetch.c#L1920)).
**(tested 2026-09-26:** a fetch with `+refs/pull/*/head:…` sent `ref-prefix refs/pull/`**)**.
Even so, `refs/pull/*/head` matches all ~99k PRs on rust-lang/rust. **(derived)** Fetch only
the open PRs, by explicit refspec `refs/pull/N/head` or by head SHA, using the numbers from the
API.

## 3. PR metadata without gh

**Endpoint.** `GET /repos/{owner}/{repo}/pulls` takes `state` (`open` by default, or
`closed`/`all`), `head`, `base`, `sort` (`created`/`updated`/`popularity`/`long-running`),
`direction` and `per_page` (at most 100)
([GHDOCS List pull requests](https://docs.github.com/en/rest/pulls/pulls#list-pull-requests)).
Pagination uses the `link` header with `rel="next"`/`rel="last"`
([GHDOCS pagination](https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api)).

**Fields.** The unauth list response carries `number`, `title`, `draft`, `user.login`,
`head.sha`, `head.ref`, `head.repo.full_name` (`null` when the fork was deleted), `base.ref`,
`merged_at` and `state` (tested 2026-09-26: `curl` unauth, `aquamoth/parterre/pulls?state=all`).
GraphQL `pullRequests(states:OPEN, first:100){number title isDraft headRefOid headRefName
headRepositoryOwner{login} baseRefName author{login}}` returns the same data and cost 1 point
per page (tested 2026-09-26 on rust-lang/rust with `gh api graphql`).

**Rate limits.** Unauthenticated: "60 requests per hour". Authenticated: "5,000 requests per
hour" ([GHDOCS REST rate limits](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api)).
The unauthenticated limit is per IP address (tested 2026-09-26: the 403 message names the
caller's IP). **(derived)** rust-lang/rust needs 14 pages per refresh: 1,385 open PRs at `per_page=100`
(tested 2026-09-26: `rel="last"` was page 1385 at `per_page=1`). So unauthenticated use covers
about 4 refreshes per hour.

**Conditional requests.** "Making a conditional request does not count against your primary
rate limit if a `304` response is returned and the request was made while correctly authorized
with an `Authorization` header"
([GHDOCS best practices §use-conditional-requests](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api#use-conditional-requests)).
Both halves confirmed (**tested 2026-09-26**, `curl -H "If-None-Match: <etag>"`):

- Unauthenticated, each 304 still used up one request (`x-ratelimit-remaining` 58 → 57).
- With a token, repeated 304s left the remaining count unchanged (4960 → 4960 → 4960).

**(derived)** Store the ETag of each page, and poll only with a token.

## 4. Fork branches that are not PRs

**Finding forks.**

- REST: `GET /repos/{o}/{r}/forks`, `sort` = `newest` (default), `oldest`, `stargazers` or
  `watchers`; `per_page` ≤ 100 ([GHDOCS forks](https://docs.github.com/en/rest/repos/forks)).
  There is no `pushed` sort, and branches need one extra `GET /repos/{fork}/branches` call per
  fork ([GHDOCS branches](https://docs.github.com/en/rest/branches/branches#list-branches)).
- GraphQL: one query does both. `repository.forks(first:100, orderBy:{field:PUSHED_AT,
  direction:DESC}){nodes{nameWithOwner pushedAt refs(refPrefix:"refs/heads/", first:20){nodes{name
  target{oid}}}}}` cost **1 point** (tested 2026-09-26 on cli/cli). GraphQL cost is computed
  by adding "the number of requests needed to fulfill each unique connection" and dividing by
  100, and `first` must be 1–100
  ([GHDOCS GraphQL limits](https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api)).
- GraphQL needs a token. The docs say "You can authenticate to the GraphQL API using a personal
  access token, GitHub App, or OAuth app"
  ([GHDOCS forming calls](https://docs.github.com/en/graphql/guides/forming-calls-with-graphql#authenticating-with-graphql)).
  Unauthenticated `POST /graphql` returned **403**, and `/rate_limit` reported
  `graphql.limit = 0` (tested 2026-09-26: `curl`).

**Noise** (tested 2026-09-26 on cli/cli):

- The repository has 9,084 forks, which is 102 REST pages.
- Its newest forks all report the upstream's `pushed_at` (`2026-09-25T07:51:36Z`) at creation.
  So `pushed_at` does not mean the fork has its own pushes.
- The 100 most recently pushed forks listed 1,239 branch tips, many of them copies of upstream
  branches.
- **(derived)** Useful filters:
  1. only forks pushed after they were created;
  2. only tips not already in the local graph;
  3. only branches ahead of the base (see the next bullet);
  4. a cap, e.g. 20 forks × 10 branches.
- "Ahead of base" costs one call per branch. REST `GET /repos/{o}/{r}/compare/{base}...{user}:{repo}:{branch}`
  returns `status`, `ahead_by` and `behind_by`
  ([GHDOCS compare](https://docs.github.com/en/rest/commits/commits#compare-two-commits)).
  Tested 2026-09-26: `trunk...DynamoIVII:cli:patch-1` gave `diverged 1 4`. GraphQL
  `ref(qualifiedName:"refs/heads/trunk"){compare(headRef:"DynamoIVII:patch-1"){aheadBy behindBy status}}`
  gave the same answer and cost 1. **(derived)** Alternatively, fetch the tips and let the
  local graph decide which are ahead, at no API cost.

**Fetching fork commits through the parent URL.** GitHub documents that "Commits pushed to any
repository in a network can be accessible from other repositories in that network, including
the upstream repository", even after the fork is deleted
([GHDOCS about forks](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/working-with-forks/about-forks)).
TRUFFLE (2024-07-24) calls this "Cross Fork Object Reference" and shows it through the web
UI, where GitHub answered that it is intended.

In plain git, a server accepts a `want` for an unadvertised object only with
`uploadpack.allowReachableSHA1InWant` or `allowAnySHA1InWant`, and both default to off
([GIT config/uploadpack.adoc#L16-L32](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/Documentation/config/uploadpack.adoc#L16-L32)).
GitHub's server clearly accepts such wants for objects in the fork network. It rejects
unknown SHAs with `not our ref`. **Tested 2026-09-26:**

- Commit `020e1317` is the tip of `info-zezotechnology/cli-1:bump-go-1.27.1`. It is not
  advertised by cli/cli, and it is not reachable from any cli/cli ref: it was missing from a
  `tree:0` mirror of `+refs/*:refs/*`.
- `git fetch --no-tags --filter=tree:0 https://github.com/cli/cli 020e1317…:refs/probe/x`
  **succeeded**.
- `git fetch https://github.com/cli/cli 0000…0001:…` failed with `upload-pack: not our ref`.
- A lazy fetch in a `blob:none` partial clone of cli/cli also filled in fork objects on demand.
  My `cat-file -e` probe loop created 156 packs.
- Fetching from the fork's own URL also works:
  `git fetch https://github.com/DynamoIVII/cli refs/heads/patch-1:refs/fork/viafork`.

**(derived)** One connection to the base URL can fetch PR heads *and* fork tips by SHA, and the
fork's own URL is the fallback. That GitHub keeps allowing this is **(unverified)** as a promise.
Only the forks page states it.

## 5. Where fetched objects go without touching the user's repo

| Option | Effect on the user's repo | Verdict |
|---|---|---|
| (a) Fetch into `refs/parterre/*` in the user's repo | Adds refs, objects and `FETCH_HEAD`. Tags are auto-followed unless `--no-tags`. `--filter` writes `remote.<url>.promisor`/`partialclonefilter` and bumps `repositoryformatversion` (§5.2). Runs `git maintenance run --auto` ([GIT fetch-options.adoc#L170-L175](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/Documentation/fetch-options.adoc#L170-L175)). The refs show up in `git log --all`, gitk and TortoiseGit. | Rejected: parterre is read-only. |
| (b) Fetch objects only (`FETCH_HEAD` / `--no-write-fetch-head`) | Still writes objects. They are unreachable, and "the fetched objects will eventually be removed by git's built-in housekeeping" ([GIT git-fetch.adoc#L291-L293](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/Documentation/git-fetch.adoc#L291-L293)). They are refetched after every gc. | Rejected. |
| (c) Separate cache repo; alternates → user objects; `GIT_ALTERNATE_OBJECT_DIRECTORIES` → cache objects | None. | **Recommended.** |

### 5.1 How (c) works

- `objects/info/alternates` "records paths to alternate object stores that this object store
  borrows objects from, one pathname per line"
  ([GIT gitrepository-layout](https://git-scm.com/docs/gitrepository-layout)).
- `GIT_ALTERNATE_OBJECT_DIRECTORIES` is a `:`-separated list (`;` on Windows) of object
  directories to search, and "New objects will not be written to these directories"
  ([GIT git.adoc#L524-L535](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/Documentation/git.adoc#L524-L535)).
- Alternates nest up to 5 levels deep
  ([GIT odb.c#L192-L199](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/odb.c#L192-L199)),
  so a user repo that itself uses alternates still works.
- fetch-pack sends the alternates' ref tips as "haves" during negotiation
  ([GIT fetch-pack.c#L111-L121, L375, L1794](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/fetch-pack.c#L1794)).
  The cache therefore downloads only what the user's repo lacks.

**Tested 2026-09-26** on a throwaway clone of aquamoth/parterre plus `/tmp/pt/cache.git`, a bare
repo whose alternates point at the clone's `.git/objects`:

1. `git -C cache.git fetch --no-tags --filter=tree:0 <url> '+refs/pull/*/head:refs/pull/*/head'`
   brought in 14 refs and **1 object**, because the other PR heads were already in the clone.
2. In the clone, a plain `git log` of PR #30's commit failed with `bad object`. With
   `GIT_ALTERNATE_OBJECT_DIRECTORIES=/tmp/pt/cache.git/objects`,
   `git log --stdin -z --format=%H%x00%P%x00%T%x00%s` walked it into local history, and `%T`
   printed the tree id even though no tree object exists.
3. The loop cache → user → cache (alternates plus the env var) caused no error.

**Size and speed** (tested 2026-09-26, same setup):

| Repository | Open PRs | Commits fetched | Pack size | Time |
|---|---|---|---|---|
| cli/cli, blob:none clone | 63 | 105 | 51 KiB | 1.4 s |
| rust-lang/rust, tree:0 clone of `main` (341,536 commits) | 1,385 (`fetch --stdin`) | 3,862 | 1.39 MiB | 4.1 s |

- Without alternates, the cli/cli fetch pulled 12,378 commits (5.7 MiB) instead of 105.
  `--shallow-exclude=trunk` cut that to 136, but it makes a shallow repo.
- On rust-lang/rust, `git log` over `main` plus the 1,385 extra tips took about the same time as
  `main` alone (≈1.7–3.0 s, 3,847 extra commits).

### 5.2 Caveats of (c)

- **gc in the user's repo can delete objects the cache depends on.** git warns about exactly
  this for `clone --shared`: objects that become unreferenced in the source "may be removed by
  normal Git operations"
  ([GIT git-clone.adoc#L80-L94](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/Documentation/git-clone.adoc#L80-L94)).
  **(derived)** Treat the cache as disposable. If `git log` fails with a missing object, drop
  the cache and fetch again. PR bases usually sit on long-lived branches, so this should be
  rare.
- **`--filter` on a URL turns the cache into a partial clone.** Git adds
  `remote.<url>.promisor=true` and `partialclonefilter` and upgrades the repository format to
  version 1 (tested 2026-09-26; code:
  [GIT builtin/fetch.c#L2351-L2377](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/builtin/fetch.c#L2351-L2377),
  [list-objects-filter-options.c#L364-L394](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/list-objects-filter-options.c#L364-L394)).
  That is harmless in the cache and one more reason to avoid option (a).
- **Missing trees.** Any command that reads trees of remote-only commits, such as a future
  diff, would fail or lazily fetch. `GIT_NO_LAZY_FETCH` / `--no-lazy-fetch` exist from git 2.45
  ([GIT git.adoc#L949-L952](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/Documentation/git.adoc#L949-L952),
  [RelNotes 2.45.0#L120](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/Documentation/RelNotes/2.45.0.adoc#L120)).
  **(derived)** `Git::message()` uses `%B`, which lives in the commit object, so it works if it
  gets the same env var.
- **(derived)** Keep FETCH_HEAD and maintenance out of the cache: pass `--no-write-fetch-head`
  ([GIT fetch-options.adoc#L139](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/Documentation/fetch-options.adoc#L139))
  and `--no-auto-maintenance`, and run gc on the cache yourself if at all.
- **(derived)** Every `git` call that may meet a remote-only commit must carry the env var:
  `load`, `message`, and later ones. The simplest place is `Git::command()`, when a cache
  directory is set.
- **(derived)** A stale local clone makes the fetch also bring in newer upstream `main`
  commits that the PR heads build on. Those commits are "remote-only" too and should be drawn
  that way.

## 6. Authentication without requiring gh

Recommended order **(derived)**. Every step may fail silently and fall through to the next:

1. **`GH_TOKEN`, then `GITHUB_TOKEN`**, for github.com. For GitHub Enterprise Server hosts use
   `GH_ENTERPRISE_TOKEN`, then `GITHUB_ENTERPRISE_TOKEN`. This is gh's precedence
   ([GHCLI environment](https://cli.github.com/manual/gh_help_environment)).
2. **`gh auth token [--hostname HOST]`** if `gh` is on PATH. It "outputs the authentication
   token for an account on a given GitHub host"
   ([GHCLI gh auth token](https://cli.github.com/manual/gh_auth_token)). Tested 2026-09-26: it
   exits 0 when logged in. Run it with a null stdin and a timeout.
3. **`git credential fill`** with `protocol=https\nhost=<host>\n\n` on stdin. It "will attempt
   to add 'username' and 'password' attributes … by reading config files, by contacting any
   configured credential helpers, or by prompting the user"
   ([GIT git-credential.adoc#L25-L34](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/Documentation/git-credential.adoc#L25-L34)).
   This reuses GCM, osxkeychain, wincred or `gh auth git-credential`, whichever the user has
   configured. To make sure it never prompts, set:
   - `GIT_TERMINAL_PROMPT=0`: "git will not prompt on the terminal"
     ([GIT git.adoc#L758-L760](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/Documentation/git.adoc#L758-L760)).
   - `-c credential.interactive=false`, which "Some credential helpers respect"
     ([GIT config/credential.adoc#L12-L18](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/Documentation/config/credential.adoc#L12-L18)).
   - `GCM_INTERACTIVE=never`: GCM will then "fail if interaction is required"
     ([GCM environment.md#gcm_interactive](https://github.com/git-ecosystem/git-credential-manager/blob/main/docs/environment.md#gcm_interactive)).

   **Tested 2026-09-26:** for `host=github.com` it returned a token via the configured
   `gh auth git-credential` helper, and that token worked as `Authorization: Bearer` against
   the REST API (limit 5000). For `host=example.invalid` it failed with "Cannot prompt because
   user interactivity has been disabled" and exit 128. GCM printed network "probe" warnings
   first, so this call can hit the network for unknown hosts.

   **(derived)** The password might be a real password rather than a token. The API rejects
   those, so a 401 should drop to step 4, not fail the feature.
4. **Unauthenticated**, for public repositories only. Forks-via-GraphQL is unavailable here (§4).

**Private repositories.** The API needs a token (§3). **(derived)** The git fetch itself uses
the user's normal git auth (credential helper or SSH keys) as long as the fetch URL is the
remote's own URL (§7). An SSH remote may prompt for a passphrase or host key on the TTY.
Running fetch with stdin from `/dev/null` plus `GIT_TERMINAL_PROMPT=0` does not cover SSH
prompts. Whether to add `GIT_SSH_COMMAND="ssh -o BatchMode=yes"` is **(unverified)**: it would
override the user's `core.sshCommand`.

**Security (derived).**

- Never put the token in a command line, a log, `GitError::Failed { args, stderr }`, the status
  bar, or a panic message.
- Keep it in a type whose `Debug` impl prints `***`.
- Send it only to the API host it was obtained for.
- git fetch needs no token from parterre, because git does its own auth.

## 7. Which GitHub repository, and which one is the base

**URL forms (derived from GH's parser).** gh accepts:

- `https://host/o/r(.git)`
- `ssh://git@host(:port)/o/r(.git)`
- scp-like `git@host:o/r(.git)`, rewritten to `ssh://`
- `git+https`/`git+ssh`

It takes owner and repo from the first two path segments and strips `.git`
([GH git/url.go#L29-L60](https://github.com/cli/cli/blob/0cf1092493af067646fc5f3db9421c6a6ec9c938/git/url.go#L29-L60),
[GH internal/ghrepo/repo.go#L61-L73](https://github.com/cli/cli/blob/0cf1092493af067646fc5f3db9421c6a6ec9c938/internal/ghrepo/repo.go#L61-L73)).
The host is `github.com`, `*.ghe.com`, or a configured GHES host. GHES serves its API at
`http(s)://HOSTNAME/api/v3`
([GHDOCS GHES quickstart](https://docs.github.com/en/enterprise-server@latest/rest/quickstart)).
**(derived)** Without `gh`, parterre cannot know which other hosts are GHES. Either accept a
per-repo setting, or probe `https://HOST/api/v3/meta`.

**`insteadOf`.** "Any URL that starts with this value will be rewritten"
([GIT config/url.adoc#L1-L10](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/Documentation/config/url.adoc#L1-L10)).
Both `git remote get-url` ("Configurations for `insteadOf` and `pushInsteadOf` are expanded
here", [GIT git-remote.adoc#L137-L140](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/Documentation/git-remote.adoc#L137-L140))
and `git ls-remote --get-url` ([GIT git-ls-remote.adoc#L56-L59](https://github.com/git/git/blob/e9019fcafe0040228b8631c30f97ae1adb61bcdc/Documentation/git-ls-remote.adoc#L56-L59))
apply it. Tested 2026-09-26: with `url.https://github.com/.insteadOf=gh:`, both printed
`https://github.com/aquamoth/parterre` for remote URL `gh:aquamoth/parterre`, while
`git config remote.origin.url` printed the raw `gh:…`. **Use `get-url`, not the raw config.**

**Choosing the base like gh does.**

- gh reads `remote.<name>.gh-resolved`, which `gh repo set-default` writes
  ([GH git/client.go#L165-L212](https://github.com/cli/cli/blob/0cf1092493af067646fc5f3db9421c6a6ec9c938/git/client.go#L165-L212)).
  The value `base` means "this remote". Any other value is `owner/repo`
  ([GH context/context.go#L61-L80](https://github.com/cli/cli/blob/0cf1092493af067646fc5f3db9421c6a6ec9c938/context/context.go#L61-L80)).
- Otherwise remotes are ranked `upstream` > `github` > `origin` > others
  ([GH context/remote.go#L59-L78](https://github.com/cli/cli/blob/0cf1092493af067646fc5f3db9421c6a6ec9c938/context/remote.go#L59-L78)).
  gh takes the first when it cannot prompt, or asks the API about the network when it can.
- The manual says the default repo is used for "viewing and creating pull requests"
  ([GHCLI gh repo set-default](https://cli.github.com/manual/gh_repo_set-default)).

**If origin is a fork.** `GET /repos/{o}/{r}`: "The parent and source objects are present when
the repository is a fork. parent is the repository this repository was forked from, source is
the ultimate source for the network"
([GHDOCS get a repository](https://docs.github.com/en/rest/repos/repos#get-a-repository)).
Tested 2026-09-26: `waldyrious/cli` → `fork=true, parent=cli/cli, source=cli/cli`.

**(derived) Recommendation:**

1. Honour `gh-resolved` if it is set.
2. Otherwise take the highest-ranked GitHub remote.
3. Call `GET /repos` once. If it is a fork, list PRs on `parent`, and on `source` too if the
   two differ. This could be a setting.
4. Fetch from the parent's URL, built in the same scheme and host as the remote.

## 8. Other forges (for a later `Forge` trait)

| Forge | PR head ref | Merge ref | Listing API | Auth |
|---|---|---|---|---|
| GitHub | `refs/pull/N/head` (§2) | `refs/pull/N/merge` for open, mergeable PRs, sometimes kept after close (§2) | REST `/repos/{o}/{r}/pulls`, GraphQL | 60/h unauthenticated; token 5000/h |
| GitLab | `refs/merge-requests/<iid>/head`; "deleted 14 days after a merge request is closed or merged" ([GLDOCS MR troubleshooting](https://docs.gitlab.com/user/project/merge_requests/merge_request_troubleshooting/)) | **(unverified)** | `GET /projects/:id/merge_requests?state=opened` with `iid`, `sha`, `source_project_id`, `source_branch`, `target_branch`, `draft` ([GLDOCS MR API](https://docs.gitlab.com/api/merge_requests/)) | "All API calls to non-public information require authentication" (same page) |
| Azure DevOps | Not mentioned in any Microsoft doc I found: **(unverified)** | `refs/pull/<id>/merge` ("Git repo pull request: `refs/pull/1/merge`", [ADO predefined variables](https://learn.microsoft.com/en-us/azure/devops/pipelines/build/variables?view=azure-devops)) | `GET https://dev.azure.com/{org}/{project}/_apis/git/repositories/{repo}/pullrequests?searchCriteria.status=active&api-version=7.1`; fields `lastMergeSourceCommit` (source head), `lastMergeCommit` ("If empty, the most recent merge is in progress or was unsuccessful"), `sourceRefName`, `isDraft`, `forkSource`; `$top`/`$skip` ([ADO Get Pull Requests](https://learn.microsoft.com/en-us/rest/api/azure/devops/git/pull-requests/get-pull-requests?view=azure-devops-rest-7.1)) | OAuth scope `vso.code` (same page); in practice a PAT **(unverified)** |
| Bitbucket Cloud | **(unverified)**: I found no PR refs that git can fetch | n/a | `GET /repositories/{workspace}/{repo_slug}/pullrequests` **(unverified; the docs page did not load)** | **(unverified)** |

**(derived)** For ADO the head SHA comes from `lastMergeSourceCommit`. Fetching it by SHA from
the target repository is **(unverified)**, since ADO's `uploadpack` policy is not documented.
The merge ref contains the head as its second parent.

**Trait sketch (derived).** Every forge returns the same thing: a list of
`RemoteTip { oid, label, kind, url, fetch_spec }` plus one fetch URL.

```rust
pub trait Forge {
    fn detect(remotes: &[RemoteUrl]) -> Option<ForgeRepo>;               // host + owner/repo, base selection
    fn list_tips(&self, repo: &ForgeRepo, auth: &Auth, opts: &ForgeOptions)
        -> Result<Vec<RemoteTip>, ForgeError>;                           // PRs (+ fork branches)
    fn fetch_url(&self, repo: &ForgeRepo) -> String;                    // where to fetch objects from
}
```

## 9. TortoiseGit comparison

- TortoiseGit's graph walks refs through `git log --all` / `--branches`. Its default decoration
  set is `HEAD`, `refs/heads/`, `refs/tags/`, `refs/remotes/`, `refs/stash` and
  `refs/replace/` ([TGNOTE §1, Stage A](tortoisegit-revision-graph.md#stage-a-gits-revision-walk---simplify-by-decoration)),
  so it never looks at a forge.
- A code search of TortoiseGit's default branch for `api.github.com` found no hits, and
  `refs/pull` appears only in a bundled git manual page
  (tested 2026-09-26: `gh api search/code -f q='… repo:TortoiseGit/TortoiseGit'`).
  **(derived)** This feature is a deliberate deviation. When built, it needs a code comment
  and an entry in `TODO.md`, per `CLAUDE.md`.

## 10. UX and graph (derived)

- **Never block.** `main()` and `reload()` load synchronously today. Keep that for the local
  repository. Then start a `ForgeLoader` thread that sends `ForgeUpdate` messages over `mpsc`,
  the same pattern as the layout and message threads in `app.rs`. When they arrive, reload
  with the extra tips and the env var, and merge the result into the scene. A status bar item
  shows "Fetching 63 PRs…" or the error.
- **Offline or failing.** Fall back to the last cache. The cache repo keeps its refs, and a
  small JSON metadata file keeps titles and a timestamp. Mark the result stale ("PRs as of
  10:42"). All errors are non-fatal.
- **Opt-in.** Off by default. Turned on per repository (stored in settings next to the view
  state) and with a CLI flag such as `--forge=github`. `--screenshot` and `--export` stay
  offline unless the flag is given.
- **Colours.** Add `RefKind::PullRequest` and `RefKind::ForkBranch`, or one
  `RefKind::Forge { kind }`, each with its own palette entry and legend swatch. Commits that
  exist only in the cache get a distinct node style, such as a dashed outline or a lighter fill.
  That needs a `Commit::remote_only` flag, which core can set: the commit is absent from the
  plain `git log` or from the user's `rev-list --all`.
- **Labels.** `#123 Title…` for PRs: the PR number plus a truncated title, with author and
  draft state in the tooltip. `owner:branch` for fork branches. Hide PRs whose head is already
  in the local graph *and* decorated by a local or remote branch, or show them as a second
  label on that node.
- **Large repositories.** rust-lang/rust's 1,385 open PRs each add a decorated tip, so the
  simplified graph gains up to about 1,385 nodes fanning off `main`. Remedies:
  1. a cap: newest N by `updated`;
  2. filters: mine, draft or not, base branch = one of the visible branches;
  3. hiding PRs whose head is older than X.

  "Other refs" already has a toggle and name patterns (`pattern.rs`). Reuse them.
- **Refresh.** A manual "Refresh PRs" action, plus an optional timer only when authenticated,
  since unauthenticated 304s count against the limit (§3). Send ETags.

## 11. Proposed architecture

**parterre-core, no GUI:**

- `forge/mod.rs`:
  - `RemoteUrl` parsing, with URLs taken from `git remote get-url`;
  - `ForgeRepo`;
  - `RemoteTip`;
  - the `Forge` trait;
  - base selection (`gh-resolved`, remote ranking).
- `forge/github.rs`:
  - REST calls for PRs, `GET /repos` (parent), and optionally forks and branches;
  - ETag handling;
  - JSON types.
- `forge/auth.rs`: the chain from §6. It shells out to `gh`/`git credential` with a null stdin,
  a timeout and the no-prompt env vars.
- `forge/cache.rs`:
  - creates `<cache>/<hash of repo root>.git` (bare, with alternates pointing at
    `git rev-parse --git-common-dir`/objects);
  - fetches with `--no-tags --no-write-fetch-head --no-auto-maintenance --filter=tree:0 --stdin`;
  - writes metadata JSON.
- `git.rs`: `Git` gets `extra_object_dirs: Vec<PathBuf>`, which `command()` puts in
  `GIT_ALTERNATE_OBJECT_DIRECTORIES`, and `load_with(extra_tips: &[(Oid, GitRef-ish)])`, which
  appends to the `--stdin` list.
- `repo.rs`: new `RefKind` variants plus `Commit::remote_only`.

**Transport:**

- HTTP behind a cargo feature such as `forge-http`, in core or a new small
  `parterre-forge` crate. Measured on 2026-09-26 with `cargo tree` on an empty crate:

  | Option | Crates | TLS |
  |---|---|---|
  | `ureq` 3.4 (`json`) | 35 | rustls + `ring`, `webpki-roots` |
  | `reqwest` 0.13 (`blocking,json,rustls`) | 115 | tokio, hyper, `aws-lc-sys`, which needs a C toolchain / CMake on Windows **(unverified for this build)** |

  → **ureq**. It is blocking, which suits a worker thread.
- The zero-dependency alternative: shell out to `curl` (headers via `-D`, ETag via
  `If-None-Match`, token via `-H @-` on stdin so it stays out of `ps`) or to `gh api`
  (`gh` must then be installed). `curl` ships with Windows 10+ and macOS **(unverified for all
  targets)**.
- The default build could use the `curl` path, with ureq behind the feature, or the reverse.
  That is an open question.

**App:**

- The toggle.
- A background `ForgeLoader` (list → fetch → reload with tips) that sends `ForgeUpdate` over
  `mpsc`.
- Palette entries, legend swatches and label formatting.
- A status bar item.
- A settings entry for the token-source preference and caps.

**Data flow:**

```
open repo → load() local (sync, as today) → show graph
          └─ if enabled: thread { detect base → auth chain → REST list open PRs (ETag)
                                  → git -C cache fetch <base url> --stdin (open PR heads / fork SHAs)
                                  → load_with(extra tips, alt dir = cache/objects) }
                         → mpsc → app swaps Repo, relayout, tips coloured by RefKind
```

## 12. Decisions and open questions

Answered by the human on 2026-09-26:

1. **Scope for version 1:** open PRs only. Fork branches that aren't PRs are left out.
2. **Toggle:** a new toolbar icon shows or hides PRs. It is disabled until the GitHub
   connection works: a GitHub base repository is detected and the PR list has loaded.
3. **HTTP stack:** `ureq`, behind a cargo feature.
4. **Which PRs:** no arbitrary cap. A PR is shown only when its base branch is visible in the
   graph.
5. **Fetching:** commits only (`--filter=tree:0`), as in §5. File diffs will need blobs and trees
   later; that is a future requirement.
6. **Remote-only commits:** a greyed-out node, and dashed edges to it if that looks good.
7. **GitHub Enterprise:** not in version 1.

8. **Which repository's PRs:** only PRs the user can act on (2026-09-26):
   - the open PRs of the repository `origin` points to, which you can review or merge;
   - if `origin` is a fork, also the fork's own PRs into its parent. GitHub stores a PR on its
     base repository, so these come from the parent's list, keeping those whose
     `head.repo.full_name` is the fork. The REST `head` filter needs a branch name
     (`user:ref-name`), so this filtering happens client-side **(derived)**.

   PRs between the parent and other forks are not shown.

Still open:

- **Cache location and clean-up:** `$XDG_CACHE_HOME/parterre/`, `%LOCALAPPDATA%` or
  `~/Library/Caches`? When is it deleted?

## 13. Experiments (all 2026-09-26, git 2.43.0, in `/tmp`)

| # | Command (abridged) | Result |
|---|---|---|
| 1 | `git ls-remote https://github.com/aquamoth/parterre` | `refs/pull/{1-10,22-24,30}/head`; only `30/merge`. #30 is the only open PR |
| 2 | `curl …/repos/aquamoth/parterre/pulls?state=all&per_page=100` (unauth) | 14 PRs, fields as in §3, `x-ratelimit-limit: 60` |
| 3 | Same with `If-None-Match` (unauth) ×2 | 304, remaining 58 → 57: 304s count |
| 4 | Same with token from `git credential fill` and `If-None-Match` ×2 | 304, remaining 4960 → 4960: 304s free |
| 5 | `git ls-remote https://github.com/rust-lang/rust 'refs/pull/*'` | 98,795 head / 12,697 merge refs, 6.9 MB, 0.76 s |
| 6 | `gh api --paginate …/rust/pulls?state=open` vs merge refs | 1,385 open; 1,374 with merge ref, 11 without (sampled: `dirty`); 11,323 merge refs on non-open PRs |
| 7 | `GIT_TRACE_PACKET=1 git ls-remote <url> 'refs/pull/*'` | No `ref-prefix` sent (client-side filter) |
| 8 | `GIT_TRACE_PACKET=1 git fetch --dry-run <url> '+refs/pull/*/head:…'` | `ref-prefix refs/pull/` sent |
| 9 | `git fetch <url> refs/pull/30/head` in a clone | Worked; `FETCH_HEAD` written; no remote added |
| 10 | `git fetch <url> '+refs/pull/*/head:refs/parterre/github/pull/*'` (no `--no-tags`) | Also created `refs/tags/v0.2.0`, `v0.3.0` |
| 11 | Cache repo with alternates, `fetch --no-tags --filter=tree:0 …pull/*/head` | 1 object fetched; cache config gained `remote.<url>.promisor`, format version 1 |
| 12 | `GIT_ALTERNATE_OBJECT_DIRECTORIES=cache/objects git log --stdin …%T…` in the user repo | Walked PR commit into local history; `%T` printed without tree object. Without env: `bad object` |
| 13 | cli/cli: fetch 63 open PR heads into the alternates cache | 105 commits, 51 KiB, 1.4 s. Without alternates: 12,378 commits, 5.7 MiB. `--shallow-exclude=trunk`: 136 commits |
| 14 | rust-lang/rust (tree:0 clone of main, 139 MiB, 9.6 s): `fetch --stdin` of 1,385 PR heads into the cache | 3,862 commits, 1.39 MiB, 4.1 s; `git log` main+tips ≈ same time as main alone |
| 15 | `git fetch https://github.com/cli/cli 020e1317…:refs/probe/x` (fork-only commit) | Worked (cross-fork fetch by SHA); bogus SHA → `not our ref` |
| 16 | `git fetch https://github.com/DynamoIVII/cli refs/heads/patch-1:…` | Worked from the fork URL, no remote |
| 17 | `gh api graphql` forks(first:100, PUSHED_AT) × refs(first:20) | Cost 1; 1,239 tips; many are copies of upstream branches |
| 18 | `curl -X POST https://api.github.com/graphql` (unauth) | 403; `/rate_limit` shows `graphql.limit: 0` |
| 19 | REST and GraphQL compare `trunk...DynamoIVII:cli:patch-1` | `diverged`, ahead 1, behind 4 |
| 20 | `curl …/repos/waldyrious/cli` | `fork: true`, `parent` = `source` = `cli/cli` |
| 21 | `git remote get-url` / `ls-remote --get-url` with `insteadOf` | Both return the rewritten URL; raw config does not |
| 22 | `git credential fill` with `GIT_TERMINAL_PROMPT=0 GCM_INTERACTIVE=never` | github.com: token via the gh helper. Unknown host: fails fast, exit 128, after GCM network probe warnings |
| 23 | `cargo tree` on scratch crates | ureq 35 crates (ring); reqwest 115 (tokio, aws-lc-sys) |
| 24 | `gh api search/code` on TortoiseGit | No `api.github.com`; `refs/pull` only in a bundled git doc |

## 14. PR tags on nodes (derived)

Clarified by the human on 2026-09-26: a PR is shown as a **tag on an existing node**, like a ref
label. It is not a new line and not a new node. Clicking the tag opens the PR in the browser.

- **Placement:** the tag goes on the node whose commit is `head.sha`. After a fetch, that is
  normally the node carrying `origin/<head.ref>`. The PR head can be missing from the graph in
  two cases:
  1. **The commit is local but not a node.** The revision graph keeps only decorated commits
     and branch points. If `origin/<head.ref>` is stale, or the local branch has unpushed
     commits, the PR head may be a collapsed commit in the middle of a branch. Fix: pass PR
     heads to the reduction as decorations, so the commit becomes a node, like a tag.
  2. **The commit is not local.** Someone else pushed to the PR branch after your last fetch,
     for example with GitHub's "Update branch" button, a review suggestion or a bot. Other
     people's PRs are always in this case. Slice 2 fetches such commits as greyed-out nodes.

  The human accepts this as a belt-and-braces rule and will revisit it once it's implemented.
- **Tag look:** a pull-request glyph plus the number, like T3 Code's `⤴ 30`. It is not the text
  `#12`. The glyph (two circles, a stem and a curved arrow, in the style of Lucide's
  `git-pull-request`) is drawn with the egui painter. So there is no icon font, no SVG crate
  and no image asset, and it scales with zoom. The tooltip shows the title, author, draft state
  and `head → base`.
- **Cost:** one extra row on a few nodes. Layout, physics and edges are unchanged.
- **Opening the browser:** no crate. eframe's `links` feature pulls in `webbrowser` 1.2, which
  brings `url`, and through it `idna` and about a dozen ICU crates (`cargo tree`, 2026-09-26).
  That is a lot of code and binary size for one call. Instead, spawn the platform opener
  directly, with the URL as an argument and no shell:
  - `xdg-open` on Linux;
  - `open` on macOS;
  - on Windows, `ShellExecuteW` through `windows-sys`, which winit already depends on, or
    `explorer.exe <url>`. Never `cmd /c start`, because cmd interprets `&` and `^`.

  Only open URLs that start with the forge's `https://` web origin. Also add "Open PR in
  browser" to the node context menu.
- **Order of work:** PR tags on commits that are already local need only the REST call. They
  need no fetch, no cache and no alternates, so they are a small first slice. Fetching other
  people's PR heads as greyed-out nodes comes second.
- **Other platforms:** keep the core model forge-neutral: `PullRequest { id, title, author,
  draft, head_sha, head_repo, head_branch, base_branch, web_url }`, produced by the `Forge` trait
  (§11). For Azure DevOps, the PR list is expected to give `sourceRefName`, `targetRefName` and
  `lastMergeSourceCommit` **(unverified)**.


## 15. Azure DevOps origins (findings, 2026-09-26; not built)

Parked until someone asks for it (`TODO.md`, Planned). What was found while estimating it:

- **Reusable as is:** the model (`forge::PullRequest`, `Remote`, `PullRequests::heads`, which
  places pull requests on commits and finds their base refs by remote and branch name), the
  labels, clicking and the node menu, the HTTP client (ureq), and the app's cache, back-off
  and error dialog (`app/pull_requests.rs`). `PullRequest::base_repo` and `Remote::repo` are
  plain strings; for ADO they would hold `org/project/repo`.
- **Needed:**
  1. **Recognising the remote.** Forms: `https://dev.azure.com/{org}/{project}/_git/{repo}`
     (also with `{org}@` before the host, as the human's Apps remote has),
     `https://{org}.visualstudio.com/[DefaultCollection/]{project}/_git/{repo}`,
     `git@ssh.dev.azure.com:v3/{org}/{project}/{repo}` and
     `{org}@vs-ssh.visualstudio.com:v3/{org}/{project}/{repo}`. Project names can hold spaces,
     percent-encoded in URLs **(unverified)**.
  2. **A `Forge` choice** where `github::load` is called today: detect the forge from
     `origin`'s URL, and name it in the wording ("on GitHub" in the status text, the
     tooltips, the dialog, the legend).
  3. **Asking ADO.** No GraphQL. `GET https://dev.azure.com/{org}/{project}/_apis/git/
     repositories/{repo}/pullrequests?searchCriteria.status=active&api-version=7.1` (§8)
     lists active pull requests with `pullRequestId`, `title`, `isDraft`, `createdBy`,
     `sourceRefName`, `targetRefName` and `lastMergeSourceCommit.commitId` (the head).
     `searchCriteria.sourceRefName` narrows it to one branch **(unverified)**, but the Apps
     repository has about 160 branches, and ADO repositories tend to have few active pull
     requests, so one listing call (with `$top`) is cheaper than a call per branch, unlike on
     GitHub. The page to open is `https://dev.azure.com/{org}/{project}/_git/{repo}/
     pullrequest/{id}` **(unverified)**; the browser's allow list (`browser.rs`) would take
     `https://dev.azure.com/` and `https://{org}.visualstudio.com/` too.
  4. **Signing in.** There is no `gh` for ADO. Two ways, both on the human's machine:
     - **`git credential fill`** for `https://dev.azure.com/{org}/{project}/_git/{repo}`,
       non-interactive as in §6 (`GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=never`). It reuses
       Git Credential Manager, configured there for dev.azure.com with
       `credential.https://dev.azure.com.usehttppath=true`, so the path must be passed. It is
       git, so no new program (rule 1 of the roadmap). Whether ADO wants the token GCM hands
       back as `Bearer` (an Entra ID token) or in Basic auth (a PAT) is **unverified**:
       probably Bearer; a token starting `eyJ` is a JWT.
     - **`az account get-access-token --resource 499b84ac-1321-427f-aa17-267ca6975798`**
       (Azure DevOps's resource id) **(unverified here)**; `az` is installed there too, but
       it would be a second program to rely on.
  5. **Forks:** ADO has forks (`forkSource` in the list), but they are rare there; leave
     them out, as the GitHub slice 1 leaves out other people's forks.
- **Limits:** ADO throttles by "TSTUs" per user rather than a fixed hourly count
  **(unverified)**; it sends `Retry-After` and `X-RateLimit-*` headers when it does, which the
  budget code already understands for GitHub.
- **Trying it:** the human's `Cosmo/Apps` has `origin` on dev.azure.com, and Git Credential
  Manager and `az` are set up. A live test uses the human's own token against their own
  organisation; ask first.
- **Size:** about the core half of the GitHub slice (URL parsing, one REST call, JSON, sign-in,
  tests), plus wording; no new crates.
