# Writing the version into a built binary

Research note for [#400](https://github.com/aquamoth/parterre/issues/400), part of the map
[#390](https://github.com/aquamoth/parterre/issues/390): *every published build reuses the
canary binary of its commit, and the pipeline writes the published version into it afterwards,
without rebuilding. How is that done robustly on each platform?*

Sources: the Rust reference and standard library docs, the source of PostHog's symbolication
service (cymbal) at commit
[`3b515ee9fe`](https://github.com/PostHog/posthog/tree/3b515ee9fe5be28e6d00d493d8e9b8e90b586221)
(16 October 2026), `posthog-rs` 0.27.0 (the version parterre uses), the Go toolchain's source,
Electron's and Node's docs, `editpe` 0.2.4, and parterre's own build and packaging files. Two
small experiments, on Windows and on Linux, are described in §7. A statement that is my own
conclusion is marked **(derived)**. One I could not check is marked **(unverified)**. macOS
could not be tried on this machine.

## TL;DR

- **A version slot works, on Windows and Linux alike, if the program reads it with
  `ptr::read_volatile`.** The slot is a 64-byte `static`: a 16-byte marker, then the version,
  NUL-padded. In the experiment, a plain `static` read at a constant index was **constant-folded**
  (it kept printing the build's digit after patching), even with `#[used]`. `black_box` and
  `read_volatile` both read the patched bytes. Only `read_volatile` is *guaranteed* ("not elided
  or reordered"); `black_box` is "best-effort" and "must not be relied upon". `#[used]` only
  stops the static being dropped. A `#[link_section]` is optional; a unique marker, found
  exactly once, is enough for the patcher (§1).
- **So `const VERSION = env!(...)` has to go.** Every `&str` made from it carries its length as a
  compile-time constant, and the literal is copied into `.rdata` apart from any slot. parterre
  would read the slot once at start-up (`read_volatile` into a `String` in a `LazyLock`) and
  hand that to clap, the About box, telemetry and the update check (§1).
- **Prior art:** Go's own toolchain rewrites a placeholder build ID in place after linking, and
  then re-signs Mach-O itself. Electron's *fuses* and Node's single-executable *fuse* are
  sentinel byte strings that are flipped in a prebuilt binary. `cargo-auditable` puts data in a
  section of its own (`.dep-v0`) for tools to find (§1).
- **Windows' version resource can be rewritten in place too**, so no resource tool is needed.
  `rc.exe` **drops NUL padding** from a string (it ends the value at the first NUL), so the build
  pads `FileVersion` and `ProductVersion` with **spaces**. The patcher then writes the version
  NUL-padded into that room, and the fixed `FILEVERSION`/`PRODUCTVERSION` numbers in place.
  Tried: Windows (`FileVersionInfo`) reads the patched strings and numbers. The alternative that
  needs no padding is the pure-Rust crate **`editpe`**. It rebuilt the resource directory of the
  experiment's exe, even with a longer string, and the exe still ran and kept its PDB match. It
  has no OS dependencies, so it runs on a Linux runner. **rcedit** is a Windows program (Linux
  needs Wine), and its repositories were archived in 2026. Electron's packagers have moved to the
  pure-JS **`resedit`**. **Resource Hacker** is Windows-only freeware (§2).
- **What the patch leaves alone:** all three debug IDs. The GNU build ID (and the split
  `.debug`), the PE CodeView record (PDB GUID and age) and Mach-O's `LC_UUID` are written once,
  by the linker, and nothing recomputes them. In the experiment the build ID and the RSDS record
  were byte-for-byte unchanged, and Windows' `dbghelp` still resolved the patched exe's
  backtrace with the original PDB. It refused a PDB from a rebuild. PE's `CheckSum` is 0, so it
  needs no fixing (§3).
- **PostHog resolves the crash whatever `$app_version` says.** cymbal looks a symbol set up by
  team and debug ID only (`WHERE team_id = $1 AND ref = $2`); the release is never part of the
  match. One catch: PostHog then labels the exception with the **release of the symbol sets it
  used** (`latest_release_id`). A stable build's crash would thus be filed under the canary
  release its symbols were uploaded with, though `$app_version` on the event is right. So upload
  symbols under a release name that stays true for every copy, such as the commit (§4).
- **macOS: patch, then `codesign --force --sign -`.** The ad-hoc signature hashes every page,
  so any patched byte breaks it, and Apple silicon kills a binary whose signature doesn't hold.
  The tarball's binary needs re-signing on its own. The disk image needs nothing new:
  `build-dmg.sh` already signs `parterre.app` *after* copying the binary in. Re-signing doesn't
  touch `LC_UUID`, so the dSYM still matches. `rcodesign` (pure Rust) can sign ad hoc on Linux if
  ever needed (§5).
- **Linux packages need nothing new.** `build-packages.sh` takes the .deb and .rpm versions,
  and `metainfo.sh` takes the AppStream release, from `parterre --version`. A patched binary
  therefore packages under the patched version. Nothing else in .deb, .rpm, AppStream or the
  .desktop file carries a version from the binary (§6).

## 1. A version slot

### What keeps the compiler from folding it

The Rust reference on `#[used]`: it "forces a static to be kept in the output object file (.o,
.rlib, etc., excluding final binaries) even if it's never used or referenced by any other item
in the crate. The linker, however, is still free to remove it." It says nothing about how reads
are optimised. A non-`mut` static without interior mutability is immutable, so the optimiser may
replace a read of it with its initializer. The experiment (§7) shows exactly that:

| Read | Whole string after patching | `slot[2]` (constant index) after patching |
| --- | --- | --- |
| plain `static`, with or without `#[used]` | patched | **build-time value** (folded) |
| through `std::hint::black_box(&SLOT)` | patched | patched |
| `std::ptr::read_volatile(&SLOT)` | patched | patched |
| `const` from `env!` (today's `VERSION`) | **build-time value** | — |

The plain static's string happened to survive, because the optimiser didn't evaluate the NUL
search over 48 bytes. A shorter loop, or a check like `starts_with`, would have been folded too.
That is luck, not a guarantee.

- `black_box`: "only (and can only be) provided on a 'best-effort' basis … Programs cannot rely
  on `black_box` for *correctness* … it **must not be relied upon to control critical program
  behavior.**"
- `read_volatile`: volatile operations "are guaranteed to not be elided or reordered by the
  compiler", and on memory inside an allocation "the operation will actually access memory and
  not e.g. be lowered to reusing data from a previous read."

**So: one `read_volatile` of the whole slot at start-up, into a `String` the rest of the program
uses** (derived). Strictly, the abstract machine still says the static holds its initializer.
What the volatile read guarantees is that a real load is emitted, and that load sees the patched
bytes. That is all this needs.

`#[unsafe(link_section = ".pvers")]` (unsafe since the 2024 edition) gave the slot a PE section of
its own in the experiment (`.pvers`, 0x200 bytes), and an ELF one (`.pvers`, flags `A`). It isn't
needed: the patcher finds the 16-byte marker and requires it **exactly once** (§7). A section
would need per-format names (Mach-O wants `__DATA,__pvers`). It would help only if the slot had
to be found with `objcopy --dump-section`-style tools.

### The slot

A sketch for parterre (derived; the names are placeholders):

```rust
const SLOT: usize = 64;
/// The marker, then the version `build.rs` computed, NUL-padded. CI overwrites the version.
#[used]
static VERSION_SLOT: [u8; SLOT] = slot(b"@@PARTERRE-VER@@", env!("PARTERRE_VERSION"));

pub fn version() -> &'static str {
    static V: LazyLock<String> = LazyLock::new(|| {
        let bytes = unsafe { std::ptr::read_volatile(&VERSION_SLOT) };
        // after the marker, up to the first NUL
    });
    &V
}
```

`build.rs` is unchanged: it still computes the version, so dev builds, `cargo install` and the
Flatpak and Snap source builds say what they say today, and a binary that is never patched holds
its own version. clap takes the version at run time (`#[command(version = version())]`). 64 bytes
leave 48 for the version: `0.7.1-nightly.20261016+a1b2c3d` is 30. If the ring is also written in
(#396, #397), it can be a second field in the slot or a slot of its own.

### Prior art

- **Go**: `go build` links with a placeholder build ID, hashes the output, then finds the
  placeholder and rewrites it in place (`updateBuildID` → `buildid.Rewrite` in
  `src/cmd/go/internal/work/buildid.go`, `src/cmd/internal/buildid/rewrite.go`). `Rewrite` then
  regenerates the Mach-O ad-hoc signature itself (`cmd/internal/codesign`), because "the code
  signature contains hashes of the whole file … including the buildid". It added that after
  [golang/go#42684](https://go.dev/issue/42684): "On the production Apple Silicon machines, Go
  binaries are killed at start."
- **Electron fuses**: "Somewhere in the Electron binary, there will be a sequence of bytes": a
  sentinel `dL7pKGdnNz796PbbjQWNKmHXBZaB9tsX` followed by a *fuse wire*, one byte per fuse,
  flipped "at package time before you code sign your app" by `@electron/fuses`. For arm64 macOS
  without a later signing step it asks for `resetAdHocDarwinSignature: true`, or "the app will
  refuse to launch with code signature validation errors."
- **Node single executable applications**: the build searches the binary for the
  `NODE_SEA_FUSE_fce680ab2cc467b6e072b8b5df1996b2:0` string and flips the last character to `1`.
  The docs then re-sign: `codesign --sign -` on macOS, `signtool` (optional) on Windows.
- **cargo-auditable**: data in a section of its own, `.dep-v0` (`__DATA` segment on Mach-O),
  which `rust-audit-info`, or `objcopy --dump-section` on Linux, reads out.

No crate packages a patchable version slot: crates.io searches for "patch version binary",
"binary patch slot", "stamp version" and "embed version patch" turned up nothing relevant
(`shipwright-version-stamp` stamps manifests, not binaries). The mechanism is ~50 lines, std
only.

## 2. Windows' version resource

Today `build.rs` writes a resource script with `FILEVERSION`/`PRODUCTVERSION` (numbers) and
`FileVersion`/`ProductVersion` strings (`PARTERRE_VERSION`) and compiles it with `embed-resource`.
`build-msi.ps1:49` reads `ProductVersion` from the exe, but only when `PARTERRE_RELEASE_TAG` isn't
set. The release workflow sets it, so the MSI's version comes from the tag there.

### In place (recommended)

- A `VS_VERSIONINFO` `String` is `wLength, wValueLength, wType`, the UTF-16 key, padding to 32
  bits, then `wValueLength` UTF-16 units including the terminator. A patcher finds the key
  (`"ProductVersion\0"` in UTF-16, exactly once), takes the capacity from `wValueLength`, and
  writes the new value NUL-padded. Nothing moves.
- **`rc.exe` drops NUL padding.** A value written `L"0.7.1-dev.3+a1b2c3d\x0000\x0000…"` came out
  with `wValueLength` 20 and no padding at all: rc ends the value at the first NUL. Plain `\0`
  in the script broke the parse altogether (`RC2104` on the next line). **Padding with spaces
  works.** The unpatched build then reports `"0.7.1-dev.3+a1b2c3d"` followed by 45 spaces.
  `build-msi.ps1` already splits on the first space, and every published binary is patched
  anyway (derived). Generating the `VS_VERSIONINFO` bytes in `build.rs` and embedding them as a
  raw `1 16 "file"` resource would avoid the spaces (unverified).
- `VS_FIXEDFILEINFO` (signature `0xFEEF04BD`) holds the numbers as two `u32` pairs. They are
  rewritten in place: a nightly of 0.7.2 gets 0.7.2.0.
- Tried (§7): after patching, `(Get-Item x.exe).VersionInfo` reads `ProductVersion` and
  `FileVersion` as the new string with no trailing NULs or spaces, and `FileVersionRaw` and
  `ProductVersionRaw` as the new numbers. The exe runs.

### Tools that rewrite resources

| Tool | What | Runs on Linux? |
| --- | --- | --- |
| [rcedit](https://github.com/electron/rcedit) | Electron's CLI: `--set-version-string`, `--set-file-version`, `--set-product-version` | No: a Windows exe, so Wine. Both `rcedit` and `node-rcedit` are archived (read-only since 2026). |
| [resedit](https://github.com/jet2jet/resedit-js) | Pure-JS PE resource editor | Yes. `@electron/packager` (`src/win32.ts`, `resedit ^2.0.3`) and electron-builder's `app-builder-lib` (`resedit ^3.1.0`) use it now instead of rcedit. |
| Resource Hacker | Windows freeware GUI with a command line | No (Wine) |
| [editpe](https://github.com/Systemcluster/editpe) 0.2.4 | Pure Rust: `ResourceDirectory::get_version_info`/`set_version_info`, `Image::set_resource_directory`, which "rebuilds the resource directory and file and section headers" | Yes: its dependencies are `zerocopy`, `indexmap`, `log` and the like, with no OS APIs. |
| `pelite` 0.10 | Reads PE and its version info; no writing (last release 2022) | — |

**editpe, tried (§7):** it set both strings to a 73-character version, longer than the build
reserved, and the numbers to 0.7.2. The exe ran, Windows read the new values, the sections kept
their addresses, the RSDS record was unchanged, and `dbghelp` resolved a backtrace with the
original PDB. It changed 346 bytes, against 190 for the in-place patch.

**Choice (derived):** patch in place. One mechanism serves the slot and the resource, needs no
dependency, and moves nothing. editpe is the fallback if the space padding is unwanted. Either
must run before any Authenticode signing: parterre's exe and MSI are unsigned today
(`docs/distribution.md`), and a signature covers the patched bytes.

**The MSI after a patch:** WiX reads the exe's numeric version into the `File` table. The
`MajorUpgrade` in `parterre.wxs` keeps the default schedule (`afterInstallValidate`), so the old
product is removed before files are compared. Two canary builds with the same numeric version
therefore still replace each other (derived; `AllowSameVersionUpgrades="yes"` already covers rc
and release).

## 3. What the patch leaves alone

| Platform | Debug ID | Where it lives | After patching the slot |
| --- | --- | --- | --- |
| Linux | GNU build ID | `.note.gnu.build-id`, a hash the linker computes once over its output | Unchanged (§7: `439e7b2c…` before and after, and in the `objcopy --only-keep-debug` file) |
| Windows | PDB GUID + age | CodeView `RSDS` record in `.rdata`, pointed to by the debug directory | Unchanged (§7: `e41e20d7…` age 1 before and after). `dbghelp` resolved the patched exe's backtrace with the original PDB, and resolved nothing with a rebuild's PDB (GUID `10b75d3a…`). |
| macOS | `LC_UUID` | A load command the linker writes | Unchanged by the patch and by `codesign` (unverified on a Mac; `codesign` rewrites `LC_CODE_SIGNATURE`'s data in `__LINKEDIT`) |

The patched bytes are data in `.rdata`/`.rodata`/`.pvers` and `.rsrc`. No code moves, so the
addresses the debug info describes stay right. Every changed byte lies inside a slot or a
version string. That makes a cheap self-check for the patcher: compare the debug ID before and
after, or check that every difference lies inside a slot (derived).

PE's optional-header `CheckSum` was 0 (rustc doesn't pass `/RELEASE`). Windows checks it only for
drivers and boot files, so there is nothing to fix.

The other side of this: **every copy of one canary binary shares its debug ID**, whatever
version is written in. That is what lets one symbol set serve canary, nightly, rc and stable
(#394's "upload each binary once").

## 4. PostHog: matching by debug ID, labelling by release

- **What the client sends.** `posthog-rs` 0.27 reads each loaded module's ID at run time
  (`findshlibs`): GNU build ID → `debug_id`, Mach-O UUID, or PDB signature (GUID + age). It sends
  them in `$debug_images`. Its own comment: "image matching is by debug_id and address"
  (`src/error_tracking.rs`). The version is never part of it.
- **What cymbal matches.** A native frame resolves via `catalog.lookup(team_id,
  OrChunkId::chunk_id(debug_image.debug_id))` (`rust/cymbal/src/core/types/langs/native.rs`).
  That calls `SymbolSetRecord::load`:
  `… FROM posthog_errortrackingsymbolset WHERE (content_hash is not null OR storage_ptr is null)
  AND team_id = $1 AND ref = $2` (`symbol_store/saving.rs`). **No release, no version.** A crash
  from a binary whose `$app_version` is 0.7.2 resolves against a set filed under
  `0.7.2-canary…`.
- **But the release label follows the symbol set.** Alongside symbolication,
  `resolve.rs` runs `latest_release_id(team_id, &symbol_set_refs)`, "the newest release bound to
  any of `symbol_set_refs`" (`symbolication/symbol/mod.rs`), and stores it as the resolved
  exception's `release_id`. So in PostHog's release views, a stable build's crash would show
  under the release its symbols were uploaded with, the canary's. The event's `$app_version`
  still says the stable version.
- **So (derived):** upload each binary's symbols once, at its canary build, under a release that
  is true for every copy, for instance `--release-version` set to the commit. Then rely on
  `$app_version` (and the ring property) for which build crashed. Re-uploading under the
  published version fails with `release_id_mismatch` (#394). Which name to use is for #397.

## 5. macOS: re-signing after the patch

- Apple's Big Sur notes, as quoted by
  [Eclectic Light](https://eclecticlight.co/2020/08/22/apple-silicon-macs-will-require-signed-code/)
  and others (Apple's own page not found): on Apple silicon "the operating system enforces that
  any executable must be signed before it's allowed to run … a simple ad-hoc signature is
  sufficient". The linker signs ad hoc at link time, and "if you use a custom workflow involving
  tools that modify a binary after linking … you might need to manually call codesign(1)". It
  "doesn't apply to translated x86 binaries running under Rosetta 2", nor to Intel Macs.
- The signature's code directory hashes each page. Any patched byte breaks it, as Go found
  (golang/go#42684) and as Electron's fuses docs warn.
- **The tarball:** after patching, `codesign --force --sign - parterre`, for both targets. The
  Intel build is cross-compiled on the arm64 runner, and `codesign` handles it there. Re-signing
  it keeps it from shipping a stale signature, even though Intel Macs and Rosetta don't enforce
  one (derived).
- **The disk image:** `build-dmg.sh` copies `target/<triple>/release/parterre` into
  `parterre.app` and then runs `codesign --force --sign - "$app"` and `codesign --verify --strict`.
  Patching the binary before that script runs is all it takes. `--force` replaces the linker's
  signature, and `Info.plist` (still written from `--version`) is sealed with it.
- **Off a Mac:** [`rcodesign`](https://gregoryszorc.com/docs/apple-codesign/main/) signs ad hoc
  when given no certificate ("it will attempt to create an ad-hoc signature"), and its musl
  binaries "should run anywhere" on Linux. Not needed while macOS jobs run on macOS.
- Check after re-signing: `codesign --verify --strict`, `dwarfdump --uuid` equal on binary and
  dSYM, and `--version` where the binary can run (the arm64 one) (derived; unverified, no Mac
  here).

## 6. Linux packages

- `build-packages.sh` runs `"$bin" --version`, takes the second field as the release, and turns
  `-` into `~` for the package version (`--deb-version`, `-s version = …` for cargo-generate-rpm).
  `metainfo.sh` gets the same release. **Patch first, then package:** both packages and the
  AppStream file follow (derived).
- `metainfo.sh` adds a `<release>` only for a version without `-`. A nightly's AppStream file
  lists the stable tags only. That is correct, since only stable goes to software centres.
- Nothing else carries a version from the binary. `[package.metadata.deb]` and
  `[package.metadata.generate-rpm]` hold no version, the `.desktop` file has none, and the
  dependencies (`$auto`, the rpm auto-requires) come from the libraries the binary links, which
  the patch doesn't change. rpm's file digests are taken when packaging, so of the patched file.
- `split-debuginfo.sh` (at the canary build) and the patch commute: the patcher looks for its
  marker, not an offset, and `strip`/`objcopy` keep the build-ID note. No `.gnu_debuglink` is
  added. If one were, its CRC would be of the `.debug` file, which the patch doesn't touch.
- The Flatpak and Snap build from source, stable only, and keep `build.rs`'s version.

## 7. The experiments

Throwaway code under `%TEMP%\parterre-version-slot`, not in the repository:

- `slotexp`: a binary built with parterre's release profile (`lto = "thin"`, `codegen-units = 1`,
  line tables; `strip = "none"` to keep the PDB/debug info). It has three 64-byte slots, each a
  16-byte marker (`@@SLOT-NAIVE@@@@` …) and then `0.7.1-dev.3+a1b2c3d`:
  - a plain `static`, read directly (also tried with `#[used]`);
  - a `#[used]` static read through `black_box(&SLOT)`;
  - a `#[used] #[unsafe(link_section = ".pvers")]` static read with `read_volatile`.

  It prints each slot's string, and each slot's byte at index `MARK + 2` (a constant index).
  Its `build.rs` embeds a VERSIONINFO through `embed-resource` like parterre's, with the version
  strings padded to 64 characters.
- `patch.py` (Python standard library, so it runs anywhere): it requires each marker exactly
  once and writes the new version NUL-padded. On PE it also rewrites the two VERSIONINFO strings
  in place (capacity from `wValueLength`) and the `VS_FIXEDFILEINFO` numbers, and asserts that the
  RSDS record is unchanged.
- `editpe-try`: rewrites the same two strings and the numbers with editpe 0.2.4.

Windows 11, rustc 1.98.1, MSVC linker:

```
patched "0.8.0":   naive 0.8.0 | boxed 0.8.0 | volatile 0.8.0 | const 0.7.1-dev.3+a1b2c3d
                   naive[2] 7  | boxed[2] 8  | volat[2] 8
FileVersionInfo:   [0.8.0] [0.8.0] 0.8.0.0 0.8.0.0         (unpatched: version + 45 spaces)
RSDS before/after: e41e20d773dc034085476d2aa4be006e age 1 slotexp.pdb (both)
backtrace (RUST_BACKTRACE=1) of the patched exe, PDB beside it: slotexp::main at src\main.rs:43
same, with the PDB of a rebuild:                                  no frames
```

The `const` copy of the version appears in `.rdata` apart from all three slots, and is not
patched. Linux (podman, `rust:latest`, rustc 1.99.1): the same table after `strip --strip-all`
and patching, and `readelf -n` shows `Build ID: 439e7b2c5b1e18c215f89ab03ecb8560d2125caf` on the
original, the patched binary and the `--only-keep-debug` file.

## Open for the tickets this blocks

- **#397 (versions and tags):** the format of a nightly/canary version, so the slot's capacity
  can be fixed (64 bytes is ample). Also the release name the canary build files its symbols
  under (§4).
- **#396 (channel and ring):** whether the ring is written into the binary too (a second slot
  field) or only into the package's marker file (#395).
- **The build ticket:** a small std-only Rust tool in the workspace to patch (the `ubuntu:22.04`
  container has no Python), run in each job before packaging. Then `codesign` on macOS, and
  checks: `--version` where it runs, the debug ID unchanged, and every changed byte inside a slot.

## Sources

- Rust reference, [`used` and `link_section`](https://doc.rust-lang.org/reference/abi.html);
  std [`hint::black_box`](https://doc.rust-lang.org/std/hint/fn.black_box.html),
  [`ptr::read_volatile`](https://doc.rust-lang.org/std/ptr/fn.read_volatile.html).
- Go: [`buildid.go`](https://github.com/golang/go/blob/master/src/cmd/go/internal/work/buildid.go),
  [`rewrite.go`](https://github.com/golang/go/blob/master/src/cmd/internal/buildid/rewrite.go),
  [golang/go#42684](https://go.dev/issue/42684).
- [Electron fuses](https://www.electronjs.org/docs/latest/tutorial/fuses),
  [`@electron/fuses`](https://github.com/electron/fuses);
  [Node single executable applications](https://nodejs.org/api/single-executable-applications.html);
  [cargo-auditable PARSING.md](https://github.com/rust-secure-code/cargo-auditable/blob/master/PARSING.md).
- [rcedit](https://github.com/electron/rcedit), [`@electron/packager`](https://github.com/electron/packager)
  (`package.json`, `src/win32.ts`), electron-builder `packages/app-builder-lib/package.json`,
  [editpe](https://github.com/Systemcluster/editpe) (crate source 0.2.4), `pelite` on crates.io.
- PostHog cymbal at `3b515ee9fe`: `rust/cymbal/src/core/types/langs/native.rs`,
  `core/symbolication/symbol_store/{chunk_id,saving}.rs`, `core/symbolication/symbol/{mod,local}.rs`,
  `modes/resolution/service/resolve.rs`; `posthog-rs` 0.27.0 `src/error_tracking.rs`.
- Apple silicon signing: [Eclectic Light, 22 Aug 2020](https://eclecticlight.co/2020/08/22/apple-silicon-macs-will-require-signed-code/)
  quoting Apple's Big Sur developer notes;
  [rcodesign signing](https://gregoryszorc.com/docs/apple-codesign/main/apple_codesign_rcodesign_signing.html),
  [getting started](https://gregoryszorc.com/docs/apple-codesign/main/apple_codesign_getting_started.html).
- parterre: `crates/parterre/build.rs`, `src/version.rs`, `src/win_resource.rs`, `src/main.rs:62`,
  `packaging/windows/build-msi.ps1`, `parterre.wxs`, `packaging/linux/{build-packages,metainfo,split-debuginfo}.sh`,
  `packaging/macos/build-dmg.sh`, `.github/workflows/release.yml`, `docs/releasing.md`.
