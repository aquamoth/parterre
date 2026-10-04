# Free-tier backends for counts, events and the update check

Research note for [#222](https://github.com/aquamoth/parterre/issues/222), part of the map
[#175](https://github.com/aquamoth/parterre/issues/175): *where can parterre's counts, events and
update checks go on free tiers only, behind a facade the app owns so that the backend can be
swapped later?*

All pages were read on 2026-10-04. Sources are vendor docs, pricing pages, terms and source code.
A statement that is my own conclusion is marked **(derived)**. A statement I could not check is
marked **(unverified)**. A statement checked by running a command is marked **(tested
2026-10-04: `command`)**. This note reports what each backend records; which of it parterre
should send is for the data inventory, not this note.

## TL;DR

- **Hosted, free and EU:** Aptabase (20k events/month, EU region, CSV export, self-hostable),
  PostHog EU (1M events/month, Frankfurt, self-serve DPA on the free plan, 1-year retention) and
  TelemetryDeck (50k/month, Germany, 3 months queryable, no API on free) all take a plain JSON
  POST that parterre's existing `ureq` can send. Every official or community Rust client found
  pulls in `reqwest` (and usually `tokio`), a second HTTP stack. Amplitude (2M/month) and
  Mixpanel (1M/month) have free plans with EU options, but their details are mostly unverified.
  Umami's free plan is web-shaped (fake hostname, browser User-Agent) and has no read API.
  Plausible and Countly have no free hosted tier.
- **Own endpoint:** a Cloudflare Worker on trustfall.se with D1 (`--jurisdiction=eu`) is free,
  fails with errors instead of bills, and can answer the update check and count it in one request.
  It costs moving trustfall.se's nameservers to Cloudflare, a 100k requests/day ceiling, and
  owning a public API, a schema, abuse handling and GDPR paperwork.
- **Update check:** GitHub's `releases/latest` is 60 requests/hour per IP unauthenticated (a 304
  still costs one), gives the owner no counts, and lets GitHub log the user's IP. A static JSON
  file on GitHub Pages gives no counts either; behind free Cloudflare on trustfall.se it gives
  requests by country. Only an endpoint of our own (or an analytics event sent with the check)
  counts versions and OSes.
- **Free counts:** GitHub asset `download_count` (running totals: 45 binaries so far), crates.io
  per-day downloads (90 days), Snap installed base by version/OS/country, Flathub installs per
  day and country. winget has none; Chocolatey has running totals.
- **Facade:** Rust's `log`, `metrics` and OpenTelemetry, and desktop apps such as Zed, VS Code,
  Rerun, Turborepo and Meilisearch all put telemetry behind an app-owned API whose default is a
  no-op. No existing crate abstracts over analytics backends; each backend crate is a client for
  one vendor. A `parterre-telemetry` crate shaped like `parterre-forge` (domain types and a no-op
  default, one backend behind a feature that enables `ureq`) fits (§6).

## 1. Comparison

Hosted services, plus the own-endpoint option, for parterre's needs. Details and sources are in
§2 and §3; each cell is cited there.

| | Free tier | Data location | Retention (free) | Export (free) | Self-host | Rust client / API | Records about the sender | Version / OS / feature counts |
|---|---|---|---|---|---|---|---|---|
| **Aptabase** | 20k events/month, unlimited apps; paused when over, no fees | EU or US region per account | Up to 5 years (debug events 182 days) | CSV from the dashboard; no public query API | Yes: AGPL, Docker (ASP.NET, Postgres, ClickHouse) | Community `aptabase-rs` (reqwest + tokio); raw POST of a JSON array, ~25 events | Country and region from IP; IP not stored; daily-salted hash of IP+UA as user id | `appVersion`, `osName`, `osVersion` built in; feature as event name or prop |
| **TelemetryDeck** | 50k events/month; app count and overage behaviour contradict each other | Germany (Hetzner; also Azure NL, AWS Frankfurt) | 3 months queryable; cold storage 7–10 years | Not found on free; query API paid only | No | Unofficial `telemetrydeck-wasm` (reqwest + tokio); raw POST | Country from IP; IP never stored; timestamp rounded to the hour; double-hashed install id | Dashboard breakdowns by any payload key; API paid |
| **PostHog** | 1M events/month, 1 project; extra events dropped, never billed | EU Cloud, Frankfurt; self-serve DPA on free | 1 year | CSV from insights; file exports billed per row | Hobby Docker deploy, MIT, 4 vCPU/16 GB, unsupported | Official `posthog-rs` (reqwest, tokio by default); raw POST with project token | IP capture off by default for EU orgs; GeoIP on by default, off per event with `$geoip_disable` | Breakdowns by any property; HogQL SQL over the API |
| **Umami Cloud** | 100k events/month, 1 website; each property counts as an event | US and EU (choice on free unverified) | 6 months | Export yes; API Pro only | Yes: Node + Postgres, light | No crate checked; POST needs hostname, url and a browser-like UA | Country/region/city from IP; monthly-salted session hash | Breakdown by event properties in the UI; OS must be sent as data |
| **Plausible** | None (30-day trial, then $9/month) | EU (unverified) | – | – | Community Edition, AGPL (unverified) | Web events API (unverified) | Daily-salted IP+UA hash (unverified) | – |
| **Countly** | None hosted (from $175/month) | – | – | – | Community Edition, AGPL (unverified) | – | – | – |
| **Mixpanel** | 1M events/month | EU residency (unverified) | Not stated | Not checked | – | No official crate (unverified) | IP for geolocation (unverified) | Not checked |
| **Amplitude** | 2M events/month, 1 year of data | EU data center option | 1 year | Not checked | – | HTTP V2 API (unverified) | IP by default, can be dropped (unverified) | Not checked |
| **Own Cloudflare Worker + D1** | 100k requests/day; D1 100k rows written/day, 5 GB; errors when over | D1 with `--jurisdiction=eu`; Worker runs at the edge | Ours to choose; Time Travel 7 days | `wrangler d1 export` (SQL) | It is ours | Ours: one `ureq` POST or GET | Whatever we keep: IP, country, city, ASN available in `request.cf` | SQL we write |
| **Own Worker + Analytics Engine** | 100k data points/day, 10k queries/day; not billed yet | No EU option (derived: US) | 3 months | SQL API | – | Ours | As above | SQL API; sampled, count with `sum(_sample_interval)` |

## 2. Hosted analytics services

### 2.1 Built for apps: Aptabase

- **Free tier:** "20,000" events/month, "Unlimited Apps", "Export to CSV". Over the limit: "we
  will temporarily disable your analytics until the start of the next month", with "no overage
  fees". <https://aptabase.com/pricing>
- **Retention:** "Analytics data is retained for up to 5 years" (<https://aptabase.com/docs>). The
  server sets a 5-year TTL on release events and `DebugTTL = TimeSpan.FromDays(182)` on debug
  events (`src/Features/Ingestion/Buffer/EventRow.cs` in <https://github.com/aptabase/aptabase>,
  commit de1e026).
- **Location and DPA:** "All analytics data of EU-region accounts is processed and stored
  exclusively within the European Union"; Aptabase acts as processor "under Article 28 GDPR",
  with the DPA incorporated in the terms. <https://aptabase.com/legal/privacy>
- **Export and querying:** CSV from the dashboard; "Aptabase has no public query API; the MCP
  server wraps the dashboard endpoints" (<https://aptabase.com/docs>,
  <https://github.com/aptabase/aptabase-mcp>). Custom property values "must be strings or
  numbers" (<https://aptabase.com/llms-full.txt>).
- **Self-hosting:** server AGPL-3.0, SDKs MIT; Docker Compose with Postgres and ClickHouse
  (<https://github.com/aptabase/aptabase>, <https://github.com/aptabase/self-hosting>).
- **Rust:** community `aptabase-rs` 0.2.0 (MIT, 555 downloads) depends on `reqwest`, `tokio`,
  `futures`, `os_info`, `sys-locale` (<https://crates.io/crates/aptabase-rs>). The official
  `tauri-plugin-aptabase` is tied to Tauri (<https://crates.io/crates/tauri-plugin-aptabase>) but
  documents the wire format: the app key `A-EU-…` picks `https://eu.aptabase.com`, and events go
  as a JSON array (max 25) to `/api/v0/events` with an `App-Key` header; each event has
  `timestamp`, `sessionId`, `eventName`, `systemProps` (`osName`, `osVersion`, `locale`,
  `appVersion`, `sdkVersion`, `isDebug`, …) and `props` (`src/config.rs`, `src/client.rs`,
  `src/dispatcher.rs` in <https://github.com/aptabase/tauri-plugin-aptabase>;
  <https://github.com/aptabase/aptabase/wiki/How-to-build-your-own-SDK>). The server rejects
  timestamps older than a day and rate-limits ingestion to 20 requests/s per IP
  (`EventBody.cs`, `Program.cs` in the server repo). A raw `ureq` client is about 30 lines
  **(derived)**.
- **Records about the sender:** "User ID = SHA(Client IP + User Agent + Daily Rotated Salt)", the
  salt "is thrown away every 24 hours", and the SDK collects no "Device ID, Hostname or Hardware
  Identifier" (<https://aptabase.com/legal/privacy>). "Country and region, derived on the server
  from the IP address" (<https://aptabase.com/docs>). The stored row has `countryCode` and
  `regionName` but no IP column (`EventRow.cs`). The hash is SipHash-2-4 in the code, not SHA
  (`DailyUserHasher.cs`).

### 2.2 Built for apps: TelemetryDeck

- **Free tier:** the marketing pricing pages return 404; the dashboard's plan page says "Includes
  50,000 events per month and 3 apps" and "3 months of data retention", but the same bundle also
  says "One product or website to start with", and gives two different overage behaviours
  (<https://dashboard.telemetrydeck.com/plans>). Treat apps and overage as **(unverified)**.
- **Location:** "Azure services are hosted in Amsterdam … AWS Services … in Frankfurt … Hetzner
  Services … in Falkenstein … and Nürnberg"
  (<https://github.com/TelemetryDeck/docs/blob/main/guides/privacy-faq.md>); the company is in
  Germany and the DPA is part of the terms (<https://telemetrydeck.com/dpa/>).
- **Retention:** 3 months queryable on free; cold storage "We expect to delete these events after
  7-10 years" (privacy FAQ above).
- **Export and querying:** "API access — including running queries — is available on
  TelemetryDeck's paid plans" (<https://telemetrydeck.com/docs/api/api-run-query/>). The dashboard
  breaks down by any payload key, such as `TelemetryDeck.AppInfo.version`.
- **Self-hosting:** "We currently don't offer on-premise hosting or self-hosted solutions"
  (<https://github.com/TelemetryDeck/docs/blob/main/articles/hosting-solutions.md>).
- **API:** `POST https://nom.telemetrydeck.com/v2/namespace/{ns}/` with a JSON array of signals;
  required `appID`, `clientUser` ("A hash of the user's ID"), `type`; optional flat `payload`
  (<https://telemetrydeck.com/docs/ingest/v2/>). Unofficial `telemetrydeck-wasm` uses `reqwest`
  and `tokio` (<https://crates.io/crates/telemetrydeck-wasm>).
- **Records about the sender:** "IP addresses are never stored on the TelemetryDeck server";
  timestamps rounded to the hour; an anonymised user id "constant for each app installation"
  (privacy FAQ). Country from the IP (`country.isoCode`, `isInEuropeanUnion`)
  (<https://telemetrydeck.com/docs/ingest/default-parameters/>). The id is hashed on the device and
  again with TelemetryDeck's salt
  (<https://telemetrydeck.com/docs/articles/anonymization-how-it-works/>).

### 2.3 General product analytics: PostHog

- **Free tier:** "Analytics 1M events", "1 project, 1-year data retention", no card; "On the free
  plan, any additional events are permanently dropped" (<https://posthog.com/pricing>,
  <https://posthog.com/docs/data/events-retention>).
- **Location and DPA:** EU Cloud "hosted on servers based in Frankfurt"
  (<https://posthog.com/docs/privacy/gdpr-compliance>); the DPA is self-serve on "Any plan,
  including the free one" (<https://posthog.com/dpa>).
- **Export:** CSV from insights (<https://posthog.com/docs/getting-started/data-import-export>);
  Parquet/JSONL file exports are billed per row
  (<https://posthog.com/docs/cdp/file-download-exports>).
- **Querying:** breakdowns by any event property; HogQL through `POST /api/projects/:id/query/`
  with a personal API key, up to 50k rows (<https://posthog.com/docs/api/queries>). E.g.
  `select properties.app_version, properties.$os, count() from events group by 1, 2`
  **(derived)**.
- **Self-hosting:** "free Docker Compose deployment under an MIT license", needing roughly "4
  vCPU, 16GB RAM", "officially unsupported" (<https://posthog.com/docs/self-host>).
- **Rust:** official `posthog-rs` 0.27.0 (<https://crates.io/crates/posthog-rs>) with a
  non-optional `reqwest` and default features that pull in `tokio`
  (<https://posthog.com/docs/libraries/rust>). Raw API: `POST https://eu.i.posthog.com/i/v0/e/`
  with `{"api_key", "event", "distinct_id", "properties"}`; no hostname or UA needed
  (<https://posthog.com/docs/api/capture>).
- **Records about the sender:** IP capture can be discarded per project, and "EU organizations:
  Automatically default to IP data capture disabled"
  (<https://posthog.com/docs/privacy/data-collection>). "By default, the GeoIP plugin is turned
  on", adding country, city and coordinates
  (<https://posthog.com/docs/product-analytics/person-properties>); per event it is turned off with
  `$geoip_disable`. Anonymous events: `$process_person_profile: false`
  (<https://posthog.com/docs/data/anonymous-vs-identified-events>). Whether GeoIP runs when IP
  capture is off is **(unverified)**.

### 2.4 General product analytics: the rest

- **Umami Cloud Hobby:** "Up to 100K events per month", "1 website", "6 month data retention";
  export yes, API no (<https://umami.is/pricing>). `POST https://cloud.umami.is/api/send` requires
  `hostname`, `website`, `name`, `url` and "a proper `User-Agent` HTTP header"
  (<https://docs.umami.is/docs/api/sending-stats>); bot UAs are dropped unless self-hosted with
  `DISABLE_BOT_CHECK` (`src/app/api/send/route.ts` in
  <https://github.com/umami-software/umami>). Session id hashes IP and UA with a salt rotated
  monthly by default (same file). Self-hosting needs Node 18.18+ and PostgreSQL
  (<https://docs.umami.is/docs/install>).
- **Plausible:** no free hosted plan: "30-day free trial", Starter "$9 /month"
  (<https://plausible.io/#pricing>).
- **Countly:** "Flex … Starts at $ 175 / month" (<https://countly.com/pricing>); Community Edition
  self-hosted **(unverified)**.
- **Mixpanel:** "Up to 1M events / month" free (<https://mixpanel.com/pricing/>); EU residency,
  IP handling and API **(unverified)**.
- **Amplitude Starter:** "2M Events/month forever", "1 year of data access", "EU data center
  option" (<https://amplitude.com/pricing>); API and IP handling **(unverified)**.
- **Not researched:** Pirsch, Swetrix, OpenPanel.

## 3. Our own endpoint on a free tier

### 3.1 Cloudflare Workers, D1, Analytics Engine

- **Workers Free:** "a daily request limit of 100,000 requests, resetting at midnight UTC"; over
  it, Error 1027, with the route set to fail open or fail closed
  (<https://developers.cloudflare.com/workers/platform/limits/>). 10 ms CPU per request.
- **Hostname:** a Custom Domain needs "An active Cloudflare zone"
  (<https://developers.cloudflare.com/workers/configuration/routing/custom-domains/>), and "A
  CNAME setup (partial) is only available to customers on a Business or Enterprise plan"
  (<https://developers.cloudflare.com/dns/zone-setups/partial-setup/>), so trustfall.se's
  nameservers would move to Cloudflare **(derived)**. `workers.dev` is "intended for personal or
  hobby projects" (<https://developers.cloudflare.com/workers/configuration/routing/workers-dev/>);
  a URL baked into shipped binaries should be on trustfall.se so it can move **(derived)**.
- **What the Worker sees:** `request.cf` has `country`, `city`, `region`, `postalCode`,
  `latitude`, `longitude`, `asn`, `asOrganization`, … 
  (<https://developers.cloudflare.com/workers/runtime-apis/request/>), and `CF-Connecting-IP`
  the client IP (<https://developers.cloudflare.com/fundamentals/reference/http-headers/>). What is
  stored is up to us. Workers Logs are on by default, 3 days on free
  (<https://developers.cloudflare.com/workers/observability/logs/workers-logs/>,
  <https://developers.cloudflare.com/workers/platform/pricing/>); whether they hold the IP is
  **(unverified)**.
- **D1 Free:** 5M rows read and 100k rows written per day, 5 GB; over it, "D1 API will return
  errors" until 00:00 UTC (<https://developers.cloudflare.com/d1/platform/pricing/>). EU:
  `wrangler d1 create <db> --jurisdiction=eu`, set at creation
  (<https://developers.cloudflare.com/d1/configuration/data-location/>), though the Data
  Localization compatibility table still says D1 jurisdictions are "not supported today"
  (<https://developers.cloudflare.com/data-localization/compatibility/>); free-plan availability
  **(unverified)**. Export: `wrangler d1 export` as SQL
  (<https://developers.cloudflare.com/d1/best-practices/import-export-data/>). Time Travel 7 days
  on free (<https://developers.cloudflare.com/d1/reference/time-travel/>). Upserting daily
  counter rows instead of one row per ping keeps it small **(derived)**.
- **Workers Analytics Engine:** 100k data points written and 10k queries per day on Free, "you
  will not be billed … once Cloudflare starts billing for usage in the coming months"
  (<https://developers.cloudflare.com/analytics/analytics-engine/pricing/>); data "is stored for
  three months", 20 blobs + 20 doubles per point
  (<https://developers.cloudflare.com/analytics/analytics-engine/limits/>); SQL API
  (<https://developers.cloudflare.com/analytics/analytics-engine/sql-api/>); sampled at write and
  query time (<https://developers.cloudflare.com/analytics/analytics-engine/sampling/>); "not
  available outside US region" under the metadata boundary
  (<https://developers.cloudflare.com/data-localization/compatibility/>).

### 3.2 Other free platforms

- **Deno Deploy:** Classic shut down July 20, 2026 (<https://docs.deno.com/deploy/migration_guide/>);
  the new free plan has 1M requests/month and KV (<https://deno.com/deploy/pricing>), but KV
  writes go to "Northern Virginia (us-east4)"
  (<https://docs.deno.com/deploy/reference/deno_kv/>).
- **Vercel Hobby:** 1M invocations/month (<https://vercel.com/docs/plans/hobby>), but "only …
  for your personal or non-commercial use" (<https://vercel.com/legal/terms>), and commercial
  includes "a paid employee or consultant writing the code"
  (<https://vercel.com/docs/limits/fair-use-guidelines#commercial-usage>). A Trustfall AB endpoint
  plausibly counts as commercial **(derived)**.
- **Supabase Free:** 500 MB DB, 500k Edge Function calls/month, "paused after 1 week of
  inactivity" (<https://supabase.com/pricing>), no backups
  (<https://supabase.com/docs/guides/platform/backups>); regions include Stockholm
  (<https://supabase.com/docs/guides/platform/regions>), free choice **(unverified)**.
- **Netlify:** 300 credits, projects "paused" when out
  (<https://docs.netlify.com/manage/accounts-and-billing/billing/billing-for-credit-based-plans/how-credits-work/>).
  **Fly.io:** "New organizations don't have a free tier" (<https://docs.fly.io/about/pricing>).

### 3.3 What we would maintain (derived unless cited)

- The response format is a public API every shipped binary depends on; the app must treat 1027s
  and timeouts as "no update info".
- Release metadata updated on each release (from CI, or the Worker reading GitHub Releases).
- Schema, aggregation, retention and export (D1 Time Travel is only 7 days).
- Junk and spoofed pings: anything in the binary can be extracted, so validate against
  allow-lists. Free Cloudflare has one rate-limiting rule, keyed on IP
  (<https://developers.cloudflare.com/waf/rate-limiting-rules/>); a flood can use up the 100k/day
  and take the update check down until midnight UTC.
- Dashboards: SQL plus an API token.
- GDPR: Trustfall AB is controller, Cloudflare processor; notice, Art. 30 record, transfer
  analysis, retention periods.
- Vendor drift: WAE billing, Deno's shutdown, Supabase pausing.

## 4. Update-check sources

| Source | Limits | What the provider learns | What we can count |
|---|---|---|---|
| GitHub REST `GET /repos/aquamoth/parterre/releases/latest` | 60 requests/hour per IP unauthenticated ([rate limits](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api)); a 304 counts unless authorised ([best practices](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api); tested 2026-10-04: `curl -H "If-None-Match: $etag" …/releases/latest`, remaining went 13 to 12); ~14 KB, 1.8 KB gzipped (tested 2026-10-04); "latest" skips prereleases ([releases](https://docs.github.com/en/rest/releases/releases)) | "IP address, device information … operating system and application version" ([privacy statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement)) | Nothing: repository traffic shows clones, visitors, referrers, not API calls ([traffic](https://docs.github.com/en/repositories/viewing-activity-and-data-for-your-repository/viewing-traffic-to-a-repository)) |
| `https://github.com/aquamoth/parterre/releases/latest` redirect | Undocumented; `302` with the tag in `location`, 0 bytes (tested 2026-10-04: `curl -sSI …`) | As above | Nothing |
| Static JSON on GitHub Pages | 1 GB, soft 100 GB/month, 429 when excessive; not for "e-commerce" or SaaS ([limits](https://docs.github.com/en/pages/getting-started-with-github-pages/github-pages-limits)) | "the visitor's IP address is logged and stored" ([about Pages](https://docs.github.com/en/pages/getting-started-with-github-pages/about-github-pages)) | Nothing |
| Static JSON on trustfall.se behind free Cloudflare | As the plan | Cloudflare sees the request | Requests, bandwidth, unique visitors, country, 30+ days ([zone analytics](https://developers.cloudflare.com/analytics/account-and-zone-analytics/zone-analytics/), [GraphQL limits](https://developers.cloudflare.com/analytics/graphql-api/limits/)); per-path on free **(unverified)**; Web Analytics is a JS beacon and misses it **(derived)** |
| Our endpoint answers it (§3) | 100k/day | Whatever we store | Version, OS, arch, channel in the request path or query |

Comparable apps check against their own endpoint, sending channel, version, OS and arch:

- **Zed:** `/releases/{channel}/{version}/asset?asset=zed&os=…&arch=…`; ids only with telemetry on
  (`crates/auto_update/src/auto_update.rs` in <https://github.com/zed-industries/zed>).
- **VS Code:** `{updateUrl}/api/update/{platform}/{quality}/{commit}`
  (`src/vs/platform/update/electron-main/abstractUpdateService.ts` in
  <https://github.com/microsoft/vscode>); off with `update.mode: none`
  (<https://code.visualstudio.com/docs/supporting/faq>).
- **GitHub Desktop:** `/desktop/desktop/{x64|arm64}/latest` on its update host
  (`app/src/ui/lib/update-store.ts` in <https://github.com/desktop/desktop>).
- **Syncthing:** `https://upgrades.syncthing.net/meta.json` every 12 h; usage reporting is a
  separate opt-in (`lib/config/optionsconfiguration.go` in
  <https://github.com/syncthing/syncthing>; <https://docs.syncthing.net/users/security.html>).
- **Homebrew:** opt-out analytics of package, OS, arch and version, "does not contain a user
  identifier or an IP-address field", 365 days, public aggregates (<https://docs.brew.sh/Analytics>).

## 5. Counts the channels give for free

| Channel | What it gives | Source |
|---|---|---|
| GitHub Releases | `download_count` per asset, a running total; no time, OS or country; bots and CI included **(derived)**. Today: 45 binary downloads, v0.5.1's MSI 24 of them, macOS 0, rpm 0 (tested 2026-10-04: `gh api repos/aquamoth/parterre/releases`) | <https://docs.github.com/en/rest/releases/assets> |
| crates.io | Per-day downloads for 90 days, latest 5 versions; 1 request/s and an identifying UA; daily DB dumps. Today: 10 (0.5.0 only) (tested 2026-10-04: `curl https://crates.io/api/v1/crates/parterre/downloads`) | `src/controllers/krate/downloads.rs` in <https://github.com/rust-lang/crates.io>; <https://crates.io/data-access> |
| winget | None for publishers: "We only sample the data"; Partner Center integration still pending (2026-05-08). winget installs fetch the GitHub MSI, so they land in its `download_count` **(derived)** | <https://github.com/microsoft/winget-pkgs/discussions/39736> |
| Chocolatey | `DownloadCount` and `VersionDownloadCount`, running totals; what increments them **(unverified)** | <https://community.chocolatey.org/api/v2/FindPackagesById()?id='git'> |
| Snap Store | Daily installed base by channel, country, OS, version, architecture, and device churn; `snapcraft metrics`, up to 5 years | <https://ubuntu.com/docs/snapcraft/9/reference/metrics/>, <https://ubuntu.com/docs/snapcraft/9/how-to/publishing/get-snap-metrics/> |
| Flathub | `installs_total`, `installs_per_day` (~180 days), `installs_per_country`, last month / 7 days; counted from downloads of artifacts, installs and updates | <https://flathub.org/api/v2/stats/org.mozilla.firefox>, <https://docs.flathub.org/blog/over-one-million-active-users-and-growing> |
| .deb/.rpm | Only the GitHub asset totals, unless we host a repository **(derived)** | – |

## 6. The facade

### 6.1 How others do it

- **The Rust facade crates.** `log`: "Libraries should link only to the `log` crate", and with no
  implementation "the facade falls back to a 'noop' implementation … just an integer load,
  comparison and jump" (<https://docs.rs/log/latest/log/>). `metrics` is the same shape: without a
  recorder "a 'noop' recorder lives in its place", and executables call `set_global_recorder`
  once (<https://docs.rs/metrics/latest/metrics/>). OpenTelemetry splits API from SDK so that
  without an SDK "The application will still build and run without failing, although no
  telemetry data will be actually delivered"
  (<https://opentelemetry.io/docs/specs/otel/library-guidelines/>). `sentry-core` without its
  `client` feature "will blackhole a lot of operations"
  (<https://docs.rs/sentry-core/latest/sentry_core/>).
- **Zed** (Rust, desktop): a small `telemetry` crate with an `event!` macro; events go to
  `static TELEMETRY_QUEUE: OnceLock<mpsc::UnboundedSender<Event>>`, which the `client` crate fills
  with `init(tx)`; if nothing did, `send_event` does nothing
  (`crates/telemetry/src/telemetry.rs` in <https://github.com/zed-industries/zed>). The `client`
  crate batches (50 events or 5 minutes in release) and posts to its own `/telemetry/events`
  with `os_name`, `os_version`, `app_version`, `architecture`, `release_channel`, and returns
  early unless `settings.metrics` is on (`crates/client/src/telemetry.rs`).
- **Rerun** (Rust, egui): `re_analytics` crate, own `Event`/`Properties` traits,
  `Analytics::record(event)`; native and web sinks chosen by target; the native `PostHogSink`
  posts to `https://tel.rerun.io` with `ehttp`. Off with `rerun analytics disable`, in CI, and in
  tests and debug builds (`disabled_reason()`); a `testing` feature disables it
  (`crates/utils/re_analytics/src/lib.rs`, `src/native/sink.rs`, `Cargo.toml` in
  <https://github.com/rerun-io/rerun>).
- **Turborepo** (Rust CLI): `turborepo-telemetry` with a global `telem(event)` "Safe to call from
  anywhere", a `TELEMETRY_STATE: OnceLock`, no-op when disabled or uninitialised, and
  `init(config, client: impl TelemetryClient, …)` taking the HTTP client as a trait; a background
  worker flushes every 10 events or 1 s; `TURBO_TELEMETRY_DISABLED` and `DO_NOT_TRACK` turn it off
  (`crates/turborepo-telemetry/src/lib.rs` and `README.md` in
  <https://github.com/vercel/turborepo>).
- **Meilisearch** (Rust server): `Analytics { segment: Option<SegmentAnalytics> }`, `None` when
  `no_analytics` is set, events as an `Aggregate` trait
  (`crates/meilisearch/src/analytics/mod.rs` in <https://github.com/meilisearch/meilisearch>).
- **VS Code** (desktop): `ITelemetryService` with `NullTelemetryService`, and
  `ITelemetryAppender { log, flush }` per backend with a `NullAppender`
  (`src/vs/platform/telemetry/common/telemetryUtils.ts` in <https://github.com/microsoft/vscode>).

The common shape **(derived)**: the app calls its own API with its own event types; the default
is a no-op (unset global, `None`, null implementation, or feature off); one module or crate knows
the wire format and the URL; sending is batched on a background thread and failures are dropped;
the opt-out is checked in one place.

### 6.2 Existing crates

No crate found abstracts over several analytics backends the way `log` does over loggers. The
crates are each one vendor's client: `posthog-rs`, `aptabase-rs`, `telemetrydeck-wasm` (§2),
`segment` (used by Meilisearch, <https://crates.io/crates/segment>) and `rudderanalytics`
(<https://crates.io/crates/rudderanalytics>). The general facades (`metrics`, OpenTelemetry) are
for counters and traces sent to Prometheus or an OTLP collector, not product events, and none of
the hosted backends in §2 is fed by them **(derived)**. The vendor clients found use `reqwest`
(§2), while parterre already has `ureq` with rustls in the workspace (`Cargo.toml`), and each
backend's wire format is one JSON POST **(derived)**.

### 6.3 A sketch for parterre (derived)

Following `parterre-forge` (domain types in `lib.rs`, the GitHub client in `github.rs` behind the
`github` feature, which enables `dep:ureq`) and `parterre-highlight` (without `syntax`, nothing is
coloured):

```toml
# crates/parterre-telemetry/Cargo.toml
[features]
# The backend (#222): the HTTPS client and the wire format. Without it, nothing is sent and the
# update check finds nothing.
send = ["dep:ureq"]

[dependencies]
parterre-util.workspace = true
serde.workspace = true
serde_json.workspace = true
ureq = { workspace = true, optional = true }
```

```rust
// crates/parterre-telemetry/src/lib.rs: what the app sees, and nothing about the backend.
pub struct Context { pub version: &'static str, pub channel: Channel, pub os: String, pub arch: &'static str }
pub enum Event { Started, /* the list comes from the data inventory */ }
pub struct Update { pub version: String, pub url: String }

#[derive(Clone)]
pub struct Telemetry(/* None, or a channel to the sender thread */);

impl Telemetry {
    /// A no-op: what tests, `--screenshot` and opted-out users get.
    pub fn off() -> Telemetry;
    /// Starts the sender thread when the feature is on; otherwise `off()`.
    pub fn start(context: Context, consent: Consent) -> Telemetry;
    /// Never blocks, never fails.
    pub fn record(&self, event: Event);
    /// The update check; on a backend that also counts, this request is the count.
    pub fn latest(&self) -> Option<Update>;
    /// Sends what is queued, waiting at most `timeout`; called on exit.
    pub fn flush(&self, timeout: Duration);
}

#[cfg(feature = "send")]
mod backend; // the one file that knows the URL and the JSON
```

- The app owns `Context`, `Event`, `Update` and the consent; the backend module maps them onto
  one vendor's JSON. Swapping backends replaces `backend.rs` (or the crate) and nothing in
  `crates/parterre`.
- A handle passed to the app, rather than a global, matches how `parterre-forge` is called; a
  `OnceLock` global as in Zed and Turborepo saves threading it through the UI. Either fits.
- The update check and the counting sit in the same crate so that a backend which answers both
  in one request (our Worker) and one that cannot (GitHub plus Aptabase) look the same to the app.
- Crash reports (#175's other half) could be a second method on the same handle or a crate of
  their own; that is for the crash-report research.

## 7. Realistic shortlist for parterre (derived)

1. **Aptabase Cloud, EU region**, raw `ureq` POST: built for desktop apps, version and OS
   built in, IP not stored, 5-year retention, CSV export, AGPL server to self-host if it is ever
   left. Limit: 20k events/month, paused when over, so a few events per user per day at most.
   Update check from another source (GitHub API, or a static file).
2. **PostHog Cloud EU**, raw `ureq` POST with `$geoip_disable` and anonymous events where
   wanted: 1M events/month, SQL querying, self-serve DPA, dropped (not billed) over the limit.
   Retention 1 year on free. Update check from another source.
3. **Own Cloudflare Worker on trustfall.se with D1 in the EU jurisdiction**: the update check and
   the count are one request, and we decide what is stored and for how long. Costs DNS on
   Cloudflare, a 100k/day ceiling, and maintenance (§3.3). Can be added later behind the same
   facade, keeping a hosted service for feature events.

Whichever is chosen, the free channel counts (§5) come on top, and the static-file or GitHub
update check needs no backend at all.

## Not checked

- Pirsch, Swetrix, OpenPanel.
- Mixpanel's and Amplitude's IP handling, EU residency details, APIs and export.
- Umami Hobby's EU choice and over-limit behaviour; Plausible and Countly beyond pricing.
- Whether D1's EU jurisdiction and Supabase's region choice are on the free plans; whether
  Cloudflare Free zone analytics filter by path; what Workers Logs store.
- Chocolatey's download definition; the Snap dashboard.
