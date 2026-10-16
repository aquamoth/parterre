# PostHog symbol uploads for every canary and nightly build

Research note for [#394](https://github.com/aquamoth/parterre/issues/394), part of the map
[#390](https://github.com/aquamoth/parterre/issues/390): *what are PostHog's free-plan limits on
symbol sets (count, size, retention), can old ones be deleted from CI, how do other projects
keep the volume down, and how many symbol sets does a build make once one binary serves every
package of a platform?*

Sources are PostHog's own: the billing catalogue, its docs, and the source of the PostHog server,
its symbolication service (cymbal) and `posthog-cli`, read at commit
[`b344128a89`](https://github.com/PostHog/posthog/tree/b344128a899df77ee679b3fcd5c416860a44687a)
of `PostHog/posthog` (16 October 2026). Links to source below are to that commit. The log of
parterre's v1.0.0 release run is the source for what a release uploads today. A statement that
is my own conclusion is marked **(derived)**; one I could not check is marked **(unverified)**.
Nothing was changed in PostHog.

## TL;DR

- **The free plan has no limit on symbol sets**: not on their count, nor their total size. The
  only limits are 100 MB per file and an API rate limit (1,200 requests a minute, 12,000 an hour
  per key) that a build's handful of requests never nears (§1). What the free plan does limit is
  **100,000 exceptions a month** and **one project**: nightly and canary crash reports share
  the stable project, told apart by the ring on each event (§1).
- **PostHog deletes symbol sets itself, 30 days after their last use.** A cleanup job runs every
  30 minutes, on every plan, and deletes each symbol set whose `last_used` is more than 30 days
  old. Upload sets `last_used`, and so does resolving a crash report's frame with it (at most
  once in 12 hours). Stack traces already resolved stay readable: the cleanup keeps resolved
  frames. The docs don't mention any of this; the source does (§2).
- **So CI doesn't need to delete anything.** A canary build nobody runs any more is gone 30 days
  after its upload, a nightly build that is still installed somewhere keeps its symbols while
  it still crashes (§2, **derived**).
- **CI could delete, with the key it already has**, but not with `posthog-cli`: the CLI uploads,
  downloads and extracts symbol sets, and has no list or delete. The REST API has both
  (`GET` and `DELETE /api/projects/{id}/error_tracking/symbol_sets/…`, and `POST …/bulk_delete/`).
  They need `error_tracking:write`, which the *Source map upload* key that releases already use
  holds (§3). It isn't worth doing.
- **A build makes 3 symbol sets once one binary serves every package**: one for Linux x86_64 and
  one for each macOS target. Windows makes none until PostHog takes PDBs. Today a release makes
  7: Linux 3 (tarball, `.deb`, `.rpm`), macOS 2 per target (tarball, disk image) (§4).
- **Upload each binary once, under one release.** A nightly build that reuses a canary build's
  binary must not upload its symbols again under its own version: PostHog files a symbol set
  under one release for good and refuses the second (`release_id_mismatch`), so the upload step
  would fail the job. Uploading the same binary again under the *same* release costs nothing:
  the CLI asks first and sends only what PostHog lacks (§5).
- **The same 30-day rule applies to stable and rc releases today.** A release that no crash
  report has used for 30 days loses its symbols, and parterre keeps no copy of its Linux or macOS
  debug info to upload again (§6). This is outside this ticket's question but bears on it.
- **Other projects:** Sentry, the obvious comparison, does the same as PostHog with a longer
  clock: debug files expire after 90 days without use (§7). Keeping the volume down is done by
  uploading only builds that ship, once per binary, and leaving expiry to the service.

## 1. Free-plan limits

**The billing catalogue** (`https://billing.posthog.com/api/products-v2?plan=standard`, read
2026-10-16) is what PostHog's pricing page is built from. For error tracking:

| Plan | Allowance | Features listed |
|---|---|---|
| Free (`free-20250326`) | 100,000 exceptions a month | exception autocapture, stack trace demangling, issue recordings, 2 alert destinations |
| Paid (`paid-20250422`) | first 100,000 free, then $0.00037 per exception to 325,000, $0.00014 to 10 million, … | as free, unlimited destinations, triage |

No feature, limit or unit in either plan concerns symbol sets, debug files or their storage.
The *platform and support* product limits the free plan to **1 project** (6 on paid).

**PostHog's error-tracking pricing doc**
([`pricing.mdx`](https://github.com/PostHog/posthog.com/blob/master/contents/docs/error-tracking/pricing.mdx))
agrees: "Error tracking bills you based on the number of `$exception` events **_ingested_** by
PostHog _each month_." Nothing about symbol sets.

**The server enforces no count or storage quota on uploads.** `_validate_uploads` in
[`products/error_tracking/backend/logic/symbol_sets.py`](https://github.com/PostHog/posthog/blob/b344128a899df77ee679b3fcd5c416860a44687a/products/error_tracking/backend/logic/symbol_sets.py)
checks only for duplicate chunk ids, unknown release ids, and size: "Symbol sets larger than
100MB cannot be uploaded". The CLI skips such files itself (`MAX_FILE_SIZE = 100 MB` in
[`cli/src/api/symbol_sets.rs`](https://github.com/PostHog/posthog/blob/b344128a899df77ee679b3fcd5c416860a44687a/cli/src/api/symbol_sets.rs)).
parterre's files are well under it: the v1.0.0 run uploaded all 7 with "0 too large".

**Rate limits** on the symbol set API, per personal API key, from
[`posthog/rate_limit.py`](https://github.com/PostHog/posthog/blob/b344128a899df77ee679b3fcd5c416860a44687a/posthog/rate_limit.py):
`SymbolSetUploadBurstRateThrottle` 1200/minute, `SymbolSetUploadSustainedRateThrottle`
12000/hour. One upload run is a few requests (check, start, the file PUTs to storage, finish).

**Exceptions.** At about four merges a day (127 pull requests merged to `main` in the 30 days
to 2026-10-16), canary and nightly builds run on the maintainer's computers and a few testers'.
Their crash reports are a rounding error against 100,000 a month **(derived)**. How long
PostHog keeps exception events on the free plan is not in the catalogue **(unverified)**.

**One project** means canary and nightly crash reports land in parterre's one project with the
stable ones. The map already plans the ring on every event, which is what tells them apart.

## 2. Retention: PostHog deletes unused symbol sets after 30 days

**The cleanup job.** [`products/error_tracking/backend/temporal/symbol_set_cleanup/`](https://github.com/PostHog/posthog/tree/b344128a899df77ee679b3fcd5c416860a44687a/products/error_tracking/backend/temporal/symbol_set_cleanup)
is a Temporal workflow, scheduled every 30 minutes (`SCHEDULE_INTERVAL = timedelta(minutes=30)`
in `schedule.py`) with the defaults `days_old: int = 30` and `delete_unused: bool = True`
(`types.py`). `activities.py` picks what to delete:

```python
cutoff_date = timezone.now() - datetime.timedelta(days=inputs.days_old)
query_branches = [Q(last_used__isnull=False, last_used__lt=cutoff_date)]
if inputs.delete_unused:
    query_branches.append(Q(last_used__isnull=True, created_at__lt=cutoff_date))
```

The query has no team or plan filter: it applies to every project, free or paid **(derived)**.
It was added in June 2026 (PostHog#61831, #61861) and has run every 30 minutes since July
(#71848).

**What counts as use.**

- *Upload.* `bulk_create_symbol_sets` and `bulk_check_symbol_sets` call `refresh_last_used`,
  whose docstring reads: "Retention deletes a symbol set once `last_used` falls outside its
  window. … Without this call the row ages out while the chunk is still deployed". Uploading
  the same file again refreshes it without sending it, at most once per 12 hours
  (`LAST_USED_UPLOAD_REFRESH_INTERVAL`).
- *Symbolication.* cymbal, which resolves a crash report's frames, sets `last_used` when it
  reads a symbol set, throttled to once per 12 hours (`LAST_USED_THROTTLE_HOURS` in
  [`rust/cymbal/src/core/symbolication/symbol_store/saving.rs`](https://github.com/PostHog/posthog/blob/b344128a899df77ee679b3fcd5c416860a44687a/rust/cymbal/src/core/symbolication/symbol_store/saving.rs)).

So a symbol set lives until 30 days after the last crash report that used it, or 30 days after
its upload if none did.

**Resolved stack traces survive.** `ErrorTrackingSymbolSet.delete()` in
[`products/error_tracking/backend/models.py`](https://github.com/PostHog/posthog/blob/b344128a899df77ee679b3fcd5c416860a44687a/products/error_tracking/backend/models.py)
deletes only unresolved frames: "We always keep resolved frames, as they're used in the UI".
Crash reports that were resolved before the cleanup keep their functions and lines.

**The docs don't say any of this.** Neither the Rust upload guide
([posthog.com/docs/error-tracking/upload-source-maps/rust](https://posthog.com/docs/error-tracking/upload-source-maps/rust))
nor the stack trace, troubleshooting or pricing pages in
[`contents/docs/error-tracking`](https://github.com/PostHog/posthog.com/tree/master/contents/docs/error-tracking)
mentions retention of symbol sets. It is source behaviour, not a documented promise, and the
window could change.

**For canary and nightly builds** **(derived)**: GitHub keeps only the newest canary build and
the last 7 nightly builds (#390). PostHog's rule fits that without help: a build that is
replaced and no longer runs stops producing crash reports, and its symbols go 30 days later; a
build still installed and still crashing keeps them.

## 3. Deleting from CI

**`posthog-cli` can't.** In 0.18.9, the version `release.yml` pins, and in 0.18.10 (the
newest, 2026-10-07), `posthog-cli symbol-sets` has three subcommands: `upload`, `download` and
`extract` (`SymbolSetsSubcommand` in
[`cli/src/commands.rs`](https://github.com/PostHog/posthog/blob/b344128a899df77ee679b3fcd5c416860a44687a/cli/src/commands.rs)).
No list, no delete. PostHog's MCP tool catalogue
([`products/error_tracking/mcp/tools.yaml`](https://github.com/PostHog/posthog/blob/b344128a899df77ee679b3fcd5c416860a44687a/products/error_tracking/mcp/tools.yaml)),
which `posthog-cli api` exposes, has list, retrieve and download for symbol sets, and no delete.

**The REST API can.** `ErrorTrackingSymbolSetViewSet` in
[`products/error_tracking/backend/presentation/views/symbol_sets.py`](https://github.com/PostHog/posthog/blob/b344128a899df77ee679b3fcd5c416860a44687a/products/error_tracking/backend/presentation/views/symbol_sets.py),
registered at `error_tracking/symbol_sets` under projects
([`routes.py`](https://github.com/PostHog/posthog/blob/b344128a899df77ee679b3fcd5c416860a44687a/products/error_tracking/backend/routes.py)):

| Request | What |
|---|---|
| `GET /api/projects/{project_id}/error_tracking/symbol_sets/?search=…&order_by=…` | List, 100 at most per page. `search` matches "reference, release version, release project, and release commit SHA"; `order_by` takes `created_at`, `ref`, `last_used`, each also descending. |
| `DELETE /api/projects/{project_id}/error_tracking/symbol_sets/{id}/` | Delete one: its row, its unresolved frames, then its stored file. |
| `POST /api/projects/{project_id}/error_tracking/symbol_sets/bulk_delete/` with `{"ids": [...]}` | Delete many. This is a plain queryset delete, which skips the model's `delete()`, so whether it removes the stored files is not clear **(unverified)**. |

On `eu.posthog.com` for parterre. List needs `error_tracking:read`, delete
`error_tracking:write` (`scope_object_read_actions` / `scope_object_write_actions`). The key in
`POSTHOG_CLI_API_KEY` has the *Source map upload* preset, `['organization:read',
'error_tracking:write']`
([`frontend/src/lib/scopes.tsx`](https://github.com/PostHog/posthog/blob/b344128a899df77ee679b3fcd5c416860a44687a/frontend/src/lib/scopes.tsx)),
and "A `:write` scope covers the matching `:read`" (`scopes_not_covered` in
[`posthog/scopes.py`](https://github.com/PostHog/posthog/blob/b344128a899df77ee679b3fcd5c416860a44687a/posthog/scopes.py)).
So the existing secret could prune with `curl`.

**Not worth doing** **(derived)**: nothing is billed or capped by symbol set (§1), PostHog
already deletes the unused ones (§2), and deleting the symbols of a canary build that one of
the maintainer's computers still runs would leave its next crash unreadable.

## 4. Symbol sets per build

**Today**, from the log of the v1.0.0 release run (actions run 38164838727, step *Upload symbols
to PostHog*):

| Job | Files | Log |
|---|---|---|
| Linux x86_64 | 3 (`deb`, `rpm`, `tarball`) | "Found 3 native debug file(s)", "Server needs 3 of 3 chunk(s)" |
| macOS aarch64 | 2 dSYMs (`tarball`, `dmg`) | "Found 0 native debug file(s) and 2 dSYM bundle(s)" |
| macOS x86_64 | 2 dSYMs (`tarball`, `dmg`) | as above |
| Windows | 0 (PDBs are a release asset) | — |

7 symbol sets per release, each upload taking 3 to 10 seconds of the job, because each package
holds a build of its own with its own build ID or `LC_UUID` (`docs/releasing.md` › Crash
report symbols).

**With one binary per platform** (#390): **3 symbol sets per build**, one per target that
PostHog symbolicates: Linux x86_64, macOS aarch64, macOS x86_64 **(derived)**. Packages that
wrap the same binary share its build ID, and the CLI keeps one upload per debug id
(`dedup_uploads_by_chunk_id` in `cli/src/api/symbol_sets.rs`; "Validated files, deduplicated by
debug id" in `cli/src/debug_symbols/mod.rs`). A universal macOS binary would not change the
count: "Universal (fat) binaries upload one symbol set per architecture slice" (posthog-cli
changelog, commit 845de8f80c). Windows stays at 0: "Windows binaries (PDB debug info) are not
supported yet" (the Rust guide).

At about four canary builds a day that is some 12 symbol sets a day, and with the 30-day
cleanup a steady state of a few hundred **(derived)**, against no limit.

## 5. Uploading the same binary twice

The map wants builds reused rather than repeated, so a nightly build may well be the canary
build of the same commit. Two server rules decide what a second upload of its symbols does
(`products/error_tracking/backend/logic/symbol_sets.py`):

- **Same content, same release: a no-op.** `bulk_check_symbol_sets` reports which chunks the
  server still needs; one "already uploaded with identical content" is not sent again, only
  marked as still in use. The CLI has asked first since 0.18.2 ("Ask the server which chunks it
  still needs before starting upload batches", changelog commit 342f1f1c55), and 0.18.9 logs it
  as "Server needs N of M chunk(s) (K already present)".
- **Same content, another release: an error.** `_binds_release`: "An orphan symbol set may join
  a release, but a bound one never moves between releases", raising
  `ValidationError(code="release_id_mismatch", detail="Symbol set … already has a release ID")`.
  `release.yml` passes `--release-name parterre --release-version "$VERSION"`, so a reused
  binary uploaded again under a nightly version would fail the step, and with it the job.

So **(derived)**: a build's symbols are uploaded by the job that builds the binary, under the
version that binary carries, and a later workflow that publishes the same binary uploads
nothing (or the same files under the same version, harmlessly). Which version a reused binary
carries is the version ticket's to decide.

A rebuild of the same commit gets a new build ID unless the build is reproducible, so it means
new symbol sets; `docs/releasing.md` already notes that "A rebuild has another build ID"
**(unverified for reproducibility)**.

## 6. The same clock runs for stable releases

Not asked, but it falls out of §2 **(derived)**: the symbols of a stable or rc release are
deleted 30 days after the last crash report that used them, or 30 days after upload if none
did. A user who keeps running v1.0.0 and first crashes on day 40 sends a report PostHog can't
resolve. parterre can't upload again: only the Windows PDBs are kept, as a release asset; the
Linux `.debug` files and the dSYMs exist only in the build job.

Two ways out, for a ticket of its own:

- Publish the Linux and macOS debug info as release assets too, as the PDBs are, and have the
  nightly workflow run `posthog-cli symbol-sets upload` on the current stable and rc releases'
  symbols. While they exist that is only a check that refreshes `last_used`; once deleted it
  uploads them again.
- Accept it: PostHog keeps frames that were already resolved, and for a release that nobody's
  crash report has needed for 30 days the loss is small.

Whether PostHog resolves an exception's frames again after its symbols are uploaded late is not
established **(unverified)**.

## 7. How others keep the volume down

**Sentry**, whose debug-file model PostHog's follows, expires them the same way with a longer
clock. Its *Debug Information Files* page, Retention Policy
([docs.sentry.io](https://docs.sentry.io/platforms/apple/guides/tvos/data-management/debug-files/)):
"Debug Files have a retention period of 90 days, using a time to idle expiration mechanism. …
Once a debug file has not been used to process incoming events for at least 90 days, it will
automatically expire and be eligible for deletion." Its upload page advises uploading "before
deploying or releasing your application so that crash reports can be processed"
([upload](https://docs.sentry.io/platforms/native/data-management/debug-files/upload/)).

What both services' designs and the PostHog CLI's own behaviour point to **(derived)**:

- **Upload only builds that reach someone.** Pull-request and `main` CI builds that nobody
  installs need no symbols; canary and nightly builds that are published do.
- **One upload per binary.** Content-addressed symbol sets (by build ID, `LC_UUID`) make a
  repeated upload a check, not a transfer; one binary per platform cuts the files per build
  from 7 to 3.
- **Leave expiry to the service.** Both delete what goes unused; neither asks customers to prune.
- **Keep the debug info small.** parterre already does: `debug = "line-tables-only"`.

## Sources

- PostHog billing catalogue, `https://billing.posthog.com/api/products-v2?plan=standard`, read 2026-10-16.
- PostHog docs: [Rust symbol upload](https://posthog.com/docs/error-tracking/upload-source-maps/rust);
  [`pricing.mdx`](https://github.com/PostHog/posthog.com/blob/master/contents/docs/error-tracking/pricing.mdx),
  `rate-limiting.mdx`, `stack-traces.mdx`, `troubleshooting.mdx`, `docs/cli.mdx` in `PostHog/posthog.com`.
- PostHog source at [`b344128a89`](https://github.com/PostHog/posthog/tree/b344128a899df77ee679b3fcd5c416860a44687a):
  `products/error_tracking/backend/{logic,presentation/views}/symbol_sets.py`, `models.py`,
  `routes.py`, `temporal/symbol_set_cleanup/`, `mcp/tools.yaml`; `posthog/rate_limit.py`,
  `posthog/scopes.py`, `posthog/permissions.py`; `frontend/src/lib/scopes.tsx`;
  `rust/cymbal/src/core/symbolication/symbol_store/saving.rs`; `cli/src/` and `cli/CHANGELOG.md`.
- parterre: `docs/releasing.md` › Crash report symbols, `.github/workflows/release.yml`, the log
  of release run 38164838727 (v1.0.0).
- Sentry docs: Debug Information Files, Retention Policy and Upload.
