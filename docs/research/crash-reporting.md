# Crash reporting for a Rust desktop app

Research note for [#221](https://github.com/aquamoth/parterre/issues/221), part of the map
*Measure usage and crash reports* ([#175](https://github.com/aquamoth/parterre/issues/175)): *how
can a Rust egui desktop app on Windows, Linux and macOS catch crashes and get them to the
maintainer, on free tiers only?*

Researched 2026-10-04. Sources are official docs, crate sources and other apps' code at pinned
commits, and a few experiments. A statement that is my own conclusion is marked **(derived)**. A
statement I could not check is marked **(unverified)**. A statement I checked by running a
command is marked **(tested 2026-10-04)**. Nothing was built for this note (the disk was nearly
full), so binary sizes are not measured. parterre is quoted at
[`21503e0`](https://github.com/aquamoth/parterre/tree/21503e0b16adfdf72499f7be14312f556cfcb2ad).

## TL;DR

- **parterre catches nothing today.** No panic hook, no `catch_unwind`, and the release build
  strips the symbol table ([Cargo.toml#L76-L79](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/Cargo.toml#L76-L79)).
  On Windows the release build has no console, so a panic's message goes nowhere and the window
  just vanishes. A panicking layout worker leaves "Laying out…" spinning forever (§1).
- **Panics are cheap to catch well.** A `std::panic::set_hook` that writes a local report, and
  on the next start offers a GitHub new-issue page the user reviews and submits, is what Ruffle
  does and costs no service, no account for parterre and no data processor (§2). The panic's
  message and `file:line` survive any release profile; the backtrace needs symbols (§3).
- **A pre-filled issue URL must stay under about 4,000 characters**, percent-encoded. GitHub
  answers 414 from about 8,000, and a logged-out user's login redirect encodes the URL again and
  fails from about 7,600 (tested, §2.4). That fits the message, location, versions and a trimmed
  backtrace of about 2 KB, as Ruffle sends.
- **Release backtraces need `strip = "symbols"` to go.** rustc's own docs say programs that want
  crash reporting "should usually avoid `-Cstrip=symbols`". `strip = "debuginfo"` keeps function
  names on Linux and macOS; `debug = "line-tables-only"` adds `file:line`. On Windows names come
  from the `.pdb`, which rustc always writes but parterre's release doesn't ship (§3).
- **Native crashes (GPU driver, glow, winit, stack overflow) need an out-of-process monitor.**
  Embark's `crash-handler` + `minidumper` is the maintained Rust stack. The easiest way in is
  `sentry-minidump`, new in sentry-rust 0.49.3 (2026-09-21): it re-runs parterre's own binary
  as the monitor and hides the `unsafe` (§4). Only Zed among Rust desktop apps does this.
- **Minidumps are only useful with symbols on a server that can walk stacks.** Sentry's free
  plan does it (upload with `sentry-cli debug-files upload`, kept while used, 90 days idle), but
  only uploaded files: custom symbol servers, such as GitHub release assets, are Business-only.
  GlitchTip shows one frame per thread for native crashes today; Bugsink's minidumps are
  experimental (§5, §6).
- **Hangs:** only Zed (and Firefox) detect them, with a watchdog thread that times main-thread
  work. In egui a heartbeat would have to tell an idle or hidden window from a frozen one, and
  hidden Wayland windows are exactly what froze in #32 (§7).
- **Prior art:** of nine Rust desktop apps, only Zed uploads crashes (minidumps to Sentry,
  opt-out). rerun sends an anonymised panic event. GitButler removed Rust-side Sentry in 2024,
  saying the valuable reports came as GitHub issues. The rest log locally or show a dialog;
  Ruffle opens a pre-filled GitHub issue (§8).
- **Comparison of the four realistic routes** is in §9.

## 1. parterre today

**Release profile** ([Cargo.toml#L76-L79](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/Cargo.toml#L76-L79)):
`lto = "thin"`, `codegen-units = 1`, `strip = "symbols"`. `debug` and `panic` are left at the
release defaults, `debug = false` and `panic = 'unwind'`
([Cargo profiles](https://github.com/rust-lang/cargo/blob/76488151ad6133284397491892dabb50a86f5054/doc/book/src/reference/profiles.md#L296-L314)).
So the binary has no debug info and no symbol table, and panics unwind.

**Nothing catches panics.** There is no `set_hook`, `take_hook`, `catch_unwind` or
`Backtrace` anywhere in the workspace **(tested 2026-10-04:
`grep -rn "set_hook\|take_hook\|catch_unwind\|Backtrace" --include=*.rs`)**, so Rust's default
hook applies: it "prints a message to standard error"
([std::panic::set_hook](https://doc.rust-lang.org/std/panic/fn.set_hook.html)).

**On Windows that message is lost.** Release builds are GUI-subsystem programs
([main.rs#L3-L4](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/crates/parterre/src/main.rs#L3-L4)),
and before the window opens parterre detaches from any console it borrowed
([main.rs#L350-L353](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/crates/parterre/src/main.rs#L350-L353),
[console.rs](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/crates/parterre/src/console.rs)).
A panic leaves no trace a user could send **(derived)**. On Linux and macOS it reaches a terminal
only if parterre was started from one.

**A panic on the main thread ends the app.** eframe has no panic handling of its own (none in
`eframe-0.36.2/src/native/`, **tested**). winit catches a panic inside its callbacks on Windows
and macOS, stops the event loop, and resumes the unwind after it
([windows/event_loop.rs#L423-L426](https://github.com/rust-windowing/winit/blob/v0.30.13/src/platform_impl/windows/event_loop.rs#L423-L426),
[macos/event_loop.rs#L439-L450](https://github.com/rust-windowing/winit/blob/v0.30.13/src/platform_impl/macos/event_loop.rs#L439-L450)),
because unwinding through the OS's callback frames isn't allowed. Either way the unwind leaves
`eframe::run_native` and `main`
([main.rs#L384-L401](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/crates/parterre/src/main.rs#L384-L401)),
and the window disappears. The panic hook runs at the panic site, before any of that ("invoked
when a thread panics, but before the panic runtime is invoked",
[set_hook](https://doc.rust-lang.org/std/panic/fn.set_hook.html)), so a hook sees the real
backtrace even when winit re-raises the panic later **(derived)**. eframe's `save` won't run, so
settings changed in that session are lost **(derived)**.

**A panic on a worker thread is silent, and its symptom depends on the receiver.** parterre runs
git, layouts, diffs and blame on plain `std::thread::spawn` threads and returns results over
`mpsc` channels. A panicking thread just ends; its `JoinHandle` would return `Err`
([std::thread::spawn](https://doc.rust-lang.org/std/thread/fn.spawn.html)), and the channel's
sender is dropped.
- The branches window treats a dropped sender as an error and says "Branch information worker
  stopped unexpectedly."
  ([branches.rs#L1555-L1561](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/crates/parterre/src/app/branches.rs#L1555-L1561)).
- The layout job treats it like "not ready yet"
  ([app.rs#L517-L521](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/crates/parterre/src/app.rs#L517-L521),
  [#L537-L539](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/crates/parterre/src/app.rs#L537-L539)),
  so the status bar keeps showing the spinner and "Laying out…"
  ([#L1448-L1451](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/crates/parterre/src/app.rs#L1448-L1451))
  forever **(derived)**.

A process-wide panic hook catches worker panics as well as main-thread ones, which is the main
reason to have one even where the app survives **(derived)**. The threads are unnamed, except
`system-theme`
([system_theme.rs#L143](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/crates/parterre/src/system_theme.rs#L143)),
so a report could only say `<unnamed>` for the rest.

**Release artefacts** ship only the executable: the archives copy `parterre.exe` or `parterre`
([release.yml#L86-L92](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/.github/workflows/release.yml#L86-L92)),
so the `.pdb` the Windows build writes (§3) is thrown away.

**Opening a URL** already exists for pull requests: `browser::open` runs the platform's opener
for `https://github.com/` URLs made of `[A-Za-z0-9-._~/:?#=&%]`
([browser.rs#L35-L40](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/crates/parterre/src/browser.rs#L35-L40)),
which a fully percent-encoded issue URL satisfies **(derived)**.

## 2. Rust panics

### 2.1 The hook

`std::panic::set_hook` replaces the process-wide hook; `take_hook` gets the previous one so a new
hook can chain to it ([set_hook](https://doc.rust-lang.org/std/panic/fn.set_hook.html)). In the
hook, `PanicHookInfo` gives the payload (the message) and `location()`, and
`std::backtrace::Backtrace::force_capture()` captures a backtrace "regardless of environment
variable" `RUST_BACKTRACE`
([Backtrace](https://doc.rust-lang.org/std/backtrace/struct.Backtrace.html#method.force_capture)).
`std::thread::current().name()` gives the thread.

What the Rust apps in §8 do in theirs:
- **Write a file:** Neovide appends to `neovide_backtraces.log`, Lapce logs through `tracing` to
  rotating log files, WezTerm to its log, GitButler keeps the panic per thread.
- **Tell the user:** Lapce and Alacritty show a Windows `MessageBoxW`, WezTerm a toast, Ruffle an
  `rfd` Yes/No dialog that opens a pre-filled GitHub issue.
- **Send it:** Zed hands the panic to its crash helper and then calls `abort()`, so each panic
  also becomes a minidump; rerun sends an analytics event without the message.
- **Redact:** Zed strips string-slicing panic text, because it can contain the user's text;
  rerun cuts paths down to `crate/src/...` and drops frames below `run_native_app`.

parterre already links `rfd` 0.17
([Cargo.toml](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/Cargo.toml)),
so a message box from the hook would add no dependency, as Ruffle does **(derived)**. Showing UI
from inside a panic hook on the main thread is fragile, though: the event loop is mid-callback.
Writing the file in the hook and offering it on the next start avoids that, and also covers
native crashes that leave a file behind (§4) **(derived)**.

### 2.2 `human-panic` and similar crates

[`human-panic`](https://github.com/rust-cli/human-panic) 2.0.8 (2026-04-02, 13.9M downloads) is
the common crate:
- Only in release builds and only when `RUST_BACKTRACE` is unset, it installs a hook that writes
  a TOML report `report-<uuid>.toml` to the temp directory, holding name, version, OS and arch
  (through `sysinfo`), the message, `file:line` and a backtrace (through the `backtrace` crate),
  and prints "We have generated a report file at …" to stderr
  ([src/panic.rs#L9-L66](https://github.com/rust-cli/human-panic/blob/v2.0.8/src/panic.rs#L9-L66),
  [src/report.rs#L41-L110](https://github.com/rust-cli/human-panic/blob/v2.0.8/src/report.rs#L41-L110)).
- It is built for CLI tools. For parterre the stderr message is invisible on Windows (§1), the
  temp directory is cleaned by the OS, and it doesn't chain to a previous hook or offer a
  GitHub URL. It adds `sysinfo`, `toml`, `uuid` and `backtrace`
  ([Cargo.toml](https://github.com/rust-cli/human-panic/blob/v2.0.8/Cargo.toml)). The hook it
  shows is about 30 lines, so writing parterre's own is no harder **(derived)**.

Others: [`better-panic`](https://github.com/mitsuhiko/better-panic) 0.3.0 (last release
2022-01) prints prettier backtraces to the terminal;
[`color-eyre`](https://github.com/eyre-rs/eyre) 0.6.5 installs a coloured panic and error report
hook with an optional issue-URL section (terminal-oriented) **(unverified for the URL part)**.
None of them helps a GUI app on Windows.

### 2.3 What a report should hold

From the apps in §8 and what parterre needs **(derived)**:
- `parterre --version` output, which is the version and commit
  ([release.yml#L65-L68](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/.github/workflows/release.yml#L65-L68)),
  plus target triple and install channel if known (MSI, winget, Snap, …).
- OS and version, and the git version parterre runs (git support is #166).
- The panic message, `file:line`, thread name and a trimmed backtrace (Ruffle cuts its panic
  text to 2,048 characters at a line boundary,
  [main.rs#L105](https://github.com/ruffle-rs/ruffle/blob/8cbada89a8f49f53f81b0fdfd1be9dcbf2741211/desktop/src/main.rs#L105)).
- For GPU crashes, the GL vendor and renderer strings, as Zed records GPUs.
- Not: repository paths, branch names, file names or commit text, which a git viewer's panic
  message or backtrace may contain. Zed and rerun both redact; a user-reviewed issue lets the
  user see and edit what goes out.

### 2.4 A pre-filled GitHub issue

GitHub opens a new-issue page from query parameters `title`, `body`, `labels`, `assignees`,
`template` and the field ids of issue-form templates. "If you create a URL that exceeds the
server limit, the URL will return a `414 URI Too Long` error page"
([GitHub docs](https://github.com/github/docs/blob/d9fd377ea956f59385ac8cb2034ba5c613b6fd85/content/issues/tracking-your-work-with-issues/using-issues/creating-an-issue.md#L124-L144)).
The docs give no number, so I measured it **(tested 2026-10-04: `curl -s -o /dev/null -w
'%{http_code}' 'https://github.com/aquamoth/parterre/issues/new?title=t&body=aaa…'`, not signed
in)**:

| URL length | Response |
|---|---|
| up to 6,861 | 302 to the login page |
| 7,061 and 7,561 | 500 |
| 8,061 | connection dropped |
| 8,261 and longer | 414 |

A signed-out user is sent to `/login?return_to=<the URL, percent-encoded again>`. Every `%` in
the URL becomes `%25`, so a body full of encoded newlines grows on the way:
- a 3,061-character URL with 1,000 encoded newlines reached the login page as 5,119 characters
  and loaded (200);
- a 4,561-character URL with 1,500 reached it as 7,619 and failed (500).

So: keep the whole URL, percent-encoded, under about 4,000 characters, which survives the
sign-in round trip **(derived from the tests)**. That is the body of a short report with about
2 KB of backtrace, much like Ruffle's. Anything longer, and every minidump, has to be attached by
hand: the issue can ask the user to drag in the report file, whose path the app shows.

Practical notes **(derived)**:
- The user needs a GitHub account and submits the issue themselves, so it is consent per crash,
  and GitHub, not parterre, is where the data goes.
- An issue-form template (`.github/ISSUE_TEMPLATE/crash.yml`, which parterre doesn't have yet)
  gives fixed fields and a `crash` label; Ruffle uses `template=crash_report.yml`
  ([main.rs#L136](https://github.com/ruffle-rs/ruffle/blob/8cbada89a8f49f53f81b0fdfd1be9dcbf2741211/desktop/src/main.rs#L136)).
- On Windows the opener is `explorer.exe <url>`. `CreateProcess` allows a 32,767-character
  command line
  ([CreateProcessW](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw)),
  but whether Explorer passes a 4 KB URL to the browser intact is **(unverified)**; test it before
  relying on it.
- Duplicates pile up as separate issues; the maintainer closes them by hand.

## 3. Backtraces from release builds

What each setting keeps:
- `debug`: `false`/`none` (release default) has no debug info; `"line-tables-only"` is "the
  minimal amount of debug info for backtraces with filename/line number info"; `"limited"` adds
  module-level info; `"full"` everything
  ([Cargo profiles](https://github.com/rust-lang/cargo/blob/76488151ad6133284397491892dabb50a86f5054/doc/book/src/reference/profiles.md#L63-L84)).
- `strip`: `"debuginfo"` "should leave backtraces mostly-intact"; `"symbols"` also strips the
  symbol table, which "can affect them so negatively as to make the trace incomprehensible.
  Programs which may be combined with others, such as CLI pipelines and developer tooling, or
  even anything which wants crash-reporting, should usually avoid `-Cstrip=symbols`"
  ([rustc codegen options](https://github.com/rust-lang/rust/blob/7ca25f1de518f33b79535a0fc0ce3f5fd1685ed7/src/doc/rustc/src/codegen-options/index.md#L683-L707)).
- `split-debuginfo`: `packed` (default on Windows MSVC and macOS) puts debug info in a separate
  `.pdb`, `.dSYM` or, on Linux, `.dwp` file; `off` (default for ELF) keeps it in the binary. All
  three values work on Linux and Apple; MSVC supports only `packed`
  ([rustc](https://github.com/rust-lang/rust/blob/7ca25f1de518f33b79535a0fc0ce3f5fd1685ed7/src/doc/rustc/src/codegen-options/index.md#L653-L681)).
- **On Windows `strip` does nothing:** rustc always links MSVC binaries with `/DEBUG`, which "will
  cause the Microsoft linker to generate a PDB file", and ignores the strip value
  ([linker.rs#L1083-L1095](https://github.com/rust-lang/rust/blob/4613f38cc5a7d50ab76bf93f1f3b76fdf0d89764/compiler/rustc_codegen_ssa/src/back/linker.rs#L1083-L1095),
  called unconditionally from
  [link.rs#L3344](https://github.com/rust-lang/rust/blob/538a927606760caf1be3376b0a60852769c96565/compiler/rustc_codegen_ssa/src/back/link.rs#L3344)).
  With `debug = false` that PDB has function names but no line tables **(derived)**. The
  backtrace code finds it next to the `.exe`; parterre doesn't ship it (§1).

So for parterre's current profile **(derived)**:

| Platform | Panic message and `file:line` | Backtrace frames |
|---|---|---|
| Linux, macOS | yes (compiled into the panic site, independent of debug info) | addresses only, `<unknown>` names |
| Windows | yes | addresses only, unless `parterre.pdb` sits next to the exe |

The panic location is often enough for `unwrap`/`expect` panics, since those report their
caller **(derived)**. For the rest, the cheapest improvements, none measured here:
1. `strip = "debuginfo"` (or `strip = "none"` with `debug = false`): function names on Linux and
   macOS, at the cost of the symbol table, typically a few percent of the binary **(unverified,
   not measured)**. Alacritty ships `debug = 1`; Zed `debug = "limited"`, uploaded then stripped;
   Neovide `strip = true` and gets unsymbolised traces (§8).
2. `debug = "line-tables-only"` kept in the binary: `file:line` per frame, a larger binary
   **(size not measured)**.
3. Split debug info kept out of the download: `split-debuginfo = "packed"` (or `objcopy
   --only-keep-debug` on Linux, `dsymutil` on macOS, the existing `.pdb` on Windows), stored per
   release, and the binary shipped stripped. Then the report must carry raw addresses and the
   build id, and someone symbolicates later, by hand or on a server (§5).

## 4. Native crashes

A segfault, illegal instruction, stack overflow, abort or Windows exception (a GPU driver bug,
`glow`/winit FFI, a C library) never runs the panic hook. Catching one in-process is unsafe:
the process is in an unknown state, so the robust pattern is to have another process write a
minidump of it. rerun's in-process signal handler admits it can deadlock (§8).

### 4.1 Embark's `crash-handler` + `minidumper`

The maintained Rust stack, from [EmbarkStudios/crash-handling](https://github.com/EmbarkStudios/crash-handling)
(not archived, pushed 2026-09-25):

| Crate | Latest | Role |
|---|---|---|
| [`crash-handler`](https://crates.io/crates/crash-handler) | 0.8.1, 2026-09-25 | in the app: signal handlers on Linux (and a `pthread_create` hook so every thread has an alternate stack for stack overflows), exception handlers on Windows, Mach exception ports on macOS ([README](https://github.com/EmbarkStudios/crash-handling/blob/main/crash-handler/README.md)) |
| [`minidumper`](https://crates.io/crates/minidumper) | 0.11.0, 2026-07-20 | IPC client in the app, server in the monitor process that writes the dump |
| [`minidump-writer`](https://github.com/rust-minidump/minidump-writer) | 0.13.0, 2026-07-20 | writes the minidump of another process (ptrace on Linux) |
| [`minidumper-child`](https://github.com/timfish/minidumper-child) | 0.5.0, 2026-07-06 | packages the monitor: "spawns the current executable again with an argument that causes it to start in crash reporter mode" ([README](https://github.com/timfish/minidumper-child/blob/main/README.md)) |

What it takes **(derived from the READMEs and Zed)**:
- **A monitor process.** parterre's own binary, started again with a hidden argument at launch,
  stays running beside the app and writes `<id>.dmp` when the app crashes. Zed does exactly this
  (`zed --crash-handler <socket>`, §8). It works the same in Snap and Flatpak since it's the same
  executable **(unverified)**.
- **Linux ptrace permission.** Yama's ptrace scope stops one process reading another unless
  allowed; `minidumper-child` calls `crash-handler`'s `set_ptracer(Some(server_pid))`
  ([`CrashHandler::set_ptracer`](https://docs.rs/crash-handler/0.8.1/crash_handler/struct.CrashHandler.html#method.set_ptracer),
  called from `minidumper-child`'s `client.rs`).
- **`unsafe`.** Installing the handler goes through `crash_handler::make_crash_event`, an
  `unsafe fn` ([docs.rs](https://docs.rs/crash-handler/0.8.1/crash_handler/fn.make_crash_event.html)).
  parterre denies `unsafe_code` workspace-wide, with one local exception
  ([Cargo.toml#L64-L67](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/Cargo.toml#L62-L65));
  `minidumper-child` or `sentry-minidump` keep the `unsafe` inside their own crates.
- **macOS signals.** Mach exception ports take precedence over signal handlers, so "if you use
  this crate in conjunction with signal handling on MacOS, you will not get the results you
  expect" ([README](https://github.com/EmbarkStudios/crash-handling/blob/main/crash-handler/README.md)).
- **Dependencies:** `crash-handler` needs `crash-context`, `libc`, `mach2` (macOS) and
  `parking_lot`; `minidumper` adds `minidump-writer` (with `goblin`, `scroll`, `procfs-core`,
  `memmap2`, `serde_json`), `polling` and `uds`
  ([crates.io dependencies, tested 2026-10-04](https://crates.io/crates/minidumper/0.11.0/dependencies)).
  All pure Rust, no C++ build.
- **Size:** no published figure, and I didn't build. Judging by the dependency list it is in the
  hundreds of kilobytes, far below the ~2 MB that `ureq` + rustls cost parterre
  ([Cargo.toml#L46-L51](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/Cargo.toml#L49-L54))
  **(unverified, not measured)**.
- **Then the dump has to go somewhere**, and is only readable with the matching symbols (§5).
  Minidumps "might contain sensitive information … such as environment variables, local
  pathnames or maybe even in-memory representations of input fields"
  ([Sentry minidumps](https://docs.sentry.io/platforms/native/guides/minidumps/)), so they are
  personal data in GDPR terms **(derived)**.

### 4.2 `sentry-minidump` (sentry-rust 0.49.3)

New in sentry-rust 0.49.3 (2026-09-21), behind the `minidump` feature: it "captures native
crashes as minidumps in a separate process and sends them to Sentry as attachments". All the work
happens inside `sentry::init`: in the app it spawns the crash reporter, and in the reporter
process `init` "never returns". "Code before `sentry::init` runs in both processes, because the
crash reporter re-executes the current binary." Scope changes (user, tags) must be sent across
explicitly. Linux, macOS and Windows only
([README](https://github.com/getsentry/sentry-rust/blob/0.49.3/sentry-minidump/README.md)). It is
built on `minidumper-child` 0.5. The older third-party
[`sentry-rust-minidump`](https://github.com/timfish/sentry-rust-minidump) 0.18.1 (2026-09-28,
593k downloads) does the same and has the longer track record; `sentry-minidump` itself has
under 3,000 downloads.

For parterre it would mean:
- `main` must call `sentry::init` before parsing arguments or opening a window, since everything
  before it runs in the reporter process too **(derived)**.
- The transport: sentry's `ureq` feature uses ureq 3, which parterre already has, but sentry's
  `rustls` and `rustls-no-provider` features both turn on `ureq?/rustls`
  ([sentry/Cargo.toml#L60-L68](https://github.com/getsentry/sentry-rust/blob/0.49.3/sentry/Cargo.toml#L60-L68)),
  which pulls in `webpki-roots`, the licence `packaging/about.toml` rejects. Enable `ureq` alone
  and hand sentry parterre's own configured `ureq::Agent` through
  `UreqHttpTransport::with_agent`
  ([transports/ureq.rs#L71](https://github.com/getsentry/sentry-rust/blob/0.49.3/sentry/src/transports/ureq.rs#L71)) **(derived, not built)**.
- Default features include `release-health`, which sends sessions, i.e. usage data
  ([sentry/Cargo.toml#L24](https://github.com/getsentry/sentry-rust/blob/0.49.3/sentry/Cargo.toml#L24-L33),
  [Releases & Health](https://docs.sentry.io/platforms/rust/configuration/releases/)). Turn off
  default features and pick `panic`, `backtrace`, `contexts`, `minidump`, `ureq` deliberately,
  so crash reporting doesn't quietly become usage tracking.
- The DSN is compiled in. "DSNs are safe to keep public because they only allow submission of
  new events" ([DSN explainer](https://docs.sentry.io/concepts/key-terms/dsn-explainer/)); abuse
  is bounded by the free plan's quota, after which events are dropped (§6).

### 4.3 Crashpad and Breakpad bindings

- [`crashpad-rs`](https://github.com/bahamoth/crashpad-rs) 0.2.7 (2025-08-28, 2 stars, 3.8k
  downloads): wraps Google's C++ Crashpad and ships a separate `crashpad_handler` executable,
  with a build-time bundler. A C++ build and a second binary in every package **(derived from the
  README)**.
- [`crashpad`](https://crates.io/crates/crashpad) 0.1.2 (2021): abandoned.
- [`breakpad-handler`](https://crates.io/crates/breakpad-handler) 0.2.0 (2023-05) from Embark's
  `sentry-contrib-rust`, which is archived; Embark replaced it with `crash-handling`.
- `sentry-native` (C SDK with a Crashpad backend) is what Sentry recommends for C/C++; from Rust
  it means a CMake build **(derived)**.

None of them is better than §4.1 for parterre.

## 5. Symbols

**Upload per release.** Zed's CI runs `sentry-cli debug-files upload --include-sources` on every
platform and then strips what ships: the unstripped ELF then `llvm-objcopy --strip-debug` on
Linux, `dsymutil` then `strip -x` on macOS, the PDBs on Windows (§8). Sentry's docs: "Debug files
should be uploaded before deploying or releasing your application so that crash reports can be
processed"; the CLI recursively scans folders and skips files already uploaded
([upload](https://docs.sentry.io/platforms/native/data-management/debug-files/upload/),
[sentry-cli dif](https://docs.sentry.io/cli/dif/)). `--include-sources` must run on the build
machine. For parterre this is one step per job in
[release.yml](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/.github/workflows/release.yml)
with a `SENTRY_AUTH_TOKEN` secret **(derived)**.

**Retention.** Debug files on Sentry use "time to idle": a file not used to process an event for
90 days expires ([debug files](https://docs.sentry.io/platforms/native/data-management/debug-files/#retention-policy)).
An old release that nobody runs loses its symbols, which doesn't matter because it also sends no
crashes **(derived)**.

**Free-tier symbol servers.** Sentry can also fetch from symbol servers, but custom ones
(HTTP, S3, GCS) "require a *Business* or *Enterprise* plan"; only built-in ones (Microsoft, iOS)
are free ([symbol servers](https://docs.sentry.io/platforms/native/data-management/debug-files/symbol-servers/)).
So on the free plan, GitHub release assets can't serve as Sentry's symbol store; files must be
uploaded **(derived)**.

**GitHub release assets** as the store for the maintainer's own use: up to 1,000 assets per
release, each under 2 GiB, and "no limit on the total size of a release, nor bandwidth usage"
([about releases](https://github.com/github/docs/blob/6c39b7b7dee4ce3fdcabe563f6bb8e3b15b342ac/content/repositories/releasing-projects-on-github/about-releases.md#L49)).
Attaching `parterre-<version>-<target>.debug.zip` (`.pdb`, `.dSYM`, `.debug`) costs nothing, and
WezTerm ships `wezterm.pdb` inside its Windows zip (§8). With these the maintainer can
symbolicate a minidump locally with `minidump-stackwalk`
([rust-minidump](https://github.com/rust-minidump/rust-minidump), 0.27.0, 2026-08-11) **(derived)**.

**Rust panics through sentry-rust** are symbolicated on the client: `sentry-backtrace` calls
`backtrace::Backtrace::new()`, which resolves names in the process
([sentry-backtrace lib.rs#L24](https://github.com/getsentry/sentry-rust/blob/0.49.3/sentry-backtrace/src/lib.rs#L24)),
so they read as well as the binary's symbols allow (§3). With stripped binaries and uploaded
debug files, the `debug-images` feature lets the server do it instead; Sentry's Rust docs warn
not to enable `debug-images` when debug info stays in the binary
([Sentry for Rust](https://docs.sentry.io/platforms/rust/)).

**Elsewhere.** GlitchTip has `glitchtip-cli debug-files upload` for dSYM, PDB and ELF, but shows
one frame per thread for native crashes today; Bugsink's minidump support is experimental (§6).
So of the free options, only Sentry walks a Rust minidump's stack server-side.

## 6. Hosted crash services with a free tier

Checked against each service's own pricing page, docs or repo on 2026-10-04.

**Sentry** ([pricing](https://sentry.io/pricing/)):
- Free Developer plan: 5,000 errors a month, one user, 30-day retention, 1 GB attachments.
- Over quota, data "will be dropped and you won't be charged for it"
  ([docs/pricing](https://docs.sentry.io/pricing/)). Per-project rate limits can be set
  ([quotas](https://docs.sentry.io/pricing/quotas/)).
- EU region (Frankfurt) on all plans including the free one, chosen when the organization is
  created and never changeable
  ([changelog](https://sentry.io/changelog/data-storage-location-in-germany-is-generally-available/),
  [data storage location](https://docs.sentry.io/organization/data-storage-location/)).
- DPA 5.1.0, accepted in-product, SCCs as fallback ([DPA](https://sentry.io/legal/dpa/),
  [help](https://www.sentry.help/en/articles/13965008-how-do-i-sign-your-data-processing-addendum)).
- Sponsored open-source plan with Business features (5M errors, 10 GB attachments); the page
  asks for "a friendly license like Apache or MIT"
  ([for/open-source](https://sentry.io/for/open-source/)). Whether GPL-3.0 parterre qualifies is
  **(unverified)**.
- Minidumps: 40 MB compressed per request, 200 MB decompressed; processed and then "removed
  immediately" unless stored as attachments
  ([minidumps](https://docs.sentry.io/platforms/native/guides/minidumps/)). Whether processed
  dumps count against the 1 GB attachment quota is **(unverified)**.
- The SDK "purposefully does not send PII" (`send_default_pii`), and `before_send` can scrub an
  event before it leaves the machine
  ([sensitive data](https://docs.sentry.io/platforms/rust/data-management/sensitive-data/)).

**GlitchTip** (Sentry-protocol server, MIT backend):
- Hosted free: 1,000 events a month, unlimited projects and members, EU hosting on all plans
  ([pricing](https://glitchtip.com/pricing)); events purged after 90 days, the EU instance on
  DigitalOcean Frankfurt ([hosted architecture](https://glitchtip.com/documentation/hosted-architecture/)).
- Self-hosted: PostgreSQL plus one service, 512 MB RAM recommended
  ([install](https://glitchtip.com/documentation/install/)), free software but a server to run
  and pay for.
- Minidumps are immature: a native crash shows "one frame per thread"
  ([#511](https://gitlab.com/glitchtip/glitchtip-backend/-/work_items/511)), and a minidump
  attached to an envelope, which is how `sentry-minidump` sends it, is ignored today
  ([draft MR !2585](https://gitlab.com/glitchtip/glitchtip-backend/-/merge_requests/2585)).

**Bugsink** (Sentry-SDK compatible):
- Hosted "15K / Evaluation" plan: 15K events a month, one user, at most 5K events kept; whether
  "Evaluation" is permanent is **(unverified)**. Self-hosting is free, a single Docker container
  on SQLite, MySQL or PostgreSQL ([pricing](https://www.bugsink.com/#pricing),
  [install](https://www.bugsink.com/docs/installation/)).
- Hosted data "stored and processed within the European Union"
  ([privacy policy](https://www.bugsink.com/privacy-policy/)).
- Licence PolyForm Shield 1.0.0, source-available, not OSI ([repo](https://github.com/bugsink/bugsink)).
- Minidumps experimental since 2.0.7 behind `FEATURE_MINIDUMPS`, "has not passed
  security-review yet" ([CHANGELOG](https://github.com/bugsink/bugsink/blob/main/CHANGELOG.md)).

**Others**, briefly:

| Service | Free tier | EU on free | Sentry SDK | Minidumps |
|---|---|---|---|---|
| [PostHog error tracking](https://posthog.com/pricing) | 100K exceptions/month, 1-year retention | yes (Frankfurt) | no; `posthog-rs` captures panics ([docs](https://posthog.com/docs/error-tracking/installation/rust)) | no |
| [Backtrace (Sauce Labs)](https://backtrace.io/pricing) | 25K errors, 1 month | (unverified) | (unverified) | yes |
| [BugSplat](https://www.bugsplat.com/pricing/) | 15K crashes, 1 month, 3 users | (unverified) | no | yes (Crashpad/Breakpad) |
| [BugSnag](https://www.bugsnag.com/pricing/) | 7.5K events, 7 days | not stated | no | not stated |
| [Rollbar](https://rollbar.com/pricing) | 5K occurrences, 30 days | no ([GDPR](https://docs.rollbar.com/docs/gdpr-rollbar)) | no | (unverified) |
| [Honeybadger](https://www.honeybadger.io/plans/) | 5K errors, 15 days | no (Business only) | no | no |
| [Better Stack](https://betterstack.com/pricing) | 100K exceptions, 90 days | (unverified) | yes | not stated |
| [Raygun](https://raygun.com/pricing), [Telebugs](https://telebugs.com/pricing) | none | | | |
| [Socorro](https://github.com/mozilla-services/socorro) | self-host, MPL-2.0, "no capacity to support non-Mozilla uses" | | no | yes |

PostHog is also a candidate for the usage-counting side of #175, and would take panics
(not minidumps) in the same account **(derived)**.

## 7. Hangs and freezes

- **Zed** runs a `HangDetection` OS thread that checks once a second and flags main-thread
  stalls over 100 ms and frames over 24 ms in release builds; incidents go out as a telemetry
  event every 30 minutes and on quit, under the `metrics` setting rather than `diagnostics`
  ([hang_detection.rs#L31-L60](https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/crates/zed/src/reliability/hang_detection.rs#L31-L60),
  [#L101](https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/crates/zed/src/reliability/hang_detection.rs#L101)).
- **Firefox**'s crash reporter docs say hangs can also produce crash reports
  ([index.md#L11](https://github.com/mozilla-firefox/firefox/blob/3f73c528a1ae5784ea5e1ee2c5ad3762507395f2/toolkit/crashreporter/docs/index.md#L11)).
- No other app in §8 detects hangs, and none of the crash crates do it on their own.

For parterre **(derived)**:
- A frozen main thread is not a crash, so no hook or minidump fires. A watchdog thread has to
  notice that the main thread stopped making progress, and can then record it. With §4.1 in
  place it could also ask for a dump of the live process: `crash-handler` has
  [`simulate_signal`](https://docs.rs/crash-handler/0.8.1/crash_handler/struct.CrashHandler.html#method.simulate_signal)
  (Linux) for raising its crash event without a crash, and a minidump holds every thread's
  stack, the frozen main thread's included **(unverified for this use)**.
- egui repaints only when something happens, so "no frame for N seconds" is normal when idle.
  The watchdog has to time work the main thread started (Zed's approach), or ask for a repaint
  and time the answer. A hidden or minimised window may legitimately not paint, especially on
  Wayland.
- That last case is the Wayland freeze of [#32](https://github.com/aquamoth/parterre/issues/32):
  a vsync'd swap of a hidden window blocked the whole app inside eframe (egui#5145), fixed by
  turning vsync off on Wayland
  ([main.rs#L356-L369](https://github.com/aquamoth/parterre/blob/21503e0b16adfdf72499f7be14312f556cfcb2ad/crates/parterre/src/main.rs#L356-L369)).
  A watchdog that measured "update started, paint not finished after 5 s" would have caught it,
  since the block was in the swap after `update` returned; one that only measured `update` would
  not.
- Cost: a thread, an atomic timestamp written each frame, and a policy for what to send. A hang
  report without a stack says only "it froze", so it is mostly useful as a count per version and
  platform, which fits the usage side of #175 better than the crash side.

## 8. Prior art: how Rust desktop apps report crashes

Snapshots: Zed v1.22.0 `76659a55`, Lapce v0.4.6 `b012cef4`, GitButler 0.22.3 `0ba35dc6`, WezTerm
main `cab25161`, Alacritty v0.17.0 `94e7c887`, rerun 0.38.1 `b08c599e`, Helix 25.07.1 `a05c151b`,
Neovide 0.16.2 `d2d6ebd4`, Ruffle master `8cbada89`, Firefox main `3f73c528`.

**Zed** is the only one with a full Rust-native pipeline:
- The `crashes` crate uses `crash-handler` 0.8 and `minidumper` 0.11
  ([Cargo.toml#L10-L12](https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/crates/crashes/Cargo.toml#L10-L12)).
  Zed relaunches itself as `zed --crash-handler <socket>`
  ([main.rs#L222-L224](https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/crates/zed/src/main.rs#L222-L224)),
  installs handlers that ask the helper for a dump, and pings it every 10 s
  ([crashes.rs#L94-L177](https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/crates/crashes/src/crashes.rs#L94-L177)).
- The helper writes a zstd-compressed `<session>.dmp` and a `<session>.json` with version, commit,
  panic, GPUs and glibc's `__abort_msg`
  ([crashes.rs#L423-L499](https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/crates/crashes/src/crashes.rs#L423-L499)).
- The panic hook sends the message and location to the helper, then `abort()`s so every panic
  becomes a minidump; string-slicing panic text is redacted first
  ([crashes.rs#L546-L599](https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/crates/crashes/src/crashes.rs#L546-L599)).
- On the next launch the pairs are posted to a Sentry minidump endpoint compiled in from a CI
  secret ([reliability.rs#L259-L447](https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/crates/zed/src/reliability.rs#L259-L447),
  [release_nightly.yml#L105](https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/.github/workflows/release_nightly.yml#L105)).
- Opt-out: `"telemetry": {"diagnostics": true}` by default
  ([default.json#L1675-L1683](https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/assets/settings/default.json#L1675-L1683),
  [telemetry.md#L27-L42](https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/docs/src/telemetry.md#L27-L42)).
- Symbols: `debug = "limited"`
  ([Cargo.toml#L1108-L1111](https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/Cargo.toml#L1108-L1111)),
  uploaded with `sentry-cli debug-files upload --include-sources`, then stripped
  ([bundle-linux#L90-L129](https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/script/bundle-linux#L90-L129),
  [bundle-mac#L305-L352](https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/script/bundle-mac#L305-L352),
  [bundle-windows.ps1#L159-L183](https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/script/bundle-windows.ps1#L159-L183)).
- Hangs: the watchdog in §7.

**rerun**: a panic hook prints an anonymised backtrace and sends a `crash-panic` analytics event
with `message: None`, then exits with code 102
([re_crash_handler lib.rs#L28-L84](https://github.com/rerun-io/rerun/blob/b08c599e934b0dedee1e95fd74a989a1582ce3d5/crates/utils/re_crash_handler/src/lib.rs#L28-L84),
[#L198-L303](https://github.com/rerun-io/rerun/blob/b08c599e934b0dedee1e95fd74a989a1582ce3d5/crates/utils/re_crash_handler/src/lib.rs#L198-L303)).
A Unix-only `libc::signal` handler captures a backtrace in the handler, which "can deadlock",
and sends `crash-signal`
([#L86-L185](https://github.com/rerun-io/rerun/blob/b08c599e934b0dedee1e95fd74a989a1582ce3d5/crates/utils/re_crash_handler/src/lib.rs#L86-L185)).
Events go to PostHog through `tel.rerun.io`
([sink.rs#L11](https://github.com/rerun-io/rerun/blob/b08c599e934b0dedee1e95fd74a989a1582ce3d5/crates/utils/re_analytics/src/native/sink.rs#L11)),
opt-out ([config.rs#L66](https://github.com/rerun-io/rerun/blob/b08c599e934b0dedee1e95fd74a989a1582ce3d5/crates/utils/re_analytics/src/native/config.rs#L66)),
off in CI and debug builds. Release uses `panic = "abort"` and no debug info
([Cargo.toml#L657-L661](https://github.com/rerun-io/rerun/blob/b08c599e934b0dedee1e95fd74a989a1582ce3d5/Cargo.toml#L657-L661)).

**GitButler** (Tauri) removed Rust-side Sentry on 2024-04-29: "the actually valuable bug reports
are submitted by developers as github issues or on our Discord"
([commit 83b40eab](https://github.com/gitbutlerapp/gitbutler/commit/83b40eabb627a2397696dc71d38e3f30a5e9362b)).
Today a panic hook records each thread's panic
([panic_capture.rs#L87-L113](https://github.com/gitbutlerapp/gitbutler/blob/0ba35dc675c0d0eebcf2e1d5e114b73d46019942/crates/but-api/src/panic_capture.rs#L87-L113))
and every API command runs in `catch_unwind`, so a backend panic reaches the UI as an error
([but-api-macros lib.rs#L133-L144](https://github.com/gitbutlerapp/gitbutler/blob/0ba35dc675c0d0eebcf2e1d5e114b73d46019942/crates/but-api-macros/src/lib.rs#L133-L144)).
The web frontend reports to Sentry, opt-out with a toggle at onboarding
([defaults.jsonc#L12](https://github.com/gitbutlerapp/gitbutler/blob/0ba35dc675c0d0eebcf2e1d5e114b73d46019942/crates/but-settings/assets/defaults.jsonc#L12),
[AnalyticsSettings.svelte#L30-L45](https://github.com/gitbutlerapp/gitbutler/blob/0ba35dc675c0d0eebcf2e1d5e114b73d46019942/apps/desktop/src/components/shared/AnalyticsSettings.svelte#L30-L45)).

**Ruffle desktop**: the panic hook shows an `rfd` Yes/No dialog; Yes opens a GitHub issue with
`template=crash_report.yml`, the panic text and stack cut to 2,048 characters, OS, version and
renderer ([main.rs#L79-L149](https://github.com/ruffle-rs/ruffle/blob/8cbada89a8f49f53f81b0fdfd1be9dcbf2741211/desktop/src/main.rs#L79-L149)).
Release uses `panic = "abort"`
([Cargo.toml#L164-L165](https://github.com/ruffle-rs/ruffle/blob/8cbada89a8f49f53f81b0fdfd1be9dcbf2741211/Cargo.toml#L164-L165)).

**Local only:**
- Lapce: log files with backtrace, Windows message box
  ([logging.rs#L66-L100](https://github.com/lapce/lapce/blob/b012cef466741528f338879fa708355217fc40bd/lapce-app/src/app/logging.rs#L66-L100)).
- WezTerm: log plus a "Wezterm panic" toast
  ([main.rs#L801-L809](https://github.com/wezterm/wezterm/blob/cab25161054c50fd6c705db4ceefef0f1e5a9575/wezterm-gui/src/main.rs#L801-L809));
  ships `wezterm.pdb` in the Windows zip
  ([ci/deploy.sh#L119](https://github.com/wezterm/wezterm/blob/cab25161054c50fd6c705db4ceefef0f1e5a9575/ci/deploy.sh#L119)).
- Alacritty: Windows message box only
  ([panic.rs#L10-L25](https://github.com/alacritty/alacritty/blob/94e7c8874e526b1e67b349d9ba30ddf81669119e/alacritty/src/panic.rs#L10-L25));
  release `debug = 1`
  ([Cargo.toml#L14-L17](https://github.com/alacritty/alacritty/blob/94e7c8874e526b1e67b349d9ba30ddf81669119e/Cargo.toml#L14-L17)).
- Neovide: appends to `neovide_backtraces.log`
  ([main.rs#L411-L441](https://github.com/neovide/neovide/blob/d2d6ebd4ee6b8b645c9b9a3b0d5d30923509b2c0/src/main.rs#L411-L441));
  release `strip = true`, so mostly unsymbolised
  ([Cargo.toml#L209-L218](https://github.com/neovide/neovide/blob/d2d6ebd4ee6b8b645c9b9a3b0d5d30923509b2c0/Cargo.toml#L209-L218)).
- Helix: restores the terminal, nothing else
  ([application.rs#L1123-L1131](https://github.com/helix-editor/helix/blob/a05c151bb6e8e9c65ec390b0ae2afe7a5efd619b/helix-term/src/application.rs#L1123-L1131)).

**Firefox's Rust crash reporter** is a separate executable started after a crash with the dump
path; it adds stack traces with a minidump analyzer and asks the user before submitting to
Mozilla's Socorro; unsubmitted dumps are deleted
([client main.rs](https://github.com/mozilla-firefox/firefox/blob/3f73c528a1ae5784ea5e1ee2c5ad3762507395f2/toolkit/crashreporter/client/app/src/main.rs#L1-L33),
[docs](https://github.com/mozilla-firefox/firefox/blob/3f73c528a1ae5784ea5e1ee2c5ad3762507395f2/toolkit/crashreporter/docs/index.md#L76-L117)).

| App | Panic | Native | Upload | Consent | Symbols | Hangs |
|---|---|---|---|---|---|---|
| Zed | to helper, then `abort()`; redacted | `minidumper` + `crash-handler` | Sentry minidump endpoint | opt-out | `debug="limited"`, uploaded, stripped | watchdog |
| rerun | anonymised event, no message | Unix signal handler | PostHog | opt-out | none | no |
| GitButler | per thread + `catch_unwind` | none | Sentry (JS only) | opt-out, shown at onboarding | JS source maps | no |
| Ruffle | dialog, then GitHub issue URL | none | user files it | per crash | n/a | no |
| Lapce, WezTerm, Alacritty, Neovide, Helix | log, dialog or toast | none | none | n/a | mixed | no |
| Firefox | n/a | Breakpad + Rust helper | Socorro | per-crash dialog | Mozilla symbol server | yes |

## 9. Options for parterre on free tiers

All four sit behind a small crate the app owns (say `parterre-crash`, or part of a telemetry
facade), like `parterre-forge` and `parterre-highlight`: the app calls `install()` at the top of
`main` and asks "is there a crash report from last time?"; the backend's dependencies live only
in that crate's manifest **(derived)**. Every option starts with the same local hook, so A is the
base the others add to.

**A. Local file + reviewed GitHub issue.** The hook writes `crash-<time>.txt` to parterre's data
directory (message, location, thread, versions, OS, trimmed backtrace, with paths redacted). On
the next start parterre shows "parterre quit unexpectedly last time" with *Report on GitHub*
(opens the pre-filled issue, under 4,000 characters, §2.4) and *Show file*.
- Needs: about a hundred lines of Rust, an issue-form template, and `strip = "debuginfo"` (or
  line tables) for readable traces (§3).
- Costs: nothing. No service, no DPA, no data leaves without the user pressing Submit on GitHub.
- Misses: native crashes, and every user who doesn't have or won't use a GitHub account. Counts
  are not reliable. It is what Ruffle does, and what GitButler found most useful.

**B. A + sentry-rust for panics only.** Add `sentry` with `panic`, `backtrace`, `contexts` and
`ureq` (parterre's own agent, §4.2), and no `release-health`.
- Needs: a Sentry organization in the EU region, the DPA accepted, a DSN compiled in, a
  `before_send` that redacts paths and repository names, a Settings switch, and a line on the
  "what parterre sends" page. Opt-in or opt-out per the data-inventory ticket.
- Costs: free up to 5,000 errors a month with 30-day retention and one user (§6). Readable traces
  need symbols in the binary, or `debug-images` plus `sentry-cli debug-files upload` in CI.
- Gains: counts by version and platform, grouping, crashes from users without GitHub accounts.
- Misses: native crashes.

**C. B + minidumps (`sentry-minidump`).** Turn on the `minidump` feature.
- Needs: `sentry::init` first thing in `main` (the binary re-runs itself as the monitor), a
  `sentry-cli debug-files upload` step per release target, split debug info so the downloads stay
  small, and consent wording that covers memory dumps (they can hold paths and on-screen text).
- Costs: free on Sentry's plan within the same 5,000 errors and 1 GB attachments; a second
  parterre process running beside every window; a few hundred KB more binary **(unverified)**.
  The crate is two weeks old (2026-09-21); `sentry-rust-minidump` is the fallback.
- Gains: GPU-driver, glow and winit crashes, stack overflows and aborts, which nothing else
  catches. Only Zed among Rust desktop apps does this; it pays off when users report crashes that
  leave no panic.

**D. Self-hosted or hosted GlitchTip / Bugsink.** Same client as B (both speak the Sentry
protocol), a different DSN.
- Hosted free: GlitchTip 1,000 events/month, 90 days, EU; Bugsink 15K/month, keeps 5K, EU,
  "Evaluation". Self-hosted is free software but needs a server, which is not free; GlitchTip
  wants PostgreSQL, Bugsink runs in one container on SQLite.
- Minidumps (C) don't work usefully on either today (§5, §6), so D is a B-level option.
- Gains over B: a fully open server (GlitchTip, MIT), or more events (Bugsink), and a swap that
  touches only the DSN, which is the facade working as intended.

| | A: file + GitHub issue | B: + Sentry panics | C: + minidumps | D: GlitchTip / Bugsink |
|---|---|---|---|---|
| Rust panics | yes | yes | yes | yes |
| Native crashes | no | no | yes | no (minidumps immature) |
| Hangs | no (needs a watchdog, §7) | as events, if built | as dumps, if built | as events, if built |
| Reaches users without GitHub | no | yes | yes | yes |
| Consent | per crash, by the user on GitHub | setting (opt-in or opt-out) | setting, wider wording | setting |
| Third party | GitHub (user's own account) | Sentry, EU, DPA | Sentry, EU, DPA | GlitchTip/Bugsink EU, or none if self-hosted |
| Free limits | none | 5K errors/month, 30 days, 1 user | same, + 1 GB attachments | 1K/month (GlitchTip) or 15K/month, 5K kept (Bugsink); self-host: server cost |
| Symbols | in the binary (`strip = "debuginfo"` or line tables) | in the binary, or uploaded | uploaded per release (`sentry-cli`) | in the binary |
| New dependencies | none | `sentry` (+ reuses `ureq`) | + `minidumper-child`, `crash-handler`, `minidumper`, `minidump-writer` | as B |
| Work in CI | none | none, or upload step | upload step per target | none |
