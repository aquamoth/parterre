# GitHub Releases as the home of nightly and canary builds

Research note for [#392](https://github.com/aquamoth/parterre/issues/392), part of the map
[#390](https://github.com/aquamoth/parterre/issues/390): *how should nightly and canary builds live
on GitHub Releases? Should each build get its own release and tag, or should a moving tag
(`nightly`, `canary`) have its assets replaced? What does each choice mean for the update check,
the Latest badge, watchers' notifications, list growth, pruning, limits and `--generate-notes`?
And what do other projects do?*

Sources are official docs, source code at pinned commits, and experiments I ran. A statement that
is my own conclusion is marked **(derived)**. A statement I could not check is marked
**(unverified)**. A statement I checked by running something is marked **(tested 2026-10-16)**.
Experiments used git 2.53.0.windows.3 and the live GitHub API.

## TL;DR

- **Recommendation (derived): one release and one tag per build, in this repository.**
  - Mark each one `--prerelease`.
  - Create the new release before pruning the old ones.
  - Prune with `gh release delete <tag> --cleanup-tag --yes`, keeping the newest canary and the
    last 7 nightlies.
  - Pass `--notes-start-tag` explicitly when generating notes for nightly and rc releases.
  - Make `release.yml` ignore ring tags.
- **Why not a moving tag:**
  - **It can't keep 7 nightlies.** A moving tag holds only one build per ring, which rules it out
    for nightly. For canary it is merely possible.
  - **Its tag doesn't parse as a version.** `nightly` is not a version, so the update check
    would have to find the version somewhere else.
  - **The Download URL scheme breaks.** The URL is `releases/download/v{version}/{file}`.
  - **Asset bytes change under a fixed URL.** That breaks hash-pinned installers (neovim
    #38585).
  - **Builds can be half-replaced.** While assets are uploaded, a release is partly old and
    partly new.
  - **It breaks `git fetch --tags` in every clone.** The fetch fails with *would clobber
    existing tag* (tested).
  - **It can't coexist with immutable releases.**
- **Latest badge and `/releases/latest`:** safe either way. "The latest release is the most
  recent non-prerelease, non-draft release", and prereleases cannot be set as latest. No
  `--latest=false` is needed. None of the projects surveyed passes it either.
- **The update check (`?per_page=30`) still finds stable and rc** (derived).
  - The list is ordered newest first by `created_at`. That is the tag's date, not the publish
    date (tested).
  - Retention caps ring releases at 8, which leaves at least 22 slots, and the newest stable and
    rc are always among the newest anyway.
  - **But `update.rs` must learn rings.** Today an rc or dev build takes the highest
    *prerelease* of any kind, so it would be offered nightlies and canaries. Already shipped rc
    binaries (≤ 1.0.0-rc1) will do exactly that if ring tags parse as higher versions. That is
    an input to #397.
- **Notifications are the real cost.**
  - Creating (publishing) a release notifies everyone who watches the repository's releases.
  - GitHub has no setting to watch releases but not prereleases. The community request for one
    has stood unanswered since 2022.
  - With one release per build, every canary (up to 20 merges a day in early October) and every
    nightly notifies watchers.
  - Today the repository has 0 watchers. The way out, if that changes, is a separate releases
    repository (yt-dlp's pattern), or a moving canary that is updated in place. Updating is not
    documented to notify (unverified).
- **Limits are no concern.**
  - 1000 assets per release, each under 2 GiB, and no total size or bandwidth limit. A parterre
    release is 11 assets, about 170 MB.
  - Content creation is limited to 80 requests a minute and 500 an hour. One build is about 15
    requests.
  - The unauthenticated update check is unchanged: one request, out of 60 an hour per IP.
- **`--generate-notes` picks its own starting point, and that is a trap.**
  - From this repository's own releases: a stable release compares with the previous *stable*,
    and a prerelease with the previous release of *any* kind.
  - So once ring builds exist, an rc's notes would start at the latest nightly or canary, and a
    nightly's would start at the canary of the same commit (empty). The stable notes are
    unaffected.
  - Pass `--notes-start-tag`: the previous rc or stable for an rc, and the previous nightly for a
    nightly.

## Sources (pinned)

| Short name | What | Link |
|---|---|---|
| REST | REST API, Releases (API version 2022-11-28), read 2026-10-16 | https://docs.github.com/en/rest/releases/releases?apiVersion=2022-11-28 |
| ABOUT | About releases (limits) | https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases |
| NOTES | Automatically generated release notes | https://docs.github.com/en/repositories/releasing-projects-on-github/automatically-generated-release-notes |
| IMM | Immutable releases | https://docs.github.com/en/code-security/supply-chain-security/understanding-your-software-supply-chain/immutable-releases |
| NOTIF | Configuring notifications (custom watch: releases) | https://docs.github.com/en/account-and-profile/managing-subscriptions-and-notifications-on-github/setting-up-notifications/configuring-notifications |
| RATE | REST rate limits | https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api |
| TOKEN | `GITHUB_TOKEN` does not trigger workflows | https://docs.github.com/en/actions/concepts/security/github_token |
| GHCREATE | `gh release create` manual, and source at `cli/cli@3d7967d` | https://cli.github.com/manual/gh_release_create, https://github.com/cli/cli/blob/3d7967df5a94052b0554898ffa02526f0f90b8c9/pkg/cmd/release/create/create.go#L497-L557 |
| GHDELETE | `gh release delete` manual | https://cli.github.com/manual/gh_release_delete |
| FETCH | git-fetch manual (tag updates since 2.20, `--prune-tags`) | https://git-scm.com/docs/git-fetch |
| C10776 | GitHub community: "Ability to get notifications for Releases but not Pre-Releases" | https://github.com/orgs/community/discussions/10776 |
| PURS | PureScript: nightly prereleases notify watchers | https://discourse.purescript.org/t/purescript-continuous-prereleases/3006 |

Project workflows are linked at pinned commits in §7.

---

## 1. What parterre does today (context)

- **The update check.**
  [`github.rs`](../../crates/parterre-telemetry/src/github.rs) asks `/releases/latest` for a
  release build. A build whose version has a pre-release part (an rc or a dev build) asks
  `/releases?per_page=30` instead. Both are unauthenticated. Drafts are dropped, and only
  `tag_name` and `prerelease` are read.
- **The comparison.** [`update.rs`](../../crates/parterre-telemetry/src/update.rs) `newer` parses
  each tag with `Version::parse` and passes over tags that aren't versions. For a pre-release
  build it takes the highest version of any kind. *Download* opens
  `https://github.com/aquamoth/parterre/releases/download/v{version}/{file}`, and the file names
  carry the version (`parterre-1.0.0-x86_64-pc-windows-msvc.msi`).
- **Publishing.** [`release.yml`](../../.github/workflows/release.yml) runs on `push: tags: ["v*"]`
  and calls `gh release create "$GITHUB_REF_NAME" dist/* --verify-tag --generate-notes`, adding
  `--prerelease` for a tag with a `-` in it.
- **Current state (tested).**
  - 15 releases, 15 tags, 0 watchers.
  - `immutable-releases` is `{"enabled":false}`.
  - The 1.0.0 release has 11 assets totalling about 170 MB.

## 2. Release per build vs moving tag

| | Release + tag per build | Moving tag, release updated in place (ghostty, wezterm) | Moving tag, release deleted and recreated (neovim, rust-analyzer) |
|---|---|---|---|
| Keeps the last 7 nightlies | yes | no: one build per tag | no |
| Tag is a version `Version::parse` reads | yes, if #397 says so | no: `nightly` | no |
| Download URL `releases/download/v{version}/…` | unchanged | needs a ring-specific URL | needs a ring-specific URL |
| Bytes behind a URL never change | yes | no (breaks hash-pinned installs, e.g. neovim #38585) | no |
| A moment with no release, or mixed assets | none: `gh` publishes only after uploading (GHCREATE) | mixed old and new assets while uploading | no release between delete and create (neovim #15709) |
| Notifies watchers | every build | not documented to (unverified) | every build |
| Remote release and tag count | ≤ 8 extra, by pruning | 2 | 2 |
| Effect on clones | ring tags accumulate locally (§5) | `git fetch --tags` fails (§5) | same as in place |
| Immutable releases | compatible | incompatible: assets and tag are locked | incompatible: a tag name can't be reused |

- **GHCREATE, on how `gh` publishes.** When assets are given, `gh release create` makes separate
  calls: it creates the release as a draft, uploads the assets, and then publishes it. It also
  refuses when a published release with that tag already exists.
- **IMM, on immutable releases.**
  - Once published, the tag "cannot be changed, and cannot be deleted while the release exists".
  - Assets "are protected from modification or deletion".
  - "If you delete the immutable release, you can delete the tag, but you cannot reuse the same
    tag name."
  - Per-build tags are never reused, so pruning works with immutable releases turned on
    (derived).

## 3. The Latest badge and `/releases/latest`

- **REST, Get the latest release:** "The latest release is the most recent non-prerelease,
  non-draft release, sorted by the `created_at` attribute. The `created_at` attribute is the date
  of the commit used for the release, and not the date when the release was drafted or
  published."
- **REST, `make_latest`:** `true`, `false` or `legacy`, default `true`. "Drafts and prereleases
  cannot be set as latest."
- **GHCREATE:** `--latest` defaults to "automatic based on date and version", and
  `--latest=false` explicitly does not set it.
- So a nightly or canary marked `--prerelease` never becomes Latest, and a stable build's check
  (`/releases/latest`) never sees one. No `make_latest` is needed. neovim, rust-analyzer,
  ghostty and wezterm rely on `prerelease` alone (§7).
- **What `created_at` really is (tested).** It is the *tag's* date: the tagger date of an
  annotated tag, or the commit date of a lightweight one.
  - parterre's `v1.0.0` is an annotated tag dated 18:40:57Z on a commit from 2026-10-09, and the
    release's `created_at` is 18:40:57Z.
  - neovim's lightweight `nightly` has the commit's date.

## 4. The update check with ring builds in the list

- **Order (derived from REST and the observed lists).** The list endpoint documents no order. In
  practice it is newest `created_at` first: parterre's list and neovim's both come back that way
  (tested).
- **The 30-release window still works (derived).**
  - Retention keeps at most 8 ring releases (1 canary and 7 nightlies), so 22 or more slots
    remain for stable and rc.
  - The newest stable and the newest rc are the newest of their kind by date, so they can't drop
    out of the window while 8 ring releases sit in front of them.
  - A ring build left unpruned (a failed prune) costs one slot each. A prune step that fails
    loudly is enough.
- **Rings must come from the tag (derived).**
  - `prerelease` is `true` for rc, nightly and canary alike, so it can't tell rings apart.
  - `newer` currently lets an rc or dev build take the highest version of any kind. An rc
    installation would be offered nightlies and canaries.
  - The fix belongs in `update.rs`: classify each tag into a ring and keep the ones at or before
    the installation's ring.
- **Already shipped binaries can't be fixed (derived).**
  - Shipped rc builds (`0.5.0-rc1` … `1.0.0-rc1`) run the old `newer`.
  - A ring tag that today's `Version::parse` accepts, and that sorts above them, will be offered
    to them. `v1.1.0-nightly.20261017` is one: `1.1.0` is greater than `1.0.0`.
  - A tag shape the old parser rejects keeps them blind to rings. A `v` followed by something
    that isn't `N.N.N`, or a pre-release identifier with a character outside `[A-Za-z0-9-]`,
    would do it.
  - Stable installations are safe either way, because they only read `/releases/latest`.
  - This, and how `nightly`/`canary` identifiers sort against `rc` (semver puts
    `1.1.0-nightly.x` *before* `1.1.0-rc1`), are inputs to #397.
- **Download.** A per-build tag `v{version}` with versioned file names keeps today's URL scheme
  unchanged. A moving tag would need `releases/download/nightly/…` and the version read from
  the release title or the file names.
- **Rate limits are unchanged.** It is still one unauthenticated request per check, and the
  limit is 60 an hour per IP (RATE). A separate releases repository (§6) would need a second
  request for the nightly and canary rings.

## 5. Tags: the remote and every clone

- **Remote (derived).** Releases per build with pruning leave at most 8 ring tags on GitHub at
  any time.
- **Clones keep the deleted tags (tested).** git's default tag auto-following fetches each new
  ring tag that points into fetched history. Tags are not pruned unless asked: FETCH says "Tags
  are not subject to pruning if they are fetched only because of the default tag
  auto-following". After one nightly was deleted and another created upstream:
  - `git fetch` added `v1.1.0-nightly.2` and kept `v1.1.0-nightly.1`;
  - `git fetch --prune --prune-tags` removed it.

  So a contributor's clone, and parterre's own graph of it, collects ring tags. Every merge's
  canary tag would end up in a clone that fetches often. `fetch.pruneTags=true` is the cure;
  `docs/` could suggest it (derived).
- **Moving tags break fetches (tested).** FETCH: "Since Git version 2.20 … any updates
  [to `refs/tags/*`] will be rejected without `+` in the refspec (or `--force`)." After the
  `nightly` tag moved upstream:
  - plain `git fetch` silently left the old `nightly` in place;
  - `git fetch --tags` failed (exit 1) with `! [rejected] nightly -> nightly (would clobber
    existing tag)`.
- **`release.yml` must not publish ring tags (derived).**
  - It runs on `push: tags: ["v*"]`. A `v`-prefixed ring tag pushed with `GITHUB_TOKEN` does not
    trigger it: TOKEN says "events triggered by the `GITHUB_TOKEN` will not create a new workflow
    run", except `workflow_dispatch` and `repository_dispatch`.
  - The same tag pushed with a PAT, a GitHub App token, or by hand *would* trigger a full
    release run.
  - So give `release.yml` a `tags-ignore` for the ring pattern, or a shape that `v*` doesn't
    match.

## 6. Notifications to people watching releases

- **NOTIF.** A custom watch can choose "releases" among issues, pull requests, discussions and
  security alerts.
- **REST, Create a release.** "This endpoint triggers notifications."
  - Publishing a draft is how `gh` creates a release with assets, and it is also a release
    being published.
  - Nothing documents a notification for *updating* a release or uploading an asset (unverified
    either way).
- **No filter for prereleases.** C10776, opened 2022-01-30, asks for "notifications for
  Releases but not Pre-Releases":
  - GitHub staff have not answered it;
  - commenters said in September 2026 that nothing had changed;
  - the Fluent Bit maintainer reported "notification overload" from nightly prereleases.

  PURS shows a user getting "a notification from the PureScript repo every night".
- **What it costs parterre (derived).**
  - **Release per build in this repository:** one notification per nightly (at most one a day)
    and one per canary (one per merge to `main`). First-parent commits on `main` ran 1–20 a day
    in October 2026.
  - **Today it costs nothing.** The repository has 0 watchers.
- **Ways out, if watchers appear (derived):**
  1. **Canary as a moving tag updated in place.** The tag moves and assets are uploaded with
     `--clobber`, with the drawbacks of §2 for the maintainer's own ring only. It is also not
     documented to be silent.
  2. **A separate releases repository** (yt-dlp-nightly-builds), for nightly and canary or for
     canary alone. It costs:
     - a second request in the update check;
     - a token secret, because `GITHUB_TOKEN` can't write to another repository;
     - tags in that repository point at *its* commits, so `--generate-notes` there wouldn't see
       parterre's pull requests (unverified).

## 7. What other projects do

Checked 2026-10-16 at the pinned commits.

- **neovim**
  ([release.yml#L268-L292](https://github.com/neovim/neovim/blob/be5167e21cfbb5d566c09a67c1852d0e18c62c38/.github/workflows/release.yml#L268-L292)).
  - **Mechanics:** a moving `nightly`, deleted and recreated every night (cron `5 5 * * *`):
    `gh release delete nightly --yes`, then `git push … :nightly`, then
    `gh release create nightly --prerelease --notes-file … --target $GITHUB_SHA`.
  - **Notes:** a template filled with `nvim --version` and the commit link, not generated.
  - **Stable:** stable releases also recreate a moving `stable` release, and then `vX.Y.Z`
    becomes Latest.
  - **Issues it hit:**
    - [#15709](https://github.com/neovim/neovim/issues/15709): the nightly upload sometimes
      fails, leaving no nightly;
    - [#38585](https://github.com/neovim/neovim/issues/38585): winget "Installer hash does not
      match", because the URL stays the same while the bytes change.
- **rust-analyzer**
  ([release.yaml#L210-L243](https://github.com/rust-lang/rust-analyzer/blob/71fbb7faf489656947a03566b32556d93c312c36/.github/workflows/release.yaml#L210-L243),
  [github-release action](https://github.com/rust-lang/rust-analyzer/blob/71fbb7faf489656947a03566b32556d93c312c36/.github/actions/github-release/main.js)).
  - **Mechanics:** a moving `nightly`. A vendored action, copied from wasmtime, deletes the
    release, force-updates the tag (`git.updateRef … force: true`) and recreates it with
    `prerelease: true`.
  - **Assets:** "We can't overwrite assets", so it deletes the asset and re-uploads it, with
    retries.
  - **Notes:** none.
  - **Weekly releases:** date-tagged, and *not* prerelease, so they become Latest.
- **ghostty**
  ([release-tip.yml#L123-L128, #L241-L252](https://github.com/ghostty-org/ghostty/blob/b3dc3d794513c11633e7c3347935693957744e83/.github/workflows/release-tip.yml#L123-L128)).
  - **Mechanics:** a moving `tip` tag (`git tag -fa tip` and `git push --force`) on one
    persistent prerelease. Assets are overwritten in place by `softprops/action-gh-release`.
  - **Trigger:** it runs after tests pass on `main`, which makes it a canary, not a nightly.
  - **Concurrency:** one run at a time, "to prevent corrupting our release artifacts".
  - **Per-commit copies:** kept outside GitHub, in an R2 bucket.
  - **Notes:** a static body.
- **wezterm**
  ([gen_windows_continuous.yml#L116-L120](https://github.com/wezterm/wezterm/blob/e2a98e1b8af156988ad0ac778658ac27b60a55fb/.github/workflows/gen_windows_continuous.yml#L116-L120)).
  - **Mechanics:** a prerelease called `nightly`, created in 2017, whose tag never moves.
    `gh release upload --clobber nightly …` runs in a retry loop with fixed asset names.
  - **No pruning:** 130 assets have piled up, stale ones included.
  - **Notes:** the body tells users to ignore the source archive, because it is from the old tag
    commit.
- **zed**
  ([release_nightly.yml#L587-L655](https://github.com/zed-industries/zed/blob/28d0a3547f16b9609b301b48ab363bfca649c864/.github/workflows/release_nightly.yml#L587-L655)).
  - **Nightly:** not on GitHub Releases. Builds go to object storage under a fixed `nightly/`
    path and a versioned path, and a `nightly` git tag is force-moved. It skips the build when
    `nightly` already points at HEAD.
  - **Preview channel:** `vX.Y.Z-pre`, one prerelease per tag, created as a draft and published
    with `gh release edit --draft=false`.
- **yt-dlp**
  ([release.yml#L292-L339](https://github.com/yt-dlp/yt-dlp/blob/897341241f2bfb2439fe0367f4c7c9ccd29a3828/.github/workflows/release.yml#L292-L339)).
  - **Mechanics:** one release per nightly, tagged with a timestamp, in a *separate* repository,
    [yt-dlp-nightly-builds](https://github.com/yt-dlp/yt-dlp-nightly-builds). It is not marked
    prerelease, so `releases/latest` there *is* the newest nightly.
  - **Notes:** a generated changelog.
  - **No pruning:** 874 releases since 2023-03, and 17 watchers.
  - **Same-repository fallback:** `gh release delete --yes --cleanup-tag`, then
    `gh release create --verify-tag`, with `sleep 5` between steps for a "deletion race
    condition".
- **alacritty and helix:** no nightlies. Both publish only on version tags.
- **Pattern (derived).**
  - Projects that keep one rolling build use a moving tag. Most of them pay with the problems
    of §2, and ghostty and zed keep per-build copies elsewhere.
  - The only project that keeps every build on GitHub Releases does it in a separate
    repository.
  - None prunes, because none keeps more than one build per ring in the main repository.
    parterre's "last 7 nightlies" is new ground, and per-build releases are the only GitHub
    Releases shape that holds it.

## 8. Generated notes for ring builds

- **NOTES** documents a *Previous tag* choice with an "auto" default, but not how "auto"
  chooses.
- **REST** `previous_tag_name` is "the name of the previous tag to use as the starting point";
  `gh` exposes it as `--notes-start-tag`.
- **What "auto" did in this repository (tested, from each release's *Full Changelog* link):**

  | Release | Compared with | So |
  |---|---|---|
  | `v1.0.0` | `v0.6.0` | a stable release compares with the previous stable, skipping 0.7.0-rc1…rc3 and 1.0.0-rc1 |
  | `v1.0.0-rc1` | `v0.7.0-rc3` | a prerelease compares with the previous release of any kind |
  | `v0.7.0-rc2` | `v0.7.0-rc1` | same |

- **Consequences (derived from the observed rule):**
  - **Stable:** unaffected by ring releases.
  - **rc:** would start at the newest nightly or canary. Pass
    `--notes-start-tag <previous rc or stable tag>` in `release.yml`.
  - **Nightly:** would start at the canary of the same or a later commit, often giving empty
    notes. Pass `--notes-start-tag <previous nightly>`.
  - **Canary:** create the new release before deleting the old one, so that "auto" finds the
    previous canary.
  - **Dead links:** the *Full Changelog* link of a ring build's notes points at a tag that
    pruning later deletes. That only matters while the release still exists, and the oldest
    kept nightly is the one whose link dies.
- **`--fail-on-no-commits`** ("Fail if there are no commits since the last release") doesn't fit
  "only when `main` has moved". The newest release is usually a canary of the same commit.
  Compare `main` with the previous nightly's tag instead (derived).

## 9. Limits

- **ABOUT:** "Up to 1000 release assets may be associated with a single release"; "Each file
  included in a release must be under 2 GiB"; "There is no limit on the total size of a
  release, nor bandwidth usage."
- **RATE:**
  - `GITHUB_TOKEN` gets 1,000 requests per hour per repository.
  - Content creation is limited to "no more than 80 content-generating requests per minute and
    no more than 500 content-generating requests per hour".
  - REST, Create a release: "Creating content too quickly using this endpoint may result in
    secondary rate limiting."
- **One build (derived):** about 15 such requests. That is 1 create, 11 uploads and 1 publish,
  plus deleting a release and a tag when pruning. Well within the limits at 20 merges a day.

## 10. Proposed shape (derived)

```sh
# after building and checking a ring build; TAG's shape is #397's decision
gh release create "$TAG" dist/* --verify-tag --prerelease \
  --title "parterre $VERSION" --generate-notes --notes-start-tag "$PREVIOUS_SAME_RING"
# then prune: everything of the ring past the newest N (canary 1, nightly 7)
gh release list --limit 100 --json tagName,isPrerelease,createdAt \
  | jq -r '<ring filter, sort by createdAt desc, .[N:][] .tagName>' \
  | xargs -r -n1 gh release delete --cleanup-tag --yes
```

- **Concurrency.** Use one concurrency group per ring, as ghostty does, so that two canaries
  don't publish or prune at once.
- **Pruning.** Delete releases by *ring tag pattern*, never by "prerelease", so rc releases are
  never touched.
- **`release.yml`:** add `tags-ignore` for the ring pattern, and `--notes-start-tag` for rc
  builds.
- **`update.rs`:** classify tags into rings (#397, #398).
- **Open for the human:** whether canary should be a moving tag in place to spare watchers
  (§6). With 0 watchers today, per-build is simpler and consistent.
