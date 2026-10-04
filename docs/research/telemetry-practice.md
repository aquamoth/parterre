# Telemetry in comparable open-source tools

Research note for [#219](https://github.com/aquamoth/parterre/issues/219), part of the map
[#175](https://github.com/aquamoth/parterre/issues/175): *what do comparable open-source tools
collect, what is on by default and what is opt-in, how do they tell the user, and where did users
push back?*

Researched 2026-10-04. Sources are official docs, source code at pinned commits, the projects' own
issue trackers and mailing lists. Press articles are used only where the primary source would not
load, and are marked as such. A statement that is my own conclusion is marked **(derived)**. A
statement I could not check is marked **(unverified)**. This note describes practice; it does not
recommend anything.

Pinned sources used repeatedly (short names in the text):

| Short name | Source |
|---|---|
| T3 | [pingdotgg/t3code@b3b6ae2](https://github.com/pingdotgg/t3code/tree/b3b6ae2cebbae39a706038911dbb8c85c7120be7) |
| VSC | [microsoft/vscode@5e86c7c](https://github.com/microsoft/vscode/tree/5e86c7c4c2eb99b22e631e0aa4eb05f6b8b53f35) |
| VSCod | [VSCodium/vscodium@5a73682](https://github.com/VSCodium/vscodium/tree/5a73682ca091082675b10c9dc3f348c1d824d94f) |
| Z | [zed-industries/zed@a846890](https://github.com/zed-industries/zed/tree/a84689073d296dfd39987bc7dd478e43ef76d83a) |
| GD | [desktop/desktop@3754e26](https://github.com/desktop/desktop/tree/3754e26d1f021ccb2de77f86061ea1dab2c95d39) |
| HB | [Homebrew/brew@8b92a1a](https://github.com/Homebrew/brew/tree/8b92a1a6b5716d1719d3a424e6955c725673cbe2) |
| GT | [golang/telemetry@ed294f9](https://github.com/golang/telemetry/tree/ed294f9431572147af3f0aaa3fe932a93b698664) |
| DN | [dotnet/sdk@590b097](https://github.com/dotnet/sdk/tree/590b0970fe66407ef532b7c95335a0ac4d08f1ab) |
| NX | [vercel/next.js@ba80ee4](https://github.com/vercel/next.js/tree/ba80ee48fc319735151c3ad6d9bb9a8180c9f09e) |
| VC | [vercel/vercel@c628be7](https://github.com/vercel/vercel/tree/c628be7835e03a965b93e9cf9e2bd5ac2acbf5eb) |
| DE | [denoland/deno@b4f08f1](https://github.com/denoland/deno/tree/b4f08f127652d8442b4d3dbabc277aca3840bc1d) |
| GB | [gitbutlerapp/gitbutler@12cd332](https://github.com/gitbutlerapp/gitbutler/tree/12cd33291625e7f9e3907c5983a3b943b4a997ba) |
| LG | [jesseduffield/lazygit@ff375b1](https://github.com/jesseduffield/lazygit/tree/ff375b124d149f03f9156d3720a998cbc04482fa) |
| GU | [gitui-org/gitui@2fa693c](https://github.com/gitui-org/gitui/tree/2fa693cb6ed431b21ebc300dd02e83c2476699ce) |
| TG | [tortoisegit@7338078](https://gitlab.com/tortoisegit/tortoisegit/-/tree/7338078f8ddd924b8cddee35f512f2286072136d) |
| ST | [syncthing/syncthing@7ad73b4](https://github.com/syncthing/syncthing/tree/7ad73b408adc792cabeed41d89a37b93e2bd84d0) |
| FF | [mozilla-firefox/firefox@3f73c52](https://github.com/mozilla-firefox/firefox/tree/3f73c528a1ae5784ea5e1ee2c5ad3762507395f2) |
| KUF | [KDE/kuserfeedback@57023cf](https://github.com/KDE/kuserfeedback/tree/57023cf004072b055dde09bb7d49f79a7aac7966) |
| PW | [KDE/plasma-workspace@a3302a6](https://github.com/KDE/plasma-workspace/tree/a3302a698ad247fceadad224a0ecd0b6a74f2516) |

## TL;DR

- **Update checks are default-on everywhere that has one**, and nobody found here has had lasting
  pushback over one that sends only version, OS and architecture with no ID. Deno's one complaint
  was answered with an env var; Audacity's 2021 reset kept its update check while dropping
  analytics.
- **Default-on usage analytics with a random install ID is common among developer tools** (VS Code,
  .NET, Next.js, Vercel, Zed, GitHub Desktop, GitButler, Homebrew until 2023, t3code). Each of
  them has open or closed "make it opt-in" requests, and each kept the default. What they have in
  common is a first-run notice and a one-step off switch.
- **The exceptions that went opt-in or removed collection** were language toolchains (Go 2023,
  rustup 2019, rustc's "no telemetry" goal), desktop/community projects (KDE, Debian popcon,
  Syncthing, Ubuntu after 2025), and Audacity after its 2021 backlash.
- **Backlash reliably follows**: collection users find before they are told (t3code 2026), data
  sent after opting out (VS Code 2016, GitHub Desktop 2017, GitButler 2026), third-party ad or
  analytics SDKs (Homebrew's Google Analytics, Audacity's Google/Yandex), IDs tied to an account
  (GitHub Desktop's user ID, t3code's hashed Claude/Codex IDs), vague legal text (Audacity 2021,
  Firefox 2025), and toggles that do not stick.
- **Accepted notices are short and concrete**: they name what is sent, say what is not, link the
  full list, and give the off switch in the same sentence. Several show the exact payload
  (Syncthing, Ubuntu, VS Code, Next.js, Audacity's crash dialog).

## 1. Comparison table

"n/c" = not covered by this research. Tools are grouped: t3code, developer tools, git GUIs,
desktop apps. Detail and links for each row are in the sections below.

| Tool | Default-on data | Opt-in data | IDs | How users are told | Off switch | Backend | Update check |
|---|---|---|---|---|---|---|---|
| **t3code** | PostHog events: server boot (thread/project counts), client connected (surface, OS, app version, browser), turn requested, thread started, provider turn completed (provider, model, effort, tokens, duration), ChatGPT login events; version, platform, arch, WSL distro | OTLP traces/metrics/logs to the user's own collector | SHA-256 of Codex account ID, else Claude user ID, else random install UUID (unsalted) | None in the app. A docs page since 2026-09-04; privacy policy does not name PostHog | `T3CODE_TELEMETRY_ENABLED=false` env var only; toggle request closed "not planned" | PostHog US cloud (`us.i.posthog.com`), key in source | electron-updater against GitHub Releases, 15 s after start then every 4 min; `T3CODE_DISABLE_AUTO_UPDATE` |
| VS Code | Usage, errors, crashes (`telemetryLevel: all`); version, OS, arch, session | Experiments (separate switch) | `machineId` (SHA-256 of MAC), `sqmId`, shared `devDeviceId`, session | Welcome-page footer; `code --telemetry` dumps events; telemetry output channel | `telemetry.telemetryLevel` (all/error/crash/off), CLI flags, enterprise policy | Microsoft 1DS / App Center (closed) | Yes (n/c) |
| VSCodium | Nothing from the editor; extensions may still send | – | – | README, docs/telemetry.md | Patched off at build | – | Yes, documented as remaining |
| Zed | Diagnostics (crash minidumps) and metrics (features, file extensions, frameworks) | Edit-prediction training data | `system_id`, `installation_id` (random UUIDs), `metrics_id` (per user, may link to email), session | Onboarding switches shown on; docs page; telemetry log view | Two settings; crash upload compiled in only if endpoint set at build | Sentry, Snowflake, Hex, Amplitude; Cloudflare country from IP | Auto-update (n/c) |
| GitHub Desktop | Daily payload: version, OS, arch, theme, editor, shell, repo counts, ~190 counters; fatal crashes regardless of opt-out | Nothing | Random `guid`; separate updater UUID; GitHub user ID 2018–2021 | One line on the welcome screen | Settings > Advanced; one-time ping without guid on change | GitHub internal (closed) | Yes, with its own UUID |
| Homebrew | Install events: package, arch, OS version, prefix kind, CI; since 2026-07 `command_run` without option values | CI test-bot results | None since 2023-02 (UUID removed) | Terminal notice before anything is sent | `brew analytics off`, `HOMEBREW_NO_ANALYTICS=1` | Google Analytics 2016–2023, InfluxDB EU, own proxy since 2026-08; public aggregates | `brew update` (n/c) |
| Go toolchain | Counters written locally only (`local` mode) | Weekly upload of allowlisted counters and crash stack counters | None in uploads; IP not recorded | Release notes, blog; gopls asks 1% of users after 7 days | `go telemetry off\|local\|on` | Open source server; raw data public | – |
| .NET CLI | Command (hashed), OS, runtime ID, SDK version, 3-octet IP, crash type and stack; since 2026-09 a Microsoft-employee check | Builds from source: off unless compile flag | Unsalted SHA-256 of MAC, shared `devdeviceid`, session | First-run banner | `DOTNET_CLI_TELEMETRY_OPTOUT=1` (before install) | Application Insights (closed); aggregates CC-BY | n/c |
| Next.js | Command, version, OS, CPU, CI, plugins, build stats | Error feedback | Random `anonymousId`; salted hash of git remote as `projectId` | One-time console notice | `next telemetry disable`, `NEXT_TELEMETRY_DISABLED=1`, debug mode prints | Vercel (closed) | n/c |
| Vercel CLI | Command, args and flags, version, OS, CI, account identifiers | – | `team_id`, `user_id`, `project_id` unhashed; random `deviceId` | One-time notice | `vercel telemetry disable`, `VERCEL_TELEMETRY_DISABLED=1` | Vercel (closed) | n/c |
| Deno | Nothing but the version check | OpenTelemetry to the user's own collector | None | Env-var docs | `DENO_NO_UPDATE_CHECK`; distros build without `upgrade` | – | GET `release-latest.txt` ≤ daily, UA `Deno/<ver>` |
| Rust (rustup, cargo) | Nothing; rustup's UA carries version and TLS backend | Local metrics files (rustc `-Zmetrics-dir`); annual survey | None | Policy statements | – | Server logs only | rustup self-update (n/c) |
| GitKraken | Usage analytics, directory/file names, crash reports, IP, OS, version (privacy policy) | Not found | Not documented | Privacy policy only | "To the extent available" (unverified) | Unnamed third parties | n/c |
| Fork | Crash reports (AppCenter Crashes, per 2023 statement) | – | n/c | Developer statements on the tracker | Not documented | AppCenter (2023); current unverified | Velopack/Squirrel |
| Sublime Merge | No usage analytics found | – | Binary references `/var/lib/dbus/machine-id` (use unverified) | Forum statements; no privacy policy | `update_check` (licensed builds only) | Crashpad to `crash-server.sublimehq.com` (consent unverified) | `stable_update_check?version&platform&arch` |
| Sourcetree | Repo count and hosting provider if agreed; E-MAU registration analytics always | Usage data via welcome-wizard checkbox | n/c | Welcome wizard | Options checkbox, `AnalyticsHasAgreed`; E-MAU cannot be disabled | Atlassian (closed) | n/c |
| TortoiseGit | Crash dumps to drdump.com (installer feature on by default, official builds) | – | Fixed app GUID only | Installer feature list, support page | Leave the feature out at install | Doctor Dump (crash-server.com) | Weekly `version.txt`, UA has version and Windows edition, no ID |
| gitui | Nothing | – | – | – | – | – | None |
| lazygit | Nothing | – | – | – | – | – | `releases/latest` on github.com; `update.method: never` |
| GitButler | PostHog (command names, durations, errors, settings, hashed repo URL) and Sentry | Account-linked identity removed 2026-05 | PostHog distinct ID; CLI `sha256(machine_uid+"gitbutler")` | Onboarding screen, both toggles on, nothing sent before it | Two settings; `but config metrics disable` | PostHog EU, Sentry | Hourly; CLI sends the machine hash regardless of metrics |
| Syncthing | Nothing until the user answers | Daily report: version, platform, folder/device counts, sizes, feature use | Random `urUniqueID`, made when the user accepts | Modal on first GUI start with a full payload preview; asks again when the report format changes | `urAccepted = -1`, Settings | Own server, GeoLite2 country from IP; public aggregates at data.syncthing.net | Every 12 h, no identifying info |
| Firefox | Technical and interaction data (Glean/Telemetry); daily usage ping; update checks | Automatic crash-report sending; studies | `client_id`, `profile_group_id`; separate `usage.profile_id` for the usage ping | First-run privacy notice tab, Settings | Two separate checkboxes; deletion-request pings on opt-out | Mozilla pipeline (open-source tooling) | Yes |
| Audacity (3.x) | Update check (UA, IP-derived country, IP truncated to 3 octets) | Error reports (per-report dialog); UUID for DAU/retention (since 2025) | Optional random UUID | Privacy notice; error dialog shows the report | Preferences; build flags off for distro builds | Self-hosted Sentry and servers in the EU | At start |
| KDE Plasma (KUserFeedback) | Nothing (`NoTelemetry`) | Basic/detailed system info and usage stats (Qt, GPU, screens, panel count) | None by policy | System Settings slider; one encouragement after 5 starts | Settings | telemetry.kde.org (KDE-run) | Discover (n/c) |
| Debian popcon | Nothing (installer default "No") | Weekly installed-package list with atime | Random 128-bit `MY_HOSTID` | debconf question at install | debconf / config file | Debian; public aggregates without IDs | – |
| Ubuntu (ubuntu-report → Insights) | 2018 plan: checkbox ticked by default; Insights (25.10+): asks again | Hardware, install choices; Insights adds monthly reports | None; designed not to link reports | Initial-setup screen; JSON shown and cached locally | Settings > Privacy; opt-out sends one flag | Canonical (metrics.ubuntu.com); open source server | – |

## 2. t3code in detail

t3code is an Electron desktop app, a web client and a mobile app that all talk to one Node server
(the server also runs headless as `t3 serve`). All product analytics live in that server.

### What is sent

- **Library and endpoint.** A hand-written PostHog batch client posts to
  `${T3CODE_POSTHOG_HOST}/batch/`, default `https://us.i.posthog.com`, with the project key
  hard-coded as the default of `T3CODE_POSTHOG_KEY`. Clients do not load the PostHog browser SDK.
  [T3/apps/server/src/telemetry/AnalyticsService.ts#L69-L76](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/apps/server/src/telemetry/AnalyticsService.ts#L69-L76),
  [#L203](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/apps/server/src/telemetry/AnalyticsService.ts#L203),
  [T3/docs/internals/product-analytics.md](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/docs/internals/product-analytics.md)
- **Default.** `T3CODE_TELEMETRY_ENABLED` defaults to `true`.
  [AnalyticsService.ts#L76](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/apps/server/src/telemetry/AnalyticsService.ts#L76)
- **Every event carries** `platform`, `arch`, `wsl` distro name, `t3CodeVersion`, `clientType`
  (desktop-app or cli-web-client), `serverOs`, `serverArch`, `serverMode`, and
  `$process_person_profile: false` (no PostHog person profiles).
  [AnalyticsService.ts#L180-L199](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/apps/server/src/telemetry/AnalyticsService.ts#L180-L199)
- **Events found in the source:**
  - `server.boot.heartbeat` with `threadCount` and `projectCount`.
    [T3/apps/server/src/serverRuntimeStartup.ts#L171](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/apps/server/src/serverRuntimeStartup.ts#L171)
  - `client.connected`, `client.thread.started`, `client.turn.requested`, with the client's
    surface, app version, OS, browser, web deployment, connection method, and on mobile OS major
    version and device model.
    [T3/apps/server/src/ws.ts#L637-L680](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/apps/server/src/ws.ts#L637-L680),
    [#L3820](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/apps/server/src/ws.ts#L3820)
  - `provider.turn.completed`: provider, terminal status, input/cached/output/reasoning token
    counts, model, reasoning effort, runtime and interaction mode, duration.
    [T3/apps/server/src/orchestration-v2/ProviderEventIngestor.ts#L93-L133](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/apps/server/src/orchestration-v2/ProviderEventIngestor.ts#L93-L133)
  - `chatgpt.<event>.started` / `.completed` around ChatGPT sign-in.
    [T3/apps/server/src/provider/CodexChatGptAuth.ts#L122-L135](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/apps/server/src/provider/CodexChatGptAuth.ts#L122-L135)
- **Not sent, per the user docs:** prompts, responses, file contents, tokens, conversation IDs,
  raw provider events, child-agent output.
  [T3/docs/user/telemetry.md](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/docs/user/telemetry.md)
- **Delivery.** Buffered in memory (max 1,000 events), flushed every second in batches of 20,
  each event with a UUIDv7 so retries are deduplicated; a batch is dropped after 5 failed sends.
  The docs record why: "one stuck batch was sent every second for days".
  [AnalyticsService.ts#L52-L67](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/apps/server/src/telemetry/AnalyticsService.ts#L52-L67),
  [product-analytics.md](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/docs/internals/product-analytics.md)
- **IP.** The server posts directly to PostHog, so PostHog sees the user's IP; what PostHog stores
  depends on project settings that are not public **(unverified)**.

### The identifier

`getTelemetryIdentifier` takes the first of these and sends its SHA-256 (unsalted) as
`distinct_id`:

1. `~/.codex/auth.json` → `tokens.account_id` (ChatGPT logins),
2. `~/.claude.json` → `userID`,
3. `~/.t3/telemetry/anonymous-id`, a random UUIDv4 written on first run.

So the same person gets the same ID on every machine where they use the same Codex or Claude
account. [T3/apps/server/src/telemetry/Identify.ts#L255-L310](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/apps/server/src/telemetry/Identify.ts#L255-L310)

### How users are told, and the off switch

- **In the app:** nothing. There is no first-run notice, consent screen or settings toggle; the
  welcome-wizard and updating docs do not mention telemetry
  ([T3/docs/user/welcome-wizard.md](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/docs/user/welcome-wizard.md)).
- **Docs:** an 11-line `docs/user/telemetry.md`, added 2026-09-04 in the PR that started measuring
  token usage ([#9132](https://github.com/pingdotgg/t3code/pull/9132)). Telemetry itself was added
  2026-03-05 ([ec3778b](https://github.com/pingdotgg/t3code/commit/ec3778bbf4), "Add anonymous
  PostHog telemetry for provider lifecycle events").
- **Privacy policy** (effective 2026-07-14) names Clerk, Cloudflare, PlanetScale, Vercel, Apple
  and Google, not PostHog, and says data is used "to analyze and improve T3 Code using aggregated,
  de-identified, or other anonymous operational information".
  [T3/apps/marketing/src/pages/privacy-policy.astro](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/apps/marketing/src/pages/privacy-policy.astro)
- **Off switch:** set `T3CODE_TELEMETRY_ENABLED=false` in the server's environment before it
  starts. The project's own observability docs note that GUI launches (Finder, Start menu, dock)
  usually do not pick up shell variables, so desktop users have to launch from a shell for an env
  var to apply.
  [T3/docs/operations/observability.md#L231](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/docs/operations/observability.md#L231)

### Other network traffic

- **Update check:** electron-updater against GitHub Releases (`provider: "github"`), 15 s after
  start and then every 4 minutes; `T3CODE_DISABLE_AUTO_UPDATE` turns it off. Preview and PR builds
  ship without a feed.
  [T3/apps/desktop/src/updates/DesktopUpdates.ts#L50-L51](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/apps/desktop/src/updates/DesktopUpdates.ts#L50-L51),
  [T3/scripts/build-desktop-artifact.ts#L2585](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/scripts/build-desktop-artifact.ts#L2585)
- **Crash reporting:** none found. No Sentry or Electron `crashReporter.start` in t3code's own
  code; crashes go to local rotating logs (`server-child.log`).
  [T3/apps/desktop/src/app/DesktopObservability.ts](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/apps/desktop/src/app/DesktopObservability.ts)
- **Traces, metrics, logs:** a local NDJSON trace file is always written; OTLP export happens only
  when the user sets `T3CODE_OTLP_*` URLs ("OTLP export is opt-in").
  [observability.md#L133](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/docs/operations/observability.md#L133)
- **Resource telemetry** (a Rust `sysinfo` process monitor) stays on the machine and feeds the
  diagnostics view. [T3/docs/internals/resource-telemetry.md](https://github.com/pingdotgg/t3code/blob/b3b6ae2cebbae39a706038911dbb8c85c7120be7/docs/internals/resource-telemetry.md)

### Pushback

- **2026-03-25, [#1397](https://github.com/pingdotgg/t3code/issues/1397)** "Telemetry opt-out
  functionality": found via a sandbox logging hundreds of blocked requests to `us.i.posthog.com`.
  A maintainer replied: "We only collect data so we know what platforms people are using and how
  many users we have… We will not make this easier to opt out of, it is crucial for us to know how
  many users we have." Closed as not planned on 2026-06-23. Later comments raise GDPR and the
  unsalted account-ID hashes.
- **2026-07-18, [#4123](https://github.com/pingdotgg/t3code/issues/4123)** "Why promise no opt-out
  telemetry while telemetry has been on the whole time?": the marketing homepage said "No
  telemetry. Unless you opt in. Full stop." until commit
  [3bdaa6e](https://github.com/pingdotgg/t3code/commit/3bdaa6e10046945ccc08264d2d6f3b81775efcb3)
  (2026-06-18) removed it, while collection had been on since March. Open; one commenter says they
  only noticed because of TLS errors to PostHog at work.
- **2026-07-28, [#4707](https://github.com/pingdotgg/t3code/issues/4707)**: a downstream
  distribution ("ZL") filed a plan to strip the inherited PostHog defaults from its build.

## 3. Developer tools

Detail for the table rows. Seed research, re-checked by spot-checking links on 2026-10-04.

### VS Code and VSCodium

- Default `telemetryLevel` is `all`: crash reports, errors and usage.
  https://code.visualstudio.com/docs/configure/telemetry ;
  [VSC/…/telemetryService.ts#L321](https://github.com/microsoft/vscode/blob/5e86c7c4c2eb99b22e631e0aa4eb05f6b8b53f35/src/vs/platform/telemetry/common/telemetryService.ts#L321)
- Common properties on every event: `machineId`, `sqmId`, `devDeviceId`, `sessionID`, commit,
  version, platform, arch.
  [VSC/…/commonProperties.ts](https://github.com/microsoft/vscode/blob/5e86c7c4c2eb99b22e631e0aa4eb05f6b8b53f35/src/vs/platform/telemetry/common/commonProperties.ts)
- `machineId` is SHA-256 of a MAC address, falling back to a random UUID.
  [VSC/src/vs/base/node/id.ts#L94](https://github.com/microsoft/vscode/blob/5e86c7c4c2eb99b22e631e0aa4eb05f6b8b53f35/src/vs/base/node/id.ts#L94).
  `devDeviceId` is a random UUID shared across Microsoft developer tools including the .NET SDK.
  [microsoft/vscode-deviceid](https://github.com/microsoft/vscode-deviceid/blob/30f72eb44313814a9272a16fcebd8802bbeeff66/src/storage.ts)
- Notice: welcome-page footer "{0} collects usage data. Read our privacy statement and learn how
  to opt out."
  [VSC/…/gettingStarted.ts#L1665](https://github.com/microsoft/vscode/blob/5e86c7c4c2eb99b22e631e0aa4eb05f6b8b53f35/src/vs/workbench/contrib/welcomeGettingStarted/browser/gettingStarted.ts#L1665).
  `code --telemetry` lists every event, including extensions'
  ([argv.ts#L160](https://github.com/microsoft/vscode/blob/5e86c7c4c2eb99b22e631e0aa4eb05f6b8b53f35/src/vs/platform/environment/node/argv.ts#L160)),
  and events carry `__GDPR__` annotations in source.
- Off: `telemetry.telemetryLevel` (`all`/`error`/`crash`/`off`), `--disable-telemetry`,
  `--disable-crash-reporter`, and an enterprise policy since 1.99.
  https://code.visualstudio.com/docs/enterprise/telemetry
- Backend: 1DS to `mobile.events.data.microsoft.com`, crashes through Crashpad to App Center; the
  OSS `product.json` has no telemetry keys.
  [1dsAppender.ts#L22](https://github.com/microsoft/vscode/blob/5e86c7c4c2eb99b22e631e0aa4eb05f6b8b53f35/src/vs/platform/telemetry/common/1dsAppender.ts#L22)
- Extension telemetry is "not controlled by the telemetry.telemetryLevel setting".
  https://code.visualstudio.com/docs/supporting/faq
- **VSCodium** builds with telemetry off: `undo_telemetry.sh` rewrites `*.data.microsoft.com` to
  `0.0.0.0`, and a patch sets `telemetryLevel` to `off`. Its docs list the update checks that
  remain. [VSCod/undo_telemetry.sh](https://github.com/VSCodium/vscodium/blob/5a73682ca091082675b10c9dc3f348c1d824d94f/undo_telemetry.sh),
  [VSCod/docs/telemetry.md](https://github.com/VSCodium/vscodium/blob/5a73682ca091082675b10c9dc3f348c1d824d94f/docs/telemetry.md)

### Zed

- `"diagnostics": true` and `"metrics": true` by default.
  [Z/assets/settings/default.json#L1698-L1707](https://github.com/zed-industries/zed/blob/a84689073d296dfd39987bc7dd478e43ef76d83a/assets/settings/default.json#L1698-L1707)
- Each batch carries `system_id`, `installation_id`, `session_id`, `metrics_id`, version, OS,
  arch, channel; posted to `api.zed.dev/telemetry/events` every 5 minutes or 50 events.
  [Z/crates/telemetry_events/src/telemetry_events.rs#L7-L28](https://github.com/zed-industries/zed/blob/a84689073d296dfd39987bc7dd478e43ef76d83a/crates/telemetry_events/src/telemetry_events.rs#L7-L28),
  [Z/crates/client/src/telemetry.rs#L71-L81](https://github.com/zed-industries/zed/blob/a84689073d296dfd39987bc7dd478e43ef76d83a/crates/client/src/telemetry.rs#L71-L81)
- Metrics cover file extensions, features, project stats and frameworks. Docs: "If you've
  authenticated, this ID may be linked to your email." https://zed.dev/docs/telemetry
- The server adds a country from Cloudflare's `CF-IPCountry`; the privacy policy lists "IP address
  (and inferred approximate location)". https://zed.dev/privacy-policy
- Crash minidumps go to Sentry on next launch, only if `ZED_MINIDUMP_ENDPOINT` was set at build
  time (official CI sets it).
  [Z/crates/zed/src/reliability.rs#L300-L420](https://github.com/zed-industries/zed/blob/a84689073d296dfd39987bc7dd478e43ef76d83a/crates/zed/src/reliability.rs#L300-L420)
- Onboarding page shows both switches already on; "App First Opened" is queued before the user
  sees them **(unverified whether sent after opting out there)**.
  [Z/crates/onboarding/src/basics_page.rs#L244-L330](https://github.com/zed-industries/zed/blob/a84689073d296dfd39987bc7dd478e43ef76d83a/crates/onboarding/src/basics_page.rs#L244-L330)
- Backends named in docs: Sentry, Snowflake, Hex, Amplitude.

### GitHub Desktop

- One usage payload per 24 h after the welcome flow: version, OS, arch, theme, editor, shell,
  account type, repo counts and feature counters (29 dimensions and 192 measures in the documented
  example) to `central.github.com/api/usage/desktop`.
  [GD/docs/process/usage-data.json](https://github.com/desktop/desktop/blob/3754e26d1f021ccb2de77f86061ea1dab2c95d39/docs/process/usage-data.json),
  [GD/app/src/lib/stats/stats-store.ts#L63-L66](https://github.com/desktop/desktop/blob/3754e26d1f021ccb2de77f86061ea1dab2c95d39/app/src/lib/stats/stats-store.ts#L63-L66)
- Fatal crashes are posted with stack, platform, version and `guid` whatever the opt-out says.
  [GD/app/src/main-process/exception-reporting.ts](https://github.com/desktop/desktop/blob/3754e26d1f021ccb2de77f86061ea1dab2c95d39/app/src/main-process/exception-reporting.ts)
- `guid` is `crypto.randomUUID()`; the updater has its own UUID.
  [GD/app/src/lib/get-main-guid.ts](https://github.com/desktop/desktop/blob/3754e26d1f021ccb2de77f86061ea1dab2c95d39/app/src/lib/get-main-guid.ts),
  [GD/app/src/lib/get-updater-guid.ts](https://github.com/desktop/desktop/blob/3754e26d1f021ccb2de77f86061ea1dab2c95d39/app/src/lib/get-updater-guid.ts)
- The GitHub.com account ID was added to the payload in 2018 (with a "usage stats change" dialog
  telling users) and dropped in [PR #11845](https://github.com/desktop/desktop/pull/11845)
  (2021-03, titled "Remove unused code and usage stats from welcome flow"; the diff removes
  `findDotComAccountId` and the `user_id` field).
- Notice: "GitHub Desktop sends usage metrics to improve the product… Learn more" on the welcome
  screen. [GD/app/src/ui/welcome/start.tsx#L98-L103](https://github.com/desktop/desktop/blob/3754e26d1f021ccb2de77f86061ea1dab2c95d39/app/src/ui/welcome/start.tsx#L98-L103)
- Off: Settings > Advanced; the change itself sends `{optIn, previousOptInValue}` without the
  guid. A registry/policy switch was declined ([#10906](https://github.com/desktop/desktop/issues/10906)).

### Homebrew

- Events `formula_install`, `cask_install`, `build_error` with arch, OS version, prefix kind, CI,
  package and options.
  [HB/Library/Homebrew/utils/analytics.rb#L76-L106](https://github.com/Homebrew/brew/blob/8b92a1a6b5716d1719d3a424e6955c725673cbe2/Library/Homebrew/utils/analytics.rb#L76-L106).
  Since 2026-07 also `command_run` with option values stripped
  ([#23153](https://github.com/Homebrew/brew/pull/23153)).
- No ID since [#14684](https://github.com/Homebrew/brew/pull/14684) (2023-02-20, "analytics:
  remove UUID.").
- Nothing is sent until the notice has been shown on a TTY: "Homebrew collects anonymous
  analytics… No analytics have been recorded yet (nor will be during this `brew` run)".
  [HB/Library/Homebrew/cmd/update-report.rb#L395-L424](https://github.com/Homebrew/brew/blob/8b92a1a6b5716d1719d3a424e6955c725673cbe2/Library/Homebrew/cmd/update-report.rb#L395-L424)
- Off: `brew analytics off` or `HOMEBREW_NO_ANALYTICS=1`. https://docs.brew.sh/Analytics
- Backend history: Google Analytics 2016 → GA plus InfluxDB EU (4.0.0, 2023-02) → GA data deleted
  2023-06 (https://brew.sh/2023/07/20/homebrew-4.1.0/) → own proxy `analytics.brew.sh` since
  2026-08-24 ([#23639](https://github.com/Homebrew/brew/pull/23639)). 365-day retention; public
  aggregates at https://formulae.brew.sh/analytics/.

### Go toolchain

- Default `local`: counters stay in `os.UserConfigDir()/go/telemetry`, never uploaded.
  https://go.dev/doc/telemetry
- `go telemetry on` uploads weekly reports of counters allowlisted in a public config, plus crash
  stack counters; only reports dated after opting in are uploaded.
  [GT/internal/upload/findwork.go#L58-L63](https://github.com/golang/telemetry/blob/ed294f9431572147af3f0aaa3fe932a93b698664/internal/upload/findwork.go#L58-L63)
- "Uploaded reports do not include user IDs, machine IDs, or any other kind of ID"; IPs "are not
  recorded with the reports". https://github.com/golang/go/issues/58894
- gopls asks a 1% sample after 7 days of use.
  [golang/tools…/prompt.go#L28-L39](https://github.com/golang/tools/blob/3f3efb7c3b31192232c67403b23a00bfcdbc3eec/gopls/internal/server/prompt.go#L28-L39)
- Server open source; new counters need a public proposal; data public at https://telemetry.go.dev.

### .NET CLI

- Default: timestamp, hashed command and arguments, "three octet IP address", OS, runtime ID, SDK
  version, hashed MAC, hashed cwd, crash exception type and SDK stack traces, CI flag; SDK 10 adds
  LLM-agent detection and a hashed project ID.
  [dotnet/docs telemetry.md](https://github.com/dotnet/docs/blob/11495c160760a809e8b281cab293643f65b45e7d/docs/core/tools/telemetry.md),
  [DN/…/TelemetryCommonProperties.cs](https://github.com/dotnet/sdk/blob/590b0970fe66407ef532b7c95335a0ac4d08f1ab/src/Cli/dotnet/Telemetry/TelemetryCommonProperties.cs)
- Merged 2026-09-30: a Microsoft-employee classification
  ([#56388](https://github.com/dotnet/sdk/pull/56388)).
- Builds from source have telemetry off unless the `MICROSOFT_ENABLE_TELEMETRY` compile flag is
  set. [DN/src/Common/CompileOptions.cs](https://github.com/dotnet/sdk/blob/590b0970fe66407ef532b7c95335a0ac4d08f1ab/src/Common/CompileOptions.cs)
- First-run banner: "The .NET tools collect usage data… You can opt-out of telemetry by setting
  the DOTNET_CLI_TELEMETRY_OPTOUT environment variable…". Aggregates published under CC-BY.
- Backend: Application Insights. ([dotnet/sdk#49668](https://github.com/dotnet/sdk/issues/49668))

### Next.js and Vercel CLI

- Next.js: command, version, CPU count, OS, CI, plugins, build stats; IDs are a random
  `anonymousId` and `projectId` = SHA-256(local salt + git remote URL).
  https://nextjs.org/telemetry ;
  [NX/…/project-id.ts](https://github.com/vercel/next.js/blob/ba80ee48fc319735151c3ad6d9bb9a8180c9f09e/packages/next/src/telemetry/project-id.ts).
  Notice once: "Attention: Next.js now collects completely anonymous telemetry regarding usage…".
  Off: `next telemetry disable` or `NEXT_TELEMETRY_DISABLED=1`; `NEXT_TELEMETRY_DEBUG=1` prints
  events instead of sending.
- Vercel CLI (since 39.0.0, 2024-11, [#12555](https://github.com/vercel/vercel/pull/12555)):
  command, args and flags, version, OS, CI, and `team_id`, `user_id`, `project_id` unhashed.
  https://vercel.com/docs/cli/about-telemetry ;
  [VC/…/telemetry/index.ts](https://github.com/vercel/vercel/blob/c628be7835e03a965b93e9cf9e2bd5ac2acbf5eb/packages/cli/src/util/telemetry/index.ts)

### Deno

- No usage telemetry. The only automatic call is a GET of `dl.deno.land/release-latest.txt` at
  most daily, User-Agent `Deno/<version>`, no ID.
  [DE/cli/tools/upgrade.rs#L1446-L1474](https://github.com/denoland/deno/blob/b4f08f127652d8442b4d3dbabc277aca3840bc1d/cli/tools/upgrade.rs#L1446-L1474)
- Off: `DENO_NO_UPDATE_CHECK`, or build without the `upgrade` feature, "typically disabled for
  (Linux) distribution packages".
  [DE/cli/Cargo.toml#L32-L41](https://github.com/denoland/deno/blob/b4f08f127652d8442b4d3dbabc277aca3840bc1d/cli/Cargo.toml#L32-L41)
- A panic prints a `panic.deno.com` link with the trace encoded in the URL; nothing is uploaded
  unless the user opens it.
  [DE/cli/lib.rs#L583-L690](https://github.com/denoland/deno/blob/b4f08f127652d8442b4d3dbabc277aca3840bc1d/cli/lib.rs#L583-L690)

### Rust

- rustup's 2016 telemetry was opt-in and local-only, and was removed by
  [PR #1642](https://github.com/rust-lang/rustup/pull/1642) (2019-02-26). Its User-Agent now
  carries version and TLS backend, measured from server logs
  ([#3815](https://github.com/rust-lang/rustup/pull/3815)).
- Cargo "does not send data anywhere" (2025h2 goal, proposed).
  https://goals.rust-lang.org/2025h2/cargo-build-analysis.html
- rustc: "NO TELEMETRY, NO NETWORK CONNECTIONS… sharing… opt-in, clear, and manual"
  ([#128914](https://github.com/rust-lang/rust/issues/128914)); compiler-team MCP
  [#679](https://github.com/rust-lang/compiler-team/issues/679) calls telemetry "an explicit
  non-goal".
- Usage data comes from the opt-in annual State of Rust survey (7,310 responses in 2024).
  https://blog.rust-lang.org/2025/02/13/2024-State-Of-Rust-Survey-results/

## 4. Git GUIs

- **GitKraken:** the privacy policy (2026-03-19) lists "some usage analytics, the directory,
  folder and file names on your device where you store your code, and any crash reports", plus
  "IP address (including geolocation), operating system, product version"; opt-out "to the extent
  available". The Preferences docs list no telemetry setting. https://www.gitkraken.com/privacy ;
  https://help.gitkraken.com/gitkraken-desktop/preferences/
- **Fork:** 2018, "Fork uses Fabric to track number of active users. You can disable that in
  Preferences" (Mac only) ([#313](https://github.com/fork-dev/Tracker/issues/313)). 2023: Fabric
  removed years earlier, "doesn't send any telemetry or analytics", crashes via AppCenter
  ([#1910](https://github.com/fork-dev/Tracker/issues/1910)). 2024: "Fork doesn't call home and has
  no telemetry. So we don't have a privacy policy."
  ([#2046](https://github.com/fork-dev/Tracker/issues/2046))
- **Sublime Merge:** "We don't have a privacy policy but avoid doing data collection in the first
  place" (staff, 2024, https://forum.sublimetext.com/t/sublime-text-security-controls/72251).
  The binary contains `/updates/stable_update_check?version=…&platform=…&arch=…` and a Crashpad
  upload URL `crash-server.sublimehq.com`; the update-check box only works when licensed
  (https://forum.sublimetext.com/t/sublime-merge-disable-new-version-available-popup-after-the-update-has-already-been-discovered/76978).
- **Sourcetree:** welcome-wizard checkbox "Help improve Sourcetree by sending anonymous data about
  your usage"; sends "the number of repos you've interacted with and the provider you're using"
  (https://community.atlassian.com/forums/Sourcetree-questions/Security-information-about-Sourcetree/qaq-p/1104747).
  E-MAU registration analytics "can't disable… forms part of the EULA"
  (https://community.atlassian.com/forums/Sourcetree-questions/How-to-disable-EMauSubmissionService-in-MSI-installation-of/qaq-p/977800).
- **TortoiseGit:** weekly check of `versioncheck.tortoisegit.org/version.txt` with a signature,
  no ID; User-Agent `TortoiseGit <ver>; <platform>; Windows <edition> <version>`; random weekday,
  not on first start.
  [TG/src/TortoiseProc/CheckForUpdatesDlg.cpp#L190-L213](https://gitlab.com/tortoisegit/tortoisegit/-/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/CheckForUpdatesDlg.cpp#L190-L213),
  [UpdateDownloader.cpp#L41-L43](https://gitlab.com/tortoisegit/tortoisegit/-/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/UpdateDownloader.cpp#L41-L43).
  Crash dumps go to drdump.com from official builds; the installer feature "Crash Reporter – Sends
  crash dumps to the developers" is on by default.
  [TG/src/TortoiseGitSetup/FeaturesFragment.wxi#L84-L86](https://gitlab.com/tortoisegit/tortoisegit/-/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseGitSetup/FeaturesFragment.wxi#L84-L86) ;
  https://tortoisegit.org/support/
- **gitui:** no HTTP client in `Cargo.lock`, no update check.
  [GU/Cargo.lock](https://github.com/gitui-org/gitui/blob/2fa693cb6ed431b21ebc300dd02e83c2476699ce/Cargo.lock)
- **lazygit:** no telemetry. Update check is `GET github.com/jesseduffield/lazygit/releases/latest`
  then a HEAD on an asset URL containing version, OS and arch; `update.method:
  prompt|background|never`, every 14 days; official builds never check automatically because of a
  build-source mismatch ([#2454](https://github.com/jesseduffield/lazygit/issues/2454)).
  [LG/pkg/updates/updates.go#L153-L186](https://github.com/jesseduffield/lazygit/blob/ff375b124d149f03f9156d3720a998cbc04482fa/pkg/updates/updates.go#L153-L186)
- **GitButler:** `appMetricsEnabled` and `appErrorReportingEnabled` default true; onboarding
  "Before we begin" shows both on: "We do not collect any personal information, unless explicitly
  allowed below." Nothing initialises before onboarding is done. PostHog EU (autocapture and
  session recording off) and Sentry. The CLI update check posts `sha256(machine_uid + "gitbutler")`
  regardless of the metrics setting. The launch post said "our telemetry is opt-out"
  (https://blog.gitbutler.com/opening-up-gitbutler).
  [GB/crates/but-settings/assets/defaults.jsonc#L8-L18](https://github.com/gitbutlerapp/gitbutler/blob/12cd33291625e7f9e3907c5983a3b943b4a997ba/crates/but-settings/assets/defaults.jsonc#L8-L18),
  [GB/apps/desktop/src/components/shared/AnalyticsSettings.svelte#L14](https://github.com/gitbutlerapp/gitbutler/blob/12cd33291625e7f9e3907c5983a3b943b4a997ba/apps/desktop/src/components/shared/AnalyticsSettings.svelte#L14),
  [GB/crates/but-update/src/check.rs#L62-L130](https://github.com/gitbutlerapp/gitbutler/blob/12cd33291625e7f9e3907c5983a3b943b4a997ba/crates/but-update/src/check.rs#L62-L130)

## 5. Desktop apps

### Syncthing

- **Opt-in with a preview.** `urAccepted` defaults to `0`: "the user has not made a choice, and
  Syncthing will ask at some point in the future. `-1` means no, a number above zero means that
  that version of usage reporting has been accepted." https://docs.syncthing.net/users/config.html
- **The prompt** is a GUI modal: "The encrypted usage report is sent daily. It is used to track
  common platforms, folder sizes, and app versions. If the reported data set is changed you will
  be prompted with this dialog again. The aggregated statistics are publicly available at the URL
  below." It has a **Preview Usage Report** button that shows the exact JSON, and equal Yes/No
  buttons.
  [ST/gui/default/syncthing/usagereport/usageReportModalView.html](https://github.com/syncthing/syncthing/blob/7ad73b408adc792cabeed41d89a37b93e2bd84d0/gui/default/syncthing/usagereport/usageReportModalView.html)
- **Re-prompting.** The report format has a version (`const Version = 3`); a user who accepted an
  older version sees "Anonymous usage report format has changed. Would you like to move to the new
  format?" `urSeen` records the highest version already shown.
  [ST/lib/ur/usage_report.go#L40](https://github.com/syncthing/syncthing/blob/7ad73b408adc792cabeed41d89a37b93e2bd84d0/lib/ur/usage_report.go#L40)
- **Payload:** `uniqueID`, version, platform, folder and device counts, file counts and sizes,
  memory, CPU count, hash performance, and counts of feature use (folder types, versioning types,
  discovery and relay settings, NAT type, uptime).
  [ST/lib/ur/contract/contract.go](https://github.com/syncthing/syncthing/blob/7ad73b408adc792cabeed41d89a37b93e2bd84d0/lib/ur/contract/contract.go)
- **ID:** `urUniqueID` is "Generated when usage reporting is enabled". First report 30 minutes
  after start (`urInitialDelayS` 1800), then every 24 h, to `https://data.syncthing.net/newdata`.
  https://docs.syncthing.net/users/config.html ;
  [usage_report.go#L393](https://github.com/syncthing/syncthing/blob/7ad73b408adc792cabeed41d89a37b93e2bd84d0/lib/ur/usage_report.go#L393)
- **IP:** the server takes the sender's address (from `X-Forwarded-For`), stores it in the report
  record's `address` field and maps it to a country with GeoLite2.
  [ST/cmd/infra/ursrv/serve/serve.go#L291-L345](https://github.com/syncthing/syncthing/blob/7ad73b408adc792cabeed41d89a37b93e2bd84d0/cmd/infra/ursrv/serve/serve.go#L291-L345),
  [contract.go#L174-L183](https://github.com/syncthing/syncthing/blob/7ad73b408adc792cabeed41d89a37b93e2bd84d0/lib/ur/contract/contract.go#L174-L183)
- **Public data:** aggregated charts at https://data.syncthing.net/.
- **Update check:** default on, at startup and every 12 hours (`autoUpgradeIntervalH` 12,
  `0` disables), from `upgrades.syncthing.net/meta.json`; the requests "do not contain any
  identifiable information about the user or device".
  https://docs.syncthing.net/users/security.html
- No backlash found.

### Firefox

- **Default-on:** "Technical, IP-derived location, and settings data, as well as interaction and
  system performance data (such as number of tabs open, memory usage or the outcome of automated
  processes like updates)". Identifiers listed include "Client_id, session_id, cookie
  identifiers". Notice effective 2026-05-04. https://www.mozilla.org/en-US/privacy/firefox/
- **Release vs prerelease:** extended Telemetry is locked on for Nightly/Beta and off on release.
  https://firefox-source-docs.mozilla.org/toolkit/components/telemetry/internals/preferences.html
- **Separate daily usage ping:** the `usage-reporting` ping is a "Minimal ping to measure the
  usage frequency of Firefox", `include_client_id: false`, with its own `usage.profile_id`
  ("A UUID uniquely identifying the profile, not shared with other telemetry data"), and it does
  not follow the main telemetry switch (`follows_collection_enabled: false`). Opting out of it
  sends a `usage-deletion-request` ping. The privacy notice: "The Daily Usage Ping solely provides
  us with de-identified information that a user is using Firefox; it is not tied to any other data
  about you and you can opt out in settings."
  [FF/toolkit/components/telemetry/pings.yaml#L8-L43](https://github.com/mozilla-firefox/firefox/blob/3f73c528a1ae5784ea5e1ee2c5ad3762507395f2/toolkit/components/telemetry/pings.yaml#L8-L43),
  [FF/toolkit/components/telemetry/metrics.yaml#L805-L820](https://github.com/mozilla-firefox/firefox/blob/3f73c528a1ae5784ea5e1ee2c5ad3762507395f2/toolkit/components/telemetry/metrics.yaml#L805-L820)
- **Opting out** of technical data sends a `deletion-request` ping containing the client ID and
  profile group ID, so the pipeline deletes past data.
  https://firefox-source-docs.mozilla.org/toolkit/components/telemetry/data/deletion-request-ping.html
- **Crash reports:** desktop sends automatically only "if you opt in to sending us crash reports
  automatically"; `browser.crashReports.unsubmittedCheck.autoSubmit2` defaults to `false`, and the
  notification for unsent reports is shown on Nightly only, at most 4 times before it is
  suppressed. Crash reports may include "sites that were open at the time of the crash".
  [FF/browser/app/profile/firefox.js#L3022-L3036](https://github.com/mozilla-firefox/firefox/blob/3f73c528a1ae5784ea5e1ee2c5ad3762507395f2/browser/app/profile/firefox.js#L3022-L3036) ;
  https://www.mozilla.org/en-US/privacy/firefox/
- **Update checks:** "Desktop versions of Firefox regularly connect to Mozilla's servers… to check
  for software updates." https://www.mozilla.org/en-US/privacy/firefox/

### Audacity (Muse Group), 2021 and after

- **What was proposed:** [PR #835](https://github.com/audacity/audacity/pull/835) "Basic telemetry
  for the Audacity" (opened 2021-05-04, 1,091 comments). Google Analytics would receive session
  start/end, errors, import/export formats, OS and Audacity versions and effect use; Yandex
  Metrica only an "application opened" event. The maintainers' clarification stressed it was
  "strictly optional and disabled by default", only in official CI builds, behind a CMake option
  off by default.
- **What was walked back** ([discussion #889](https://github.com/audacity/audacity/discussions/889),
  2021-05-13): "We are dropping the telemetry features proposed in PR #835", and "We assumed that
  making it opt-in would allay privacy concerns but since this isn't the case, we are dropping
  it." Google and Yandex were replaced by self-hosting: "the convenience of using Yandex and
  Google is at odds with the public perception of trustworthiness". They blamed the surprise on
  process: "a bad communication/coordination blunder", unlike MuseScore in 2019 where they had
  announced first.
- **What they kept:**
  - Error reporting: a dialog per error with "An option to view the complete error report data
    before it is sent", equal "send"/"don't send" buttons, and an unchecked "remember" box;
    self-hosted Sentry in the EU.
  - Update checking at start, with an option to disable it; reveals IP, OS and Audacity version;
    country from a self-hosted GeoIP database, raw IP not stored. Both are excluded by CMake
    options from source and distro builds.
- **Second round:** [#1213](https://github.com/audacity/audacity/issues/1213) "New privacy policy
  is completely unacceptable!" (2021-07-03, 535 comments) over the clause "Data necessary for law
  enforcement, litigation and authorities' requests (if any)" and an age limit.
- **Today** (notice updated 2025-02-01): update check sends User-Agent (version, OS) and IP is
  "anonymised immediately… We only store the first three octets"; error reports are shown first
  and sent only on consent; a random UUID "will only be created if you choose to opt-in", used to
  "Understand daily and monthly active users" and "Track retention rates", stored in the
  Netherlands. No law-enforcement clause and no age restriction remain.
  https://www.audacityteam.org/desktop-privacy-notice/

### KDE (KUserFeedback, Plasma)

- **Policy:** "application telemetry is always opt-in. That means off by default and only
  activated by the explicit action of the user"; "we will not use any unique device, installation
  or user id"; IP addresses are "not stored together with the telemetry data"; data goes only to
  "servers under the full control of the KDE sysadmin team".
  https://community.kde.org/Policies/Telemetry_Policy ; https://kde.org/privacypolicy-apps/
- **Plasma 5.18 (2020-02)** added a "Feedback settings" slider in System Settings: "These are
  disabled by default to protect your privacy."
  https://kde.org/announcements/plasma/5/5.18.0/
- **Code:** the library default is `NoTelemetry`
  ([KUF/src/provider/core/provider.cpp#L45](https://github.com/KDE/kuserfeedback/blob/57023cf004072b055dde09bb7d49f79a7aac7966/src/provider/core/provider.cpp#L45));
  modes go Basic system info → Basic usage stats → Detailed system info → Detailed usage stats.
  Plasma sends weekly to `https://telemetry.kde.org/`: application, compiler, platform and Qt
  versions, usage time, OpenGL, screens, QPA and panel count, and shows one encouragement after 5
  starts, 30 s after start.
  [PW/shell/userfeedback.cpp#L57-L69](https://github.com/KDE/plasma-workspace/blob/a3302a698ad247fceadad224a0ecd0b6a74f2516/shell/userfeedback.cpp#L57-L69),
  [PW/kcms/feedback/feedbacksettings.kcfg](https://github.com/KDE/plasma-workspace/blob/a3302a698ad247fceadad224a0ecd0b6a74f2516/kcms/feedback/feedbacksettings.kcfg)
- **Pushback:** procedural only. In 2022 a KDE contributor raised that Plasma and Discover had
  "implemented support for opt-in-telemetry without complying with the requirements listed" in the
  policy, reviewed only in a merge request.
  https://mail.kde.org/pipermail/kde-community/2022q3/007203.html

### Debian and Ubuntu: popcon, ubuntu-report, Ubuntu Insights

- **popcon:** the installer asks "Participate in the package usage survey?", default No (press
  summary; the debconf template on salsa needs a login **(unverified at source)**). Participants
  send weekly the installed packages with each file's atime and ctime, plus architecture and
  vendor. "Each popularity-contest host is identified by a random 128bit uuid (MY_HOSTID…). This
  uuid is used to track submissions issued by the same host." "The administrators of the popcon
  server can associate submissions with a source IP address"; the public summary "does not include
  uuids". https://popcon.debian.org/FAQ
- **Ubuntu popcon:** installed by default since 2006 but not sending unless enabled; removed from
  the standard seed on 2020-07-15 because "the package and backend have both been broken since
  18.04 LTS without being much missed".
  https://discourse.ubuntu.com/t/popcon-to-be-removed-from-the-standard-seed/17238
- **ubuntu-report (2018):** Will Cooke's proposal (2018-02-14): an installer checkbox "Send
  diagnostics information to help improve Ubuntu… This would be checked by default", sending
  flavour, version, CPU, RAM, disks, screens, GPU, OEM, user-chosen location ("No IP information
  would be gathered"), install duration and choices; popcon installed and Apport crash reports sent
  automatically; results public; "Any user can simply opt out by unchecking the box, which
  triggers one simple POST stating, 'diagnostics=false'."
  https://lists.ubuntu.com/archives/ubuntu-devel/2018-February/040188.html .
  The shipped tool shows the report and asks before upload, once per release, to
  `metrics.ubuntu.com`; "This information can't be used to identify a single machine".
  https://github.com/ubuntu/ubuntu-report/blob/c9dfa34bf6daf43879ccf53f438c4cefb9d5e8ab/README.md
  Which answer the 18.04 welcome screen pre-selected is **(unverified)**.
- **Ubuntu Insights (25.10+):** replaces ubuntu-report in GNOME Initial Setup; adds monthly
  reports; stores reports as plain JSON in `~/.cache/ubuntu-insights/` before upload and keeps a
  copy of what was sent; earlier ubuntu-report answers do not carry over, so users are asked again;
  declining sends only an opt-out flag (`"OptOut": true`).
  https://discourse.ubuntu.com/t/ubuntu-insights-how-telemetry-is-changing-on-ubuntu-desktop/73442 ;
  https://github.com/ubuntu/ubuntu-insights/blob/732c78d85787cb948d38d22f937b2210327ff249/README.md

## 6. Backlash and incident timeline

| Date | Tool | Trigger | Outcome | Link |
|---|---|---|---|---|
| 2014-06 | Sourcetree (Mac) | Unticking usage data in setup not carried to Preferences | Fixed next day | https://jira.atlassian.com/browse/SRCTREE-2447 |
| 2015-11 | TortoiseGit | Double-negative crash dialog led users to upload dumps | Explained; reporter optional at install | https://gitlab.com/tortoisegit/tortoisegit/-/issues/2642 |
| 2016-04 | rustup | Telemetry proposal | Shipped opt-in and local-only; removed 2019 | https://github.com/rust-lang/rustup/issues/254 |
| 2016-04-25 | Homebrew | Google Analytics on by default with a UUID | Pre-send notice and one-command opt-out the same day; opt-in refused | https://github.com/Homebrew/brew/issues/142 |
| 2016-05 | .NET CLI | "should not SPY on users by default" (212 comments) | Banner, docs and public data added; model kept | https://github.com/dotnet/sdk/issues/6145 |
| 2016-11 | VS Code | Traffic after opt-out | Final "opted out" event removed | https://github.com/microsoft/vscode/issues/16131 |
| 2017-03/05 | GitHub Desktop | Stats sent despite opt-out; checkbox could not be cleared | Fixed | https://github.com/desktop/desktop/issues/1064 ; https://github.com/desktop/desktop/issues/1698 |
| ~2017 | Sourcetree (Win) | Usage box re-ticked after each update | No recorded outcome | https://community.atlassian.com/forums/discussion/598406/after-every-update-sourcetree-always-enables-sending-of-usage-data |
| 2018-02 | Ubuntu | Diagnostics checkbox ticked by default | Shipped as ubuntu-report with payload shown; replaced 2025 by Insights, re-asking everyone | https://lists.ubuntu.com/archives/ubuntu-devel/2018-February/040188.html |
| 2018-04 | VS Code | #47284 "make it opt-in" | Not done; telemetry viewer added; still open | https://github.com/microsoft/vscode/issues/47284 |
| 2018-05 | VS Code | Settings search sent keystrokes to Bing with telemetry off | Separate setting | https://github.com/microsoft/vscode/issues/49161 |
| 2018-06 | Fork | User found connections to AWS (Fabric) | Explained; Fabric later removed | https://github.com/fork-dev/Tracker/issues/313 |
| 2018-11 | Sourcetree 3.0.8 | Setup required ticking usage data | Fixed (closed 2021) | https://jira.atlassian.com/browse/SRCTREEWIN-10821 |
| 2019-08 | Next.js | RFC asked for opt-in | Declined; shipped opt-out | https://github.com/vercel/next.js/issues/8442 |
| 2021-03 | GitHub Desktop | GitHub user ID in stats since 2018 | Removed | https://github.com/desktop/desktop/pull/11845 |
| 2021-05-04 | Audacity | Opt-in telemetry PR using Google Analytics and Yandex (1,091 comments) | Dropped all usage telemetry; self-hosted error reports (per-report consent) and update check | https://github.com/audacity/audacity/pull/835 ; https://github.com/audacity/audacity/discussions/889 |
| 2021-07-03 | Audacity | Privacy policy: law-enforcement clause, age limit (535 comments) | Policy rewritten; current notice has neither | https://github.com/audacity/audacity/issues/1213 |
| 2021-09/10 | VS Code | Confusing settings | `telemetryLevel`; opted-out users migrated to `off` | https://code.visualstudio.com/updates/v1_61 |
| 2022-07 | KDE | Telemetry added to Plasma/Discover without the policy's review | Procedural discussion | https://mail.kde.org/pipermail/kde-community/2022q3/007203.html |
| 2023-02-08 | Go | Opt-out "transparent telemetry" proposal (95 top-level comments plus replies) | Redesigned as opt-in within weeks; Go 1.23 defaults to `local` | https://github.com/golang/go/discussions/58409 ; https://research.swtch.com/telemetry-opt-in |
| 2023-02/06 | Homebrew | GDPR complaint about the UUID; GA | UUID removed; GA data destroyed | https://github.com/Homebrew/brew/pull/14684 ; https://brew.sh/2023/07/20/homebrew-4.1.0/ |
| 2023-03 | VS Code | "Require user consent" | Still open | https://github.com/microsoft/vscode/issues/176269 |
| 2023-04 | Deno | "Disable version checking" | Pointed to `DENO_NO_UPDATE_CHECK` | https://github.com/denoland/deno/issues/18663 |
| 2024-02 | GitButler | "tracking must be opt-in (GDPR)" | First-start choice; nothing loads before onboarding; identifiable metrics opt-in | https://github.com/gitbutlerapp/gitbutler/issues/2638 |
| 2024-07/09 | Firefox | Privacy-Preserving Attribution enabled by default in an update | noyb GDPR complaint (2024-09) | https://noyb.eu/en/firefox-tracks-you-privacy-preserving-feature |
| 2024-12 | Zed | Copilot telemetry sent with Zed telemetry off | Closed not planned | https://github.com/zed-industries/zed/issues/21737 |
| 2025-02 | Firefox | Terms of Use licence wording; "never sell" FAQ line removed | Terms revised within days; "Mozilla doesn't sell data about you" | https://blog.mozilla.org/en/firefox/update-on-terms-of-use/ |
| 2025-08 | Zed | "Zedless" fork removing telemetry | Fork exists | https://news.ycombinator.com/item?id=44964916 |
| 2025-09 | GitButler | Deleting settings brings telemetry back on | Open | https://github.com/gitbutlerapp/gitbutler/issues/10358 |
| 2026-01 | GitButler | Telemetry sent while off (CLI ignored the toggle) | Fixed in 0.18.6 | https://github.com/gitbutlerapp/gitbutler/issues/11932 |
| 2026-03-25 | t3code | Undocumented default-on PostHog, env var only | "We will not make this easier to opt out of"; closed not planned | https://github.com/pingdotgg/t3code/issues/1397 |
| 2026-07 | Zed | "Anonymous" disputed given persistent IDs | Open | https://github.com/zed-industries/zed/issues/61630 |
| 2026-07-18 | t3code | Homepage promised "No telemetry. Unless you opt in" while collection was on | Docs page added 2026-09-04; no toggle; open | https://github.com/pingdotgg/t3code/issues/4123 |

No incidents found for gitui, lazygit, Sublime Merge or Syncthing.

## 7. Pattern

All of this section is **(derived)** from the tables above.

### Default-on without lasting complaint

- **Update checks** that send version, OS and architecture (in the URL or User-Agent) and no ID:
  TortoiseGit, lazygit, Deno, Syncthing, Sublime Merge, Audacity, Firefox, t3code, GitHub Desktop.
  Even projects that dropped or refused analytics kept them (Audacity 2021, Deno). The off switch
  is a setting or env var; distros sometimes compile them out (Deno, Audacity).
- **Server-side counts from those checks**: Audacity derives country, version and OS from its
  update check and truncates the IP; rustup counts its User-Agent; Syncthing and Zed map IP to
  country. Nobody here was challenged for counting update-check requests.
- **Crash reports** are split. Automatic and default-on: GitHub Desktop (even when opted out),
  Zed, VS Code, TortoiseGit official builds. Per-crash consent: Audacity, Firefox desktop. Neither
  model drew much complaint except TortoiseGit's confusing dialog; the visible concern is wording
  and whether "no" sticks.
- **Default-on usage analytics with a random install ID** in developer tools: VS Code, .NET,
  Next.js, Vercel, Zed, GitHub Desktop, GitButler, t3code. Every one has had "make it opt-in"
  requests and every one kept the default. These persist when a first-run notice exists, the off
  switch is one step, and the data is not tied to an account.

### What reliably triggers backlash

1. **Users finding it before being told.** t3code (sandbox logs, TLS errors), Fork (AWS
   connections), Audacity (a PR spotted before the announcement; their own post-mortem names this
   as the cause). Contradicting an earlier promise makes it worse (t3code's homepage).
2. **Sending after the user said no**, or toggles that do not stick: VS Code 2016, GitHub Desktop
   2017, Sourcetree, GitButler 2025–2026, Zed's Copilot traffic. These are fixed as bugs whatever
   the project's stance on defaults.
3. **Third-party ad or analytics SDKs.** Homebrew's Google Analytics (moved off and deleted the
   data), Audacity's Google and Yandex (dropped even though opt-in), Fork's Fabric (removed).
   PostHog (GitButler, t3code) and Sentry have not drawn the same reaction in the incidents found.
4. **Identifiers tied to a person.** GitHub Desktop's account ID (removed), Homebrew's UUID under
   a GDPR complaint (removed), t3code's unsalted hash of Codex/Claude account IDs (criticised in
   #1397), Zed's `metrics_id` "may be linked to your email" (#61630). Vercel sends unhashed
   user/team IDs without a recorded incident.
5. **Toolchains and compilers on opt-out.** Go's 2023 opt-out proposal became opt-in within
   weeks; rustup removed its telemetry; rustc rules it out. The same model is tolerated in
   editors and app CLIs (.NET is the exception that held).
6. **Vague legal text** rather than code: Audacity's law-enforcement clause, Firefox's 2025 Terms.
7. **New measurement features turned on by an update**: Firefox's Privacy-Preserving
   Attribution (regulator complaint). Ubuntu chose to re-ask everyone when moving to Insights
   instead of carrying old consent over; Syncthing re-asks when its report format changes.

### What settled or pre-empted complaints

- **Notice before the first send** (Homebrew sends nothing until the notice has been shown;
  GitButler initialises nothing before onboarding).
- **Showing the exact payload**: Syncthing's preview button, ubuntu-report/Insights JSON on disk,
  VS Code `--telemetry` and output channel, `NEXT_TELEMETRY_DEBUG`, Audacity's error dialog,
  Zed's telemetry log.
- **Public aggregates**: Homebrew, Go, Syncthing, .NET (CC-BY), Ubuntu, Debian popcon.
- **Self-hosting or EU hosting** as the answer to "who sees it": Audacity, Homebrew, KDE.
- **Off switches** as a one-word command plus an env var for CI (Homebrew, Next.js, Vercel, Go,
  .NET), or a setting next to the notice (GitHub Desktop, Zed, GitButler, Firefox).
- **Opt-out pings without an ID** (GitHub Desktop, Ubuntu's `diagnostics=false`, Firefox's
  deletion-request) are used to count opt-outs; none drew complaint here.

### How the accepted ones word it

Short, concrete, with the off switch in the same breath:

- Homebrew: "Homebrew collects anonymous analytics… No analytics have been recorded yet (nor will
  be during this `brew` run)", followed by the `brew analytics off` command.
- .NET: "The .NET tools collect usage data… You can opt-out of telemetry by setting the
  DOTNET_CLI_TELEMETRY_OPTOUT environment variable…"
- Next.js: "Attention: Next.js now collects completely anonymous telemetry regarding usage…",
  with the disable command and a link.
- VS Code: "{0} collects usage data. Read our privacy statement and learn how to opt out."
- GitHub Desktop: "GitHub Desktop sends usage metrics to improve the product… Learn more".
- GitButler: "We do not collect any personal information, unless explicitly allowed below."
- Syncthing: "The encrypted usage report is sent daily. It is used to track common platforms,
  folder sizes, and app versions. If the reported data set is changed you will be prompted with
  this dialog again. The aggregated statistics are publicly available…"
- Firefox, for its usage ping: "solely provides us with de-identified information that a user is
  using Firefox; it is not tied to any other data about you and you can opt out in settings."

The word "anonymous" is itself contested when a stable ID is sent (Zed #61630, t3code #1397), and
the accepted notices that use it either send no ID (Homebrew since 2023) or say what is sent.
