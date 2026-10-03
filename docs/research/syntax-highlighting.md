# Syntax highlighting for the compare and blame windows

Research note for the question: *which syntax-highlighting engine should parterre use in its file
diff ("compare") window and its blame window, so that code is easy to read whatever the language,
without parterre writing language support itself?* It continues §5 of
`docs/research/file-diff-engines.md` (branch `research/file-diff-engines`, 2026-09-26, #42), which
measured the options on a 12.8 MB binary and said "not now". The roadmap (#25) parks syntax colour
as "wanted, but last", with its own discussion when the time comes. This is that discussion's
research.

Sources are official docs, source code at pinned tags or commits, the crates.io API and
experiments I ran on this worktree (commit `dc673c7`). A statement that is my own conclusion is
marked **(derived)**. A statement I could not check is marked **(unverified)**. A statement I
checked by running a command is marked **(tested)**; the commands and numbers are in §8. Data
is as of 2026-10-03.

## TL;DR

- **Every option that covers the wanted languages costs megabytes or a C compiler, usually both.**
  syntect with two-face's syntaxes: +2.9 MiB, 12 crates, no C, 200 ms for a 3,450-line file.
  tree-sitter with seven grammar crates: +12.5 MiB (C# alone is 5.4 MB of parse tables), 15
  crates, C compiled at build time, 22 ms for the same file. Both measured here **(tested)**.
- **A C compiler is already required.** Since 2026-09-26 (fcb3400, the GitHub pull-request
  feature) the default build pulls `ring` through `ureq`/`rustls`, and `ring` compiles C and
  assembly with the `cc` crate: 30 object files in `target/release/build/ring-*/out/`
  **(tested)**. The earlier research's `cargo tree -e build -i cc` did not show this because it
  only follows build edges; `cargo tree -i cc -e normal,build` does. So "needs a C compiler" no
  longer separates the options; CI already installs `build-essential` in the Linux container and
  the Windows and macOS runners ship MSVC and Xcode's clang.
- **syntect's own default set lacks TypeScript, TOML and Dockerfile** (its Sublime Packages pin
  is from 2017-11-13); C#, Java, Rust, Markdown and the rest are there. two-face (bat's set,
  213 syntaxes) fills the gaps under permissive licences. syntect is the engine egui_extras,
  bat, delta and gitui use.
- **The known liabilities of syntect are real but bounded:** it needs `bincode` 1.3.3, whose team
  has stopped (RUSTSEC-2025-0141); the fix (PR #694, `serde-wincode`, byte-compatible) is open
  against the unreleased 6.0.0. With `default-features = false` nothing else unmaintained comes
  along (`yaml-rust` and `plist` are only in `default-fancy`, which egui_extras's `syntect`
  feature uses). fancy-regex fails 11 Markdown syntax-test assertions that Oniguruma passes.
- **tree-sitter is faster and more precise but heavier in every other way:** one grammar crate per
  language, each 0.4–5.4 MB of static tables, three legacy crates (toml, dockerfile, sql) that
  cannot even build against the current runtime, capture names that differ between grammars,
  and Markdown needs two grammars plus injection plumbing that my probe did not get right.
  The bundles (inkjet: archived; syntastica: MPL-2.0; arborium, lumis: large, young) do not
  remove those costs; they only choose the grammars for you.
- **Recommendation (§7):** syntect, minimal features, with two-face's syntax set, highlighting
  whole blobs in `parterre-core` into language-neutral scope spans, mapped to parterre's own light
  and dark palettes in the app. Fallback: syntect's default set alone (−0.7 MiB, loses
  TypeScript/TOML/Dockerfile). tree-sitter stays the option if speed or precision ever matters
  more than 10 MiB.

## Sources (pinned)

| Short name | What | Permalink base |
|---|---|---|
| SYNTECT | trishume/syntect `v5.3.0` @ `e4670846` (2025-09-27) | https://github.com/trishume/syntect/blob/v5.3.0/ |
| PACKAGES | sublimehq/Packages @ `fa6b8629` (2017-11-13, syntect's pin) and @ `759d6eed` (2019-11-21, bat's pin) | https://github.com/sublimehq/Packages/tree/fa6b8629c95041bf262d4c1dab95c456a0530122 |
| TWOFACE | two-face `v0.5.2+bat-0.26.1` (2026-08-07) on Codeberg | https://codeberg.org/CosmicHarper/two-face/src/tag/v0.5.2%2Bbat-0.26.1/ |
| TS | tree-sitter `v0.27.0` @ `6070dbfe` (2026-08-30) | https://github.com/tree-sitter/tree-sitter/blob/6070dbfefd326bd735e5683eb128cc1b57dad0c0/ |
| EGUI | emilk/egui `0.36.2` @ `49682f8b` | https://github.com/emilk/egui/blob/0.36.2/ |
| GITUI | gitui `v0.28.1` @ `e24fb45d` | https://github.com/gitui-org/gitui/blob/e24fb45df1584ee8d8ebdc4258531b4a91ca975d/ |
| DELTA | delta `0.19.2` @ `1502986e` | https://github.com/dandavison/delta/blob/1502986e81c8e3fa27bfb5b84bac6b281a6b1edd/ |
| BAT | bat `v0.26.1` @ `979ba226` | https://github.com/sharkdp/bat/blob/979ba22628bc9d8171f2cffca2bd5c90c9fc0a9e/ |
| GB | GitButler `release/0.22.3` @ `0ba35dc6` | https://github.com/gitbutlerapp/gitbutler/blob/0ba35dc675c0d0eebcf2e1d5e114b73d46019942/ |
| DIFFT | difftastic `0.71.0` @ `b7d119e9` | https://github.com/Wilfred/difftastic/blob/0.71.0/ |
| HELIX | helix `25.07.1` @ `a05c151b` | https://github.com/helix-editor/helix/blob/a05c151bb6e8e9c65ec390b0ae2afe7a5efd619b/ |
| ZED | zed `v1.22.0` @ `76659a55` | https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/ |
| LAPCE | lapce `v0.4.6` @ `b012cef4` | https://github.com/lapce/lapce/blob/b012cef466741528f338879fa708355217fc40bd/ |
| TGIT | TortoiseGit `master` @ `7338078f` (read 2026-10-03) | https://gitlab.com/tortoisegit/tortoisegit/-/blob/7338078f8ddd924b8cddee35f512f2286072136d/ |
| CRATES | crates.io API (read 2026-10-03) | https://crates.io/api/v1/crates/ |
| RUSTSEC | RustSec advisory database (read 2026-10-03) | https://rustsec.org/ |
| GIT | git `v2.55.0` | https://github.com/git/git/blob/v2.55.0/ |

## 1. The question and its constraints

parterre draws two code views: the compare window, where `FileDiff::new(old, new, options)` takes
both whole versions as `&str` and produces `DiffLine`s with `text` (tabs expanded) and `raw`
(as in the file) plus word-diff `spans` (`crates/parterre-core/src/file_diff.rs`), and the blame
window, where `Blame` holds the whole blamed file as `BlameLine`s with the same `raw`/`text` pair
(`crates/parterre-core/src/blame.rs`). Both windows build one egui `LayoutJob` per visible row,
appending `TextFormat` sections with a font, a colour and a background
(`crates/parterre/src/app/diff_window.rs` around the `job.append` loop; `blame_window.rs` uses one
`layout_no_wrap` galley per line). Only visible rows are laid out, so a cached list of spans per
line is all the windows need; the file's text is UTF-8 with invalid bytes shown as `\xNN`
(`file_diff::decode`). The app has a light and a dark `Palette` (`crates/parterre/src/theme.rs`),
and the project's rules say: `parterre-core` stays free of GUI dependencies; avoid uncommon crates
and binary bloat; prefer the git CLI over libraries; third-party notices come from `cargo-about`
with the licence list in `packaging/about.toml`, which covers crates but not assets bundled inside
them.

The owner wants code readable regardless of language: at least Markdown, Java, C#, Rust and
TypeScript, plus JavaScript, Python, C/C++, Go, HTML/CSS, JSON/YAML/TOML, shell, SQL and XML.
parterre will not write language support itself, so the engine must bring its grammars. The
earlier research's numbers (12.8 MB baseline; syntect +2.2 MiB; tree-sitter +2.7 MiB for one
grammar) are a year of releases old and were taken before `ureq` joined the build, so they are
re-measured here on the current 18.2 MB release binary.

## 2. What "good enough" means

- **Languages.** The list above, found from the file's path. All engines key on extensions;
  filenames like `Dockerfile` and `Makefile`, shebangs and `.gitattributes` `diff=` drivers are
  extras that only some provide (§3, per option). git's own built-in `diff=` drivers are a free
  hint: `ada`, `bash`, `bibtex`, `cpp`, `csharp`, `css`, `dts`, `elixir`, `fortran`, `fountain`,
  `golang`, `html`, `java`, `kotlin`, `markdown`, `matlab`, `objc`, `pascal`, `perl`, `php`,
  `python`, `ruby`, `rust`, `scheme`, `tex`
  ([gitattributes.adoc @ v2.55.0](https://github.com/git/git/blob/v2.55.0/Documentation/gitattributes.adoc)),
  and parterre already runs `git check-attr -z diff -- <path>` to find the textconv driver
  (`Git::textconv`, `crates/parterre-core/src/git.rs`), so the attribute's value is in hand
  **(tested: read the code)**.
- **State that spans lines.** Block comments (`/* … */`), raw and multi-line strings, Markdown
  fenced code and HTML `<script>`/`<style>` blocks all start on one line and end on another. A
  hunk that starts inside one of them is coloured wrongly unless the whole file was highlighted
  first. So: highlight each whole version once, then pick lines. Both windows already hold the
  whole text.
- **Light and dark.** Colours must read on both palettes. The engine should hand back *what* a
  span is (a scope or capture name), and the app maps that to a colour per palette; shipping
  someone else's theme with its own background is the wrong shape for parterre's windows, which
  draw their own backgrounds for added/removed lines, word diffs, matches and selections.
- **Diffs.** Both sides of a diff are highlighted the same way from the same engine, so a line
  that did not change looks the same on both sides. The word-diff background and the syntax
  foreground must combine, which `TextFormat { color, background }` already allows.
- **Blame.** One whole file; nothing special.
- **Cost.** Binary size, crate count, build time, a C compiler, licences of bundled grammars, and
  who maintains it. The thresholds are the owner's to set; the numbers are in §3 and §8.
- **Speed.** A 10,000-line file should highlight well under a second on a background thread, so
  the window opens with plain text and gets colour a moment later, as the diff itself is loaded.

## 3. Options

All numbers **(tested)** on this worktree unless a cell cites a source; crates.io data read
2026-10-03; sizes are bytes added to the 18,243,288-byte probe baseline (§8). "Wanted" is the
language list of §1: 18 languages plus Dockerfile and Makefile.

| Option | Version (date) | Owners | Licence: crate / assets | Wanted covered | Binary added | Crates | C | Build added | 3,450 lines | Colours | Detection | Supply chain |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| syntect minimal (§3.1) | 5.3.0 (2025-09-27) | trishume, robinst, Enselic, keith-hall | MIT / Sublime grant + MIT themes | all but TypeScript, TOML, Dockerfile | 2,315,360 (2.21 MiB) | 11 | no | ~19 s | 196 ms | scope stacks → own palette, or a code-built `Theme` | extension, name, first line | `bincode` 1.3.3 unmaintained (RUSTSEC-2025-0141); 30 M downloads; 4 owners; fix PR open |
| syntect + two-face (§3.2) | 0.5.2+bat-0.26.1 (2026-08-07) | + CosmicHorrorDev | MIT OR Apache-2.0 / 61 MIT, 13 Apache-2.0, 4 BSD, 2 Unlicense, 2 WTFPL, Sublime grant | **all** (213 syntaxes) | 3,026,152 (2.89 MiB) | 12 | no | ~22 s | 211 ms | same | same, plus `Dockerfile` | same `bincode`; one owner for two-face; repo on Codeberg |
| tree-sitter + 7 grammar crates (§3.3) | 0.27.0 (2026-08-30) | tree-sitter org; 2–3 people per grammar | MIT / MIT | measured 8 of them; crates exist for all | 13,082,688 (12.48 MiB) | 15 | **yes** | ~5 s wall (16 threads) | 22.5 ms (+108 ms once) | capture names → own palette | none; own table | no advisories; MSRV 1.90; thin grammar ownership; 3 legacy crates unusable |
| inkjet subset (§3.4) | 0.11.1 (2024-09-15) | Colonial-Dev | MIT OR Apache-2.0 / not listed in the crate | no Markdown, no XML | 12,866,472 (12.27 MiB) | 9 | yes | ~10 s | 25.8 ms | Helix scope names | token table | **archived**; runtime 0.23 |
| syntastica subset (§3.4) | 0.6.1 (2025-06-19) | RubixDev | **MPL-2.0** / nvim-treesitter queries (Apache-2.0) relabelled | no XML | 14,300,248 (13.64 MiB) | **72** | yes | (grammars prebuilt) | 43 ms | 91 nvim theme keys | names, not extensions | one author; 18 k downloads |
| arborium subset (§3.4) | 2.18.2 (2026-08-28) | fasterthanlime | MIT OR Apache-2.0 / MIT, Apache-2.0, CC0 (21 of 112 listed) | all | 13,470,960 (12.85 MiB) | 23 | yes | ~10 s | 31.7 ms (+24 ms per language once) | 73 capture names → 28 slots | string names | one author; 2025-12 crate; "co-developed with LLMs" |
| lumis (§3.4) | 0.16.0 (2026-09-29) | leandrocp | MIT / no licence list | all | not measured | 60+ grammar crates + build tools | yes | | | 293 scope names | path, shebang, emacs mode | one author; `xz2`, wasmtime optional; GPL grammar in `all-languages` |
| synoptic (§3.5) | 2.2.9 (2024-11-30) | curlpipe | MIT / own rules | all but Dockerfile, Makefile | 1,702,808 (1.62 MiB) | 8 | no | ~3 s | 24.7 ms | token names → own palette | extension | one author, last commit 2024-11-30; regex-level quality |
| egui_extras fallback (§3.5) | 0.36.2 (2026-09-08) | emilk, rerunio | MIT OR Apache-2.0 / – | C/C++, Python, Rust, TOML only | 41,440 (40 KiB) | 3 | no | ~0 | 4.2 ms | egui `TextFormat`s in the library | name or extension | egui's owners; no cross-line state |
| egui_extras `syntect` feature | | | | syntect's default set | ≈ syntect `default-fancy`: 2.21 MiB, 26 crates (earlier research) | | no | | | seven fixed `.tmTheme`s | | adds `yaml-rust` (RUSTSEC-2024-0320) and `plist` |
| external tool (§6) | | | | | 0 | 0 | | | one process per file | theirs | | not installed by default anywhere |

### 3.1 syntect (Sublime Text grammars)

**What it is.** A Rust port of Sublime Text's `.sublime-syntax` engine: regex-driven context
stacks, one line at a time, with a `ScopeStack` per token. Owners trishume, robinst, Enselic,
keith-hall; 5.3.0 on 2025-09-27, 30.1 M downloads (10.8 M in 90 days); MIT; no `rust-version`
([crates.io](https://crates.io/api/v1/crates/syntect), [owners](https://crates.io/api/v1/crates/syntect/owners)).
Releases are 12–20 months apart (5.0.0 2022-05, 5.1.0 2023-07, 5.2.0 2024-02, 5.3.0 2025-09).
2,437 stars, 140 open issues, last push 2026-04-28 **(tested: `gh api`)**. A breaking 6.0.0 is
in progress on `master` (milestone "syntect 6.0.0", 14 open issues; `ParseState::parse_line`
return type changes, `once_cell` dropped, Sublime build-4075 syntax features)
([CHANGELOG, master](https://github.com/trishume/syntect/blob/master/CHANGELOG.md),
[milestones](https://github.com/trishume/syntect/milestones)).

**Features.** From [Cargo.toml @ v5.3.0](https://github.com/trishume/syntect/blob/v5.3.0/Cargo.toml):
`default-syntaxes = [parsing, dump-load]`, `default-themes = [dump-load]`,
`dump-load = [flate2, bincode]`, `parsing = [regex-syntax, fnv, dump-create, dump-load]`,
`regex-fancy = [fancy-regex]`, `regex-onig = [onig]`, `yaml-load = [yaml-rust, parsing]`,
`plist-load = [plist, serde_json]`, `html`, `metadata`; `default-fancy` and `default-onig`
switch everything on. A regex engine is mandatory and it is either `fancy-regex` 0.16.2 (pure
Rust, owners raphlinus/robinst/keith-hall, 244 M downloads
([crates.io](https://crates.io/api/v1/crates/fancy-regex))) or `onig` 6.5.1 (Oniguruma, C via
`onig_sys` and `cc`). The dumps are deserialised with **`bincode ^1.0`**, which no feature
avoids ([dependencies](https://crates.io/api/v1/crates/syntect/5.3.0/dependencies)). The minimal
set for parterre is `default-features = false, features = ["default-syntaxes",
"default-themes", "regex-fancy"]`; it brings 11 crates (`syntect`, `fancy-regex`, `bit-set`,
`bit-vec`, `regex-automata`, `regex-syntax`, `aho-corasick`, `bincode`, `fnv`, `walkdir`,
`same-file`; `flate2` is already in the tree) and no C **(tested)**.

**The `bincode` and `yaml-rust` situation.** RUSTSEC-2025-0141 (issued 2026-01-07) marks every
`bincode` version unmaintained: "the bincode development team has permanently halted work on the
project following a doxxing and harassment incident"; the team considers 1.3.3 complete
([advisory](https://rustsec.org/advisories/RUSTSEC-2025-0141.html)). The GitHub repository is
archived (2025-08-15) with development moved to sourcehut; 2.0.1 (2025-03) and 3.0.0
(2025-12-16) exist on crates.io but syntect pins `^1.0` ([crates.io bincode](https://crates.io/api/v1/crates/bincode),
[bincode-org/bincode](https://github.com/bincode-org/bincode)). On syntect's side: issue
[#623](https://github.com/trishume/syntect/issues/623) "`bincode` is unmaintained" (open,
2026-04-04), issue [#606](https://github.com/trishume/syntect/issues/606) (open), and PR
[#694](https://github.com/trishume/syntect/pull/694) "Replace bincode with serde-wincode" (open
since 2026-06-26, mergeable, byte-compatible with the existing dumps). No released syntect has
dropped bincode 1. `yaml-rust` (RUSTSEC-2024-0320) was replaced by `yaml-rust2` on `master` after
5.3.0 ([#608](https://github.com/trishume/syntect/pull/608)); it only enters a build through
`yaml-load`, which the minimal feature set leaves out. No advisories exist for `syntect`,
`fancy-regex`, `two-face`, `plist` or `onig` **(tested: rustsec.org/packages/<name>.html → 404)**.
The only fork on crates.io, `syntect-no-panic` 6.0.0 (2025-01-03), is older than syntect itself
(`fancy-regex ^0.11`) and no fix ([crates.io](https://crates.io/api/v1/crates/syntect-no-panic)).

**Languages.** `default-syntaxes` is one dump built from the `testdata/Packages` submodule plus
Plain Text ([gendata.rs @ v5.3.0](https://github.com/trishume/syntect/blob/v5.3.0/examples/gendata.rs),
[Makefile](https://github.com/trishume/syntect/blob/v5.3.0/Makefile)). The submodule is
sublimehq/Packages at `fa6b8629` of **2017-11-13**
([.gitmodules @ v5.3.0](https://github.com/trishume/syntect/blob/v5.3.0/.gitmodules),
[commit](https://github.com/sublimehq/Packages/commit/fa6b8629c95041bf262d4c1dab95c456a0530122);
`gh api` confirms the pin **(tested)**). 75 syntaxes **(tested)**. Of the wanted list:
Markdown, Java, **C#** (`C#/` is a top-level package), Rust, JavaScript, JSON, Python, C, C++,
Go, HTML, CSS, YAML, Bash, SQL, XML and Makefile are there; **TypeScript/TSX, TOML and Dockerfile
are not** ([Packages tree @ fa6b8629](https://github.com/sublimehq/Packages/tree/fa6b8629c95041bf262d4c1dab95c456a0530122);
`find_syntax_by_extension` for `ts`, `tsx`, `toml`, `Dockerfile` returns `None` **(tested)**).
Adding them means two-face (§3.2) or shipping `.sublime-syntax` files and building a dump
(`dump-create`, `yaml-load` at build time; the files' licences then need listing by hand).

**Licences of the bundled assets.** Crate: MIT. The syntaxes: sublimehq/Packages' LICENSE is a
bare grant, "Permission to copy, use, modify, sell and distribute this software is granted. This
software is provided 'as is' without express or implied warranty, and with no claim as to its
suitability for any purpose", with an exception for files carrying their own licence (the Rust
package is MIT) ([LICENSE @ fa6b8629](https://github.com/sublimehq/Packages/blob/fa6b8629c95041bf262d4c1dab95c456a0530122/LICENSE)).
It is not an SPDX licence and I found no FSF or OSI determination; it imposes no condition at
all, so nothing in it can conflict with the GPL **(derived)**. The seven default themes come from
three MIT submodules: kkga/spacegray (the `base16-*` four, "Copyright (c) 2013 Gadzhi
Kharkharov"), braver/Solarized (MIT) and sethlopezme/InspiredGitHub.tmtheme (MIT)
([.gitmodules @ v5.3.0](https://github.com/trishume/syntect/blob/v5.3.0/.gitmodules)). syntect's
own README says only that its code is MIT and nothing about the assets
([Readme @ v5.3.0](https://github.com/trishume/syntect/blob/v5.3.0/Readme.md)). `cargo-about`
sees the MIT crate and nothing else **(tested: `cargo about generate` passes with syntect and
two-face in the lock)**, so the Packages notice must be added to the notices by hand.

**Speed.** `SyntaxSet::load_defaults_newlines()` 0.8 ms (lazy). `physics.rs` (3,450 lines,
137 KB): parse only 192 ms, parse and style 196 ms; the 10,000-line file 559/567 ms — about
55 µs per line **(tested)**. The README says fancy-regex is "about half the speed of the default
Oniguruma engine" and "absurdly slow in debug mode"
([Readme @ v5.3.0](https://github.com/trishume/syntect/blob/v5.3.0/Readme.md)); the earlier
research measured Oniguruma at 1.05 MiB but it is C. Debug builds of parterre optimise
dependencies at `opt-level = 2` (workspace `Cargo.toml`), which blunts the debug-mode warning.

**Quality.** Sublime's syntaxes are mature; the state machine handles block comments, string
escapes and nesting (`comment.block.rust` continues onto the next line; `\"` inside a string is
`constant.character.escape.rust` **(tested)**). Known failures under fancy-regex at v5.3.0: 38
assertions in `C#/tests/syntax_test_Strings.cs` (also under onig), 1 in LaTeX, and **11 in
`Markdown/syntax_test_markdown.md` that pass under onig**
([known_syntest_failures_fancy.txt](https://github.com/trishume/syntect/blob/v5.3.0/testdata/known_syntest_failures_fancy.txt),
[known_syntest_failures.txt](https://github.com/trishume/syntect/blob/v5.3.0/testdata/known_syntest_failures.txt)).

**Themes and colour mapping.** `ThemeSet::load_defaults()` gives seven `.tmTheme`-derived
themes; a `Theme` is a plain struct (`scopes: Vec<ThemeItem>`, `ThemeItem { scope:
ScopeSelectors, style: StyleModifier }`, all `Default`) and `ScopeSelectors: FromStr` parses
`"comment, string"`, so a light and a dark theme of a dozen rules can be written in code
without `plist-load` ([Theme](https://docs.rs/syntect/5.3.0/syntect/highlighting/struct.Theme.html),
[ScopeSelectors](https://docs.rs/syntect/5.3.0/syntect/highlighting/struct.ScopeSelectors.html)).
Or skip themes: classify each token's `ScopeStack` by prefix (`comment`, `string`, `keyword`,
`storage`, `entity.name`, `constant`, `markup.heading`, …) into parterre's `Kind`
**(derived)**.

**Language detection.** `find_syntax_by_extension`, `find_syntax_by_name`,
`find_syntax_by_token`, `find_syntax_by_first_line` (shebangs, from the syntaxes'
`first_line_match`) and `find_syntax_for_file` (extension or whole filename, then first line)
([SyntaxSet](https://docs.rs/syntect/5.3.0/syntect/parsing/struct.SyntaxSet.html)). `Makefile`
is found by name; `Dockerfile` only with two-face **(tested)**.

**Memory and threads.** `SyntaxSet` is `Send + Sync` and lazily links syntaxes; `ParseState` is
`Clone` but not `Send`, so highlight on one worker thread and send the spans back **(derived
from the docs.rs auto-traits)**.

### 3.2 two-face (bat's syntax and theme set for syntect)

**What it is.** The syntaxes and themes that `bat` curates, dumped for syntect: version
`0.5.2+bat-0.26.1` (2026-08-07), 9.5 M downloads (5.4 M in 90 days), one owner
(CosmicHorrorDev, who also answers in syntect's issue tracker, #606), MIT OR Apache-2.0, MSRV 1.79, repository moved
from GitHub to Codeberg in 0.5.2 ([crates.io](https://crates.io/api/v1/crates/two-face),
[CHANGELOG](https://codeberg.org/CosmicHarper/two-face/src/branch/main/CHANGELOG.md)). It
depends on `syntect ^5.3.0` with `dump-load` and `parsing`, so `bincode` 1.3.3 comes along;
features `syntect-fancy`/`syntect-onig` pick the regex engine and `syntect-default-*` the full
syntect feature sets ([Cargo.toml](https://codeberg.org/CosmicHarper/two-face/src/branch/main/Cargo.toml)).
The crate is 3.6 MB on crates.io; the `fancy` newlines dump is 959 KB **(tested: registry copy)**.

**Languages.** `two_face::syntax::extra_newlines()` is a complete replacement `SyntaxSet`:
**213 syntaxes** **(tested)**, every wanted one present including TypeScript, TypeScriptReact,
TOML and Dockerfile (by filename) **(tested)**. The assets are bat's at v0.26.1 (submodule
`979ba226`); bat's own `01_Packages` is sublimehq/Packages at `759d6eed` of 2019-11-21, two years
newer than syntect's pin **(tested: `gh api` on both submodules)**. Four syntaxes are dropped
under fancy-regex because they use features it lacks: ARM Assembly, JavaScript (Babel),
PowerShell, Salt State SLS ([README](https://codeberg.org/CosmicHarper/two-face/src/branch/main/README.md)).

**Licences of the bundled assets.** The crate ships `acknowledgement::listing()` (the licences
that require a notice; 66 entries **(tested)**) and links the full list, 83 entries
([acknowledgements_full.md @ v0.5.2+bat-0.26.1](https://codeberg.org/CosmicHarper/two-face/src/tag/v0.5.2%2Bbat-0.26.1/generated/acknowledgements_full.md)).
Classified by licence text **(tested: script over the file)**: 61 MIT (including Rust, TOML,
Docker, Swift and all but two themes), 13 Apache-2.0 (including TypeScript and Kotlin), 3
BSD-2-Clause, 1 BSD-3-Clause (HTML Twig), 2 Unlicense/public domain (GLSL, PureScript), **2
WTFPL** (the GraphQL syntax and the github-sublime-theme), and the Sublime grant above for
`01_Packages`. The `LicenseType` enum has exactly those eight variants: no GPL, LGPL or
Creative Commons ([docs.rs](https://docs.rs/two-face/latest/two_face/acknowledgement/enum.LicenseType.html)).
WTFPL and the Sublime grant are not in `packaging/about.toml`; as assets they are outside
`cargo-about`'s view anyway, so the notice file needs a hand-written section. two-face's
`Acknowledgements::to_md()` can generate it at build time **(derived)**.

**Speed.** Same engine: 202 ms parse / 211 ms styled on `physics.rs`, 600–690 ms on 10,000
lines; the larger set loads in 1.5 ms **(tested)**. 32 themes, lazily decompressed one at a
time (`EmbeddedLazyThemeSet`) — unused if parterre maps scopes itself.

**Cost.** +3,026,152 bytes over the probe baseline (2.89 MiB), 12 crates (syntect's 11 plus
two-face), no C, about 3 s more build time than syntect alone **(tested)**.

### 3.3 tree-sitter + tree-sitter-highlight + grammar crates

**What it is.** An incremental GLR parser whose grammars compile to C tables; `tree-sitter`
0.27.0 (2026-08-30; owners maxbrunsfeld, amaanq and the `tree-sitter:core` team; MIT; **MSRV
1.90**, up from 1.77 in 0.26.x; 42.6 M downloads) wraps `lib/src/lib.c` through the `cc` crate
in its build script, with no feature that avoids it
([build.rs @ v0.27.0](https://github.com/tree-sitter/tree-sitter/blob/6070dbfefd326bd735e5683eb128cc1b57dad0c0/lib/binding_rust/build.rs),
[crates.io](https://crates.io/api/v1/crates/tree-sitter)). `tree-sitter-highlight` 0.27.0 (4.3 M
downloads) adds `HighlightConfiguration` (a grammar plus its `highlights.scm`,
`injections.scm` and `locals.scm`) and `Highlighter::highlight`, which yields
`HighlightEvent`s over the whole buffer
([docs.rs](https://docs.rs/tree-sitter-highlight/0.27.0/tree_sitter_highlight/)). The
`wasm` feature loads grammars compiled to WebAssembly through `wasmtime` (what Zed does for
extensions), but the C runtime is still compiled and `wasmtime` itself needs `cc` and has 49
RustSec advisories to date ([deps](https://crates.io/api/v1/crates/wasmtime/48.0.1/dependencies),
[rustsec](https://rustsec.org/packages/wasmtime.html)); it is no way around a C compiler.
`lib/Cargo.toml` sets `links = "tree-sitter"`, so only one runtime version can exist in a build
([Cargo.toml](https://github.com/tree-sitter/tree-sitter/blob/6070dbfefd326bd735e5683eb128cc1b57dad0c0/lib/Cargo.toml),
[Cargo reference](https://doc.rust-lang.org/cargo/reference/build-scripts.html#the-links-manifest-key)).

**Grammar crates.** One crate per language, each compiling its `parser.c` (and a hand-written
`scanner.c` where the language needs one) with `cc`, exposing `LANGUAGE: LanguageFn` and the
query strings. Modern crates depend only on `tree-sitter-language ^0.1`, the ABI-stable shim,
and keep the runtime as a dev-dependency, so they work with any runtime that accepts their ABI
(0.27 takes versions 13–15; the crates below are 14 or 15)
([api.h](https://github.com/tree-sitter/tree-sitter/blob/6070dbfefd326bd735e5683eb128cc1b57dad0c0/lib/include/tree_sitter/api.h)).
Of the wanted languages, crates.io data on 2026-10-03 **(tested: crates.io API)**:

| Language | Crate | Version (date) | Owners | `parser.c` | Queries exposed |
|---|---|---|---|---|---|
| Rust | tree-sitter-rust | 0.24.2 (2026-03-27) | dcreager, maxbrunsfeld | 6.5 MB | highlights, injections, tags |
| Java | tree-sitter-java | 0.23.5 (2024-12-21) | dcreager, maxbrunsfeld | 2.6 MB | highlights, tags |
| C# | tree-sitter-c-sharp | 0.23.5 (2026-04-14) | dcreager, maxbrunsfeld, damieng | **32.0 MB** | highlights, tags (injections/locals cfg-gated off) |
| TypeScript, TSX | tree-sitter-typescript | 0.23.2 (2024-11-11) | dcreager, maxbrunsfeld, patrickt | 8.7 + 8.8 MB | highlights, locals, tags (the JavaScript queries must be prepended) |
| Markdown | tree-sitter-md | 0.5.3 (2026-02-26) | MDeiml, tree-sitter-grammars team | 2.1 + 2.3 MB (block + inline) | highlights and injections for both grammars |
| JavaScript | tree-sitter-javascript | 0.25.0 (2025-09-01) | dcreager, maxbrunsfeld | 2.9 MB | highlights, jsx, injections, locals, tags |
| Python | tree-sitter-python | 0.25.0 (2025-09-11) | dcreager, maxbrunsfeld | 3.4 MB | highlights, tags |
| Go | tree-sitter-go | 0.25.0 (2025-08-29) | dcreager, maxbrunsfeld | 1.5 MB | highlights, tags |
| C, C++ | tree-sitter-c 0.24.2, tree-sitter-cpp 0.23.4 | 2026-04-22, 2024-11-11 | dcreager, maxbrunsfeld | 3.7 MB, **25.9 MB** | highlights, tags |
| HTML, CSS | tree-sitter-html 0.23.2, tree-sitter-css 0.25.0 | 2024-11-11, 2025-09-28 | maxbrunsfeld + one | 66 KB, 492 KB | highlights (+ injections for HTML) |
| JSON | tree-sitter-json | 0.24.8 (2024-11-11) | maxbrunsfeld, sergey-sign | 27 KB | highlights |
| YAML | tree-sitter-yaml | 0.7.2 (2025-10-07) | **amaanq alone** | 1.3 MB | highlights |
| TOML | tree-sitter-toml-ng | 0.7.0 (2024-12-03) | ObserverOfTime, team | 131 KB | highlights. (`tree-sitter-toml` 0.20.0 of 2022 depends on runtime ^0.20 and cannot coexist with 0.27) |
| Shell | tree-sitter-bash | 0.25.1 (2025-12-02) | dcreager, maxbrunsfeld | 9.9 MB | highlights |
| SQL | tree-sitter-sequel | 0.3.11 (2025-10-01) | **DerekStride alone** | **39.7 MB** | highlights |
| XML | tree-sitter-xml | 0.7.0 (2024-11-13) | ObserverOfTime, team | 235 KB | highlights (XML and DTD) |
| Dockerfile | tree-sitter-containerfile 0.9.2 (2026-07-04; what difftastic uses) | not checked | | (tree-sitter-dockerfile's is 254 KB) | (`tree-sitter-dockerfile` 0.2.0 is on runtime ^0.20: unusable) |
| Makefile | tree-sitter-make | 1.1.1 (2024-12-21) | **amaanq alone** | 924 KB | highlights |

All MIT; none declares an MSRV. Sources: each crate's crates.io page and `bindings/rust/lib.rs`
at its tag (for example [tree-sitter-rust](https://github.com/tree-sitter/tree-sitter-rust/blob/v0.24.2/bindings/rust/lib.rs),
[tree-sitter-c-sharp](https://github.com/tree-sitter/tree-sitter-c-sharp/blob/v0.23.5/bindings/rust/lib.rs),
[tree-sitter-md](https://github.com/tree-sitter-grammars/tree-sitter-markdown/blob/v0.5.3/bindings/rust/lib.rs));
`parser.c` sizes from the GitHub contents API. Half of them have had no release since
November–December 2024 (java, typescript, cpp, html, json, xml, toml-ng, make). The crates.io
owners are two or three people per crate and in three cases one; every tree-sitter-org release
is published by maxbrunsfeld.

**Measured with seven crates (Rust, Java, C#, TypeScript+TSX, Markdown block+inline,
JavaScript, Python; nine configurations).** **(tested)** Binary +13,082,688 bytes (**12.5 MiB**);
15 crates (`tree-sitter`, `-highlight`, `-language`, the 7 grammars, `regex`, `regex-automata`,
`regex-syntax`, `aho-corasick`, `streaming-iterator`); `cc` compiles the runtime and every
grammar. The static libraries explain the size: C# 5.36 MB, TypeScript+TSX 2.95 MB, Rust
1.15 MB, Markdown 0.85 MB, Python 0.53 MB, JavaScript 0.45 MB, Java 0.45 MB, runtime 0.41 MB
(`target/release/build/tree-sitter-*/out/*.a`). The earlier research's "about 0.5 MiB per extra
grammar" held for its four small grammars and does not hold for C#, TypeScript, C++, Bash or
SQL. Clean build time: the C stage ran about 19 s before the Rust crates started on the first
attempt; a rebuild after `cargo clean -p` of the ten crates took 48.9 s against the 44.2 s
reference (16 threads), so the wall-clock cost is small. Speed: compiling the nine query
configurations 108 ms once; `physics.rs` parse 9.8 ms, parse and highlight 22.5 ms; 10,000
lines 28 / 64 ms — nine times faster than syntect.

**Quality.** TypeScript and C# samples come out well: block comments, template strings with
`${}` interpolation, parameters, properties, attributes **(tested, §8)**. Weak spots: `using
System` and `Console` are `variable` in C# (the query has no `module` rule for them); Rust
injections into Markdown fences and the `markdown_inline` injection did not light up in my
probe beyond `fn` as a keyword — getting Markdown right needs the block grammar, the inline
grammar, the injection callback and the `text.*` capture names, and I did not get it right in
an afternoon **(tested; may be my plumbing, unverified against Helix's set-up)**.

**Colours.** The app chooses which capture names it recognises (`configure`), so the mapping
to parterre's `Kind` and palette is direct; the names vary per grammar (§4).

**Language detection.** None in the crates. tree-sitter's `tree-sitter.json` per grammar holds
`file-types` and `first-line-regex` but the Rust crates do not expose it
([docs](https://tree-sitter.github.io/tree-sitter/3-syntax-highlighting.html)); parterre would
keep its own extension table, as difftastic and GitButler do.

**Supply chain.** No advisories on any of the crates **(tested: rustsec.org)**. The grammars
are MIT. Bus factor: the runtime has an organisation behind it; the grammars are a handful of
people, and crates.io publishing is one person for the tree-sitter org. MSRV 1.90 is below
parterre's `rust-version` 1.95, so it fits today, but the jump from 1.77 to 1.90 in one minor
release shows the policy is loose.

### 3.4 Bundles of tree-sitter grammars

These crates choose and vendor grammars and queries so that one dependency brings many
languages. None removes the C compile or the per-grammar table size; what they add is
curation, a capture-name vocabulary and themes. Measured with the same seven-language subset
where features allow **(tested)**.

| Crate | Version (date) | Owner | Licence | Runtime | Languages | Subset? | Status |
|---|---|---|---|---|---|---|---|
| inkjet | 0.11.1 (2024-09-15) | Colonial-Dev | MIT OR Apache-2.0 | tree-sitter **0.23** | 79 `language-*` features; **no Markdown or XML** in the published crate (the archived repository's `languages/markdown` never shipped) | yes | **archived 2025-09-04**: "Inkjet is no longer supported by me… check out `autumnus`" ([README](https://github.com/Colonial-Dev/inkjet/blob/d692ceb775c8194f8bc73088358513c940e71764/README.md), [gh api: archived](https://api.github.com/repos/Colonial-Dev/inkjet)) |
| syntastica + syntastica-parsers | 0.6.1 (2025-06-19) | RubixDev | **MPL-2.0** (GPL-3.0 up to 0.5) | tree-sitter 0.25.2 (own fork of tree-sitter-highlight; `runtime-c2rust` for wasm32 only) | 64 (`some` = 17, `most`, `all`); no XML | yes, per language | last commit 2026-06-11; 46 stars; 1 main contributor ([repo](https://github.com/RubixDev/syntastica)) |
| arborium | 2.18.2 (2026-08-28) | fasterthanlime (bearcove) | MIT OR Apache-2.0 | own vendored copy of the tree-sitter C runtime (`arborium-tree-sitter`, `cc`) | 112 `arborium-<lang>` crates, 111 permissive + nginx (GPL-3.0, opt-in) | yes, `lang-*` | 503 stars, 41 open issues, 410 of 424 commits by one person, "co-developed with LLMs" ([README](https://github.com/bearcove/arborium/blob/45fae8adc0d4e62a42d68eda0454f606b3fff6aa/README.md)) |
| lumis (ex-autumnus) | 0.16.0 (2026-09-29) | leandrocp | MIT | tree-sitter 0.26.9 + `lumis-wasm-runtime` (wasmtime optional) | 114 `lang-*`, all wanted ones; nginx (GPL-3.0) inside `all-languages` | yes, plus bundles | 5,218 commits, last 2026-10-03; 228 stars; build needs `cc`, `xz2`, `syn` ([Cargo.toml](https://github.com/leandrocp/lumis/blob/98737defeec3e935874d797ebe4253f61f391982/crates/lumis/Cargo.toml)) |
| tree-painter | 0.0.0 (2022-08-13) | matze | MIT | tree-sitter-highlight 0.20 | 7 | | dead (last commit 2023-02-04); HTML output only |

Sources: crates.io API for each crate and its `/dependencies` page **(tested: script)**; the
reports' permalinks per crate.

- **inkjet** vendors `parser.c` and Helix-derived queries per language under `languages/` and
  compiles them in its own `build.rs`; the published crate carries no per-grammar LICENSE files
  and the README says the queries come from Helix (MPL-2.0 project; query provenance per file
  **unverified**) ([README](https://github.com/Colonial-Dev/inkjet/blob/d692ceb775c8194f8bc73088358513c940e71764/README.md)).
  `highlight_raw` returns tree-sitter-highlight events over `constants::HIGHLIGHT_NAMES`
  (Helix's scope names). Measured with Rust, Java, C#, TypeScript, TSX, JavaScript, Python:
  **+12,866,472 bytes (12.3 MiB)**, 9 crates (`inkjet`, `tree-sitter` 0.23.2, `-highlight`
  0.23.2, `-language`, `regex` and friends, `lazy_static`), about 10 s more build time; 25.8 ms
  / 73 ms highlight. Its TypeScript queries are weaker than upstream's: the block comment,
  `function dist` and the arrow of the sample were left plain **(tested)**. Being archived on an
  old runtime, it is not a candidate.
- **syntastica** returns `Highlights = Vec<Vec<(&str, Option<&'static str>)>>`: lines of text
  slices with one of 91 nvim-treesitter theme keys (`keyword.function`, `markup.heading.1`,
  `comment.documentation`, …), which is exactly the language-neutral shape parterre wants
  ([Highlights](https://docs.rs/syntastica/0.6.1/syntastica/type.Highlights.html),
  [theme_keys.rs](https://github.com/RubixDev/syntastica/blob/d9ac0663b4b8f102c3938deff787aa45e0583db1/syntastica-core/src/theme_keys.rs)).
  Its queries are "forked from nvim-treesitter" (Apache-2.0) and shipped under MPL-2.0 without
  per-file attribution ([README](https://github.com/RubixDev/syntastica/blob/d9ac0663b4b8f102c3938deff787aa45e0583db1/README.md));
  MPL-2.0 is not in `packaging/about.toml` (it is GPL-compatible under its §3.3 secondary
  licence terms, but the owner would have to accept it). Measured with the seven languages
  (`rust, java, c_sharp, typescript, tsx, markdown, markdown_inline, javascript, python`):
  **+14,300,248 bytes (13.6 MiB)** and **72 new crates** (`palette`, three `phf` versions, two
  `strum`s, `toml`, `serde_with`, `rand`, `darling`, `lazy-regex`, …), with the grammar crates
  pinned a step behind (tree-sitter 0.25.10, tree-sitter-md 0.3.2, tree-sitter-rust 0.24.0);
  43 ms / 125 ms highlight **(tested)**. The crate count alone rules it out under the
  "uncommon crates" rule.
- **arborium** exposes `Highlighter::highlight_spans(language, source) -> Vec<Span { start,
  end, capture: String, pattern_index }>` with raw capture names, and `arborium-theme` folds the
  73 names of its vocabulary into 28 theme slots (Keyword, Function, String, Comment, Type, …)
  ([Highlighter](https://docs.rs/arborium/latest/arborium/struct.Highlighter.html),
  [Span](https://docs.rs/arborium-highlight/latest/arborium_highlight/struct.Span.html),
  [ThemeSlot](https://docs.rs/arborium-theme/latest/arborium_theme/highlights/enum.ThemeSlot.html)).
  "WASM support" means the same C compiled for `wasm32` with its own sysroot, not an
  interpreter inside a native app; every `arborium-<lang>` crate has `cc` as a build
  dependency ([arborium-rust build.rs](https://docs.rs/crate/arborium-rust/2.18.2/source/build.rs),
  [dependencies](https://crates.io/api/v1/crates/arborium-rust/2.18.2/dependencies)). The Rust
  grammar is the `grammar-orchard` fork from Codeberg, not tree-sitter's. Its `LICENSES.md`
  lists 21 of the 112 grammars ([LICENSES.md](https://github.com/bearcove/arborium/blob/45fae8adc0d4e62a42d68eda0454f606b3fff6aa/LICENSES.md)),
  and the README's "~70 languages by default" disagrees with the crate's empty `default`
  feature. Measured with `lang-rust, -java, -c-sharp, -typescript, -tsx, -markdown, -javascript,
  -python`: **+13,470,960 bytes (12.85 MiB)**, **23 crates** (Markdown drags in `arborium-html`,
  `-toml`, `-yaml` and `-css` for its injections, JavaScript drags in `-jsdoc`), about 10 s more
  build time; 31.7 ms / 90.9 ms, plus about 24 ms per language the first time a grammar is
  used (`CompiledGrammar`) **(tested)**. Its static libraries: C# 5.13 MB, Rust (orchard
  fork) 2.07 MB, TSX 1.54 MB, TypeScript 1.48 MB. To its credit, the Markdown sample came out
  right: the fenced Rust was injected and highlighted (`fn`, `main`, the block comment, the
  string and its escape); but `highlight_spans` returns every capture, so spans overlap and
  repeat (`dist` as `function` and twice as `variable`) and the caller must resolve them as
  `arborium-highlight`'s `spans_to_flat_tokens` does. Languages are strings (`"c-sharp"`
  works, `"c_sharp"` does not) **(tested)**.
- **lumis** has every wanted language, 293 scope names, 250+ themes and an active author, but
  it is built for HTML and terminal output (formatters), its build pulls `cc`, `xz2`
  (liblzma), `syn` and a 4.4 MB crate, and `all-languages` includes a GPL-3.0 grammar
  ([LANGUAGES.md](https://github.com/leandrocp/lumis/blob/98737defeec3e935874d797ebe4253f61f391982/LANGUAGES.md)
  has no licence column). `highlight::highlight_iter` does give `(text, Language, Range,
  scope, &Style)` callbacks ([docs.rs](https://docs.rs/lumis/latest/lumis/highlight/fn.highlight_iter.html)).
  Not measured: a 2026-01 crate with one owner and a build this heavy is outside the project's
  rules whatever the number comes out as **(derived)**.

### 3.5 Pure-Rust rule-based highlighters

| Crate | Version (date) | Owner | Licence | Deps | Languages | Cross-line state | Measured |
|---|---|---|---|---|---|---|---|
| synoptic | 2.2.9 (2024-11-30) | curlpipe (ox editor) | MIT | `regex`, `unicode-width`, `char_index`, `if_chain`, `nohash-hasher` | `from_extension`: 36 match arms covering every wanted language except Dockerfile and Makefile; any other extension gets an empty rule set, so nothing is coloured rather than `None` | yes: `bounded(name, start, end, escapable)` tokens span lines | +1,702,808 bytes (1.62 MiB), 8 crates, 24.7 ms / 71.6 ms **(tested)** |
| egui_extras fallback | 0.36.2 | emilk, rerunio | MIT OR Apache-2.0 | `egui_extras` without `syntect` | C/C++, Python, Rust, TOML keywords | no: strings end at newline, no block comments | +41,440 bytes (40 KiB), 3 crates (`egui_extras`, `enum-map`, `enum-map-derive`), 4.2 ms / 7.5 ms for Rust; Markdown and TypeScript come back as one plain section **(tested)** |
| egui_code_editor | 0.4.1 (2026-08-18) | p4ymak | MIT | optional `egui`, `opener` | Rust, Python, Lua, Shell, SQL, Asm (`Syntax::rust()` …) | yes, one pass over the whole text with `comment_multiline` | not measured: 6 languages, none of Markdown/Java/C#/TypeScript |

- **synoptic** ships regex rule sets in `lib.rs` (`rust_syntax_highlighter()` and so on, 1,900
  lines) with token names such as `comment`, `string`, `keyword`, `digit`, `function`,
  `struct`, `attribute`, `heading`, `link`, `list`, `bold`, `italic`
  ([lib.rs @ 720406a](https://github.com/curlpipe/synoptic/blob/720406a9d2323e5cefd12a3cf84417c1e5937f84/src/lib.rs)).
  Block comments and Markdown fences carry across lines **(tested)**. Quality is what regexes
  give: in the TypeScript sample `number` is a keyword, `a.x` makes `x` a function and
  `Math.sqrt`'s receiver is plain; in C# `System` is a struct; the Markdown fence's Rust is one
  `block` token with no inner colour **(tested)**. One author, 38 commits, last 2024-11-30,
  35 stars **(tested: `gh api`)**; the README itself says "There may be inconsistencies in the
  included pre-built language highlighting rules". Not a candidate for "easy to read
  regardless of language", but the cheapest thing that is better than nothing.
- **egui_extras**'s fallback is a loop over the text: `//` or `#` to end of line is a comment,
  `"` to the next `"` or newline a string, alphanumerics a keyword or literal, anything else
  punctuation, for four languages ([syntax_highlighting.rs @ 0.36.2](https://github.com/emilk/egui/blob/0.36.2/crates/egui_extras/src/syntax_highlighting.rs)).
  With the `syntect` feature it becomes syntect with `default-fancy` (so `yaml-rust` and
  `plist` come along), seven fixed `.tmTheme`s, a `FrameCache` keyed on the whole text that
  re-highlights everything when the text changes, and output as a finished `LayoutJob` in
  egui's types — the wrong side of the core/app boundary. `highlight_with` accepts a
  `SyntectSettings { ps, ts }` so two-face's set could be passed in, but the theme must still be
  one of its enum. Using syntect directly costs the same crates and keeps the choice of
  features and colours.
- Others found and rejected: `giallo` (TextMate, EUPL-1.2, needs Oniguruma), `ferroni` (pure
  Rust Oniguruma port, 2026-02, 3 k downloads), `cmark-syntax` (GPL-3.0, 4 languages),
  `hikari-core` (6 languages), `kopitiam-syntax` (AGPL). No Rust port of Prism or highlight.js
  exists ([crates.io searches](https://crates.io/api/v1/crates?q=syntax+highlighting&sort=recent-downloads&per_page=50)).

## 4. Highlighting a diff and a blame correctly

**Whole versions, then lines.** `FileDiff::new` already receives both whole versions, and
`Blame` the whole blamed file, so the natural shape is: highlight each version once into a
language-neutral `Vec<Span { line, range, kind }>` (or per line, `Vec<Vec<(Range<usize>,
Kind)>>`) in `parterre-core`, and let the windows look up the spans of the rows they draw. Ranges
are byte offsets into `raw`; `display_column` already maps raw offsets to display columns for the
word-diff spans, so tab expansion needs no new code **(derived)**. GitButler highlights hunk
lines in isolation and accepts the errors; parterre does not have to.

**What each engine gives for that.**

- *syntect* is a line machine: `ParseState::parse_line` takes one line (with its `\n` for the
  `newlines` syntax set) and returns scope-stack operations; `ScopeStack::apply` keeps the
  state between lines. Highlighting a whole file is a loop over `LinesWithEndings`, and the
  state after each line can be cloned and cached (`HighlightState` docs: "If you are highlighting
  an entire file you create one of these at the start and use it all the way to the end")
  ([docs.rs HighlightState](https://docs.rs/syntect/5.3.0/syntect/highlighting/struct.HighlightState.html)).
  Each span carries a `ScopeStack` such as `source.rust meta.block.rust comment.block.rust`, and
  `Scope::is_prefix_of` is a bit operation, so classifying a span as comment/string/keyword by
  prefix needs no theme at all ([Scope](https://docs.rs/syntect/5.3.0/syntect/parsing/struct.Scope.html)).
  A `Theme` can also be built in code from `ThemeItem { scope: ScopeSelectors, style:
  StyleModifier }` without `plist-load` ([Theme](https://docs.rs/syntect/5.3.0/syntect/highlighting/struct.Theme.html)).
  Cost: parsing is where the time goes (192 ms of the 196 ms on `physics.rs` **(tested)**), and
  the second version of a file is parsed again from the top; a cache keyed by line text and
  incoming state would let unchanged lines reuse the first pass **(derived, not built)**.
- *tree-sitter* parses the whole buffer into a tree (10 ms for 3,450 lines **(tested)**) and
  `tree-sitter-highlight` walks it emitting `HighlightEvent::{Source, HighlightStart,
  HighlightEnd}` with byte ranges that can cross lines (a block comment is one event). The
  crate has no per-line or sub-range API; it re-parses the whole source each call
  ([highlight.rs @ v0.27.0](https://github.com/tree-sitter/tree-sitter/blob/6070dbfefd326bd735e5683eb128cc1b57dad0c0/crates/highlight/src/highlight.rs)).
  Slicing events into per-line spans is a few lines of code. Capture names come from each
  grammar's `highlights.scm` and are not uniform: Rust uses `comment.documentation`, `escape`,
  `label`; C# uses `property.definition`, `string.escape`, `number`, `module`; Markdown uses
  `text.title`, `text.emphasis`, `text.literal`, `punctuation.special` **(tested, from the
  queries at the crate tags)**. `HighlightConfiguration::configure` maps a capture to the longest
  recognised dotted prefix, so the app's list of recognised names decides what is coloured; the
  runtime's `STANDARD_CAPTURE_NAMES` (52 names, including `markup.*`) is the reference list
  ([highlight.rs](https://github.com/tree-sitter/tree-sitter/blob/6070dbfefd326bd735e5683eb128cc1b57dad0c0/crates/highlight/src/highlight.rs)),
  but `tree-sitter-md` 0.5.3 still uses the older `text.*` names.
- *Regex-rule highlighters* (synoptic, egui_extras's fallback) have no parser. synoptic tracks
  "bounded" tokens across lines, so block comments and multi-line strings work; the egui_extras
  fallback ends a string at the newline and has no block comments at all
  ([syntax_highlighting.rs @ 0.36.2](https://github.com/emilk/egui/blob/0.36.2/crates/egui_extras/src/syntax_highlighting.rs)).

**Both sides the same.** The two versions of a diff may differ in a way that changes the
language guess (a rename from `.js` to `.ts`): guess once from the new path, falling back to the
old, and use it for both **(derived)**.

**Where the code goes.** `parterre-core` gets a `highlight` module with a `Kind` enum (comment,
string, keyword, type, function, number, constant, attribute, punctuation, markup heading,
markup emphasis, …) and `fn highlight(path, text) -> Vec<Vec<(Range<usize>, Kind)>>`; only this
module depends on the engine crate, behind a cargo feature so the binary cost is a choice. The
app crate maps `Kind` to a colour per `Palette` and merges the spans with the existing
word-diff sections when it builds each row's `LayoutJob`. The engine's own themes are not used.

## 5. Prior art

What other git tools and editors do, from their sources at pinned commits (collected 2026-10-03;
the links carry the commit).

| Tool | Engine and assets | Diffs coloured? | How | Language from |
|---|---|---|---|---|
| gitui v0.28.1 | syntect 5.3 (`regex-fancy` default) + two-face 0.4 `extra_no_newlines` ([Cargo.toml](https://github.com/gitui-org/gitui/blob/e24fb45df1584ee8d8ebdc4258531b4a91ca975d/Cargo.toml#L63-L71), [L92](https://github.com/gitui-org/gitui/blob/e24fb45df1584ee8d8ebdc4258531b4a91ca975d/Cargo.toml#L92-L93)) | **No**: file viewer only; `diff.rs` and `blame_file.rs` have no highlighter ([diff.rs](https://github.com/gitui-org/gitui/blob/e24fb45df1584ee8d8ebdc4258531b4a91ca975d/src/components/diff.rs)) | whole file, `ParseState` per line on a background job ([syntax_text.rs](https://github.com/gitui-org/gitui/blob/e24fb45df1584ee8d8ebdc4258531b4a91ca975d/src/ui/syntax_text.rs#L128-L137)) | `find_syntax_for_file`, else plain text |
| delta 0.19.2 | syntect 5 with Oniguruma via bat 0.26's assets ([Cargo.toml](https://github.com/dandavison/delta/blob/1502986e81c8e3fa27bfb5b84bac6b281a6b1edd/Cargo.toml#L22-L26)) | **Yes** | hunk lines only, `HighlightLines::highlight_line` per line, state reset at every hunk header ([paint.rs](https://github.com/dandavison/delta/blob/1502986e81c8e3fa27bfb5b84bac6b281a6b1edd/src/paint.rs#L685-L724), [hunk_header.rs](https://github.com/dandavison/delta/blob/1502986e81c8e3fa27bfb5b84bac6b281a6b1edd/src/handlers/hunk_header.rs#L158)) | file name, extension, then `find_syntax_for_file` ([paint.rs](https://github.com/dandavison/delta/blob/1502986e81c8e3fa27bfb5b84bac6b281a6b1edd/src/paint.rs#L114-L146)) |
| bat v0.26.1 | syntect 5.3 `parsing` + own dumps (bincode 1 + zlib, 77 syntax submodules) ([Cargo.toml](https://github.com/sharkdp/bat/blob/979ba22628bc9d8171f2cffca2bd5c90c9fc0a9e/Cargo.toml#L84-L87), [build_assets.rs](https://github.com/sharkdp/bat/blob/979ba22628bc9d8171f2cffca2bd5c90c9fc0a9e/src/assets/build_assets.rs#L147-L152)) | n/a | whole file | path, then first line |
| GitButler 0.22.3 | shiki 4 (TextMate grammars in JS) ([package.json](https://github.com/gitbutlerapp/gitbutler/blob/0ba35dc675c0d0eebcf2e1d5e114b73d46019942/packages/ui/package.json#L64)) | **Yes** | one line at a time, `codeToTokens(line).tokens[0]`, cached by line text; Svelte/Vue retried as TypeScript because "line-by-line tokenization often produces poorly-colored tokens" ([diffParsing.ts](https://github.com/gitbutlerapp/gitbutler/blob/0ba35dc675c0d0eebcf2e1d5e114b73d46019942/packages/ui/src/lib/utils/diffParsing.ts#L438-L472)) | hand-written extension switch, `Dockerfile` special-cased ([shikiHighlighter.ts](https://github.com/gitbutlerapp/gitbutler/blob/0ba35dc675c0d0eebcf2e1d5e114b73d46019942/packages/ui/src/lib/utils/shikiHighlighter.ts#L315-L326)) |
| GitButler `but` CLI | syntect 5.3 `regex-onig` + two-face 0.5.2 ([Cargo.toml](https://github.com/gitbutlerapp/gitbutler/blob/0ba35dc675c0d0eebcf2e1d5e114b73d46019942/crates/but/Cargo.toml#L150-L156)) | | | |
| lazygit v0.65.1 | none; `git.diffRenderers` pipes through delta, difft, diff-so-fancy ([Config.md](https://github.com/jesseduffield/lazygit/blob/17cb09fa7b08bc96d9f0e81b91f4720fc1a36700/docs/Config.md#L362-L395)) | only via an external renderer | | |
| difftastic 0.71.0 | tree-sitter 0.26.10, 58 grammar crates + 4 vendored ([Cargo.toml](https://github.com/Wilfred/difftastic/blob/0.71.0/Cargo.toml), [build.rs](https://github.com/Wilfred/difftastic/blob/0.71.0/build.rs)) | **Yes, four classes**: string, comment, keyword/type, error ([style.rs](https://github.com/Wilfred/difftastic/blob/b7d119e90ac9f972f03f69508da765dacd302c0c/src/display/style.rs#L364-L399)) | from the parse tree | extension, file name, first lines |
| Helix 25.07.1 | tree-sitter via tree-house; 247 grammars fetched by git and compiled with `cc` to dylibs at build time or by `hx --grammar build` ([grammar.rs](https://github.com/helix-editor/helix/blob/a05c151bb6e8e9c65ec390b0ae2afe7a5efd619b/helix-loader/src/grammar.rs#L425-L428), [build.rs](https://github.com/helix-editor/helix/blob/a05c151bb6e8e9c65ec390b0ae2afe7a5efd619b/helix-term/build.rs#L4-L6)) | n/a | | `languages.toml` |
| Zed v1.22.0 | tree-sitter (git rev) with `wasm`; ~20 built-in grammar crates, extension grammars as `.wasm` built with a downloaded wasi-sdk, run in wasmtime 48 ([Cargo.toml](https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/crates/language/Cargo.toml#L76), [extension_builder.rs](https://github.com/zed-industries/zed/blob/76659a55a8c10ed355a070f8764a0b1733e3c115/crates/extension/src/extension_builder.rs#L270-L299)) | n/a | | |
| Lapce v0.4.6 | tree-sitter 0.22.6, **no grammar crates**: prebuilt dylibs and queries downloaded at run time from `lapce/tree-sitter-grammars` releases, `libloading` ([language.rs](https://github.com/lapce/lapce/blob/b012cef466741528f338879fa708355217fc40bd/lapce-core/src/language.rs#L1876-L1914), [grammars.rs](https://github.com/lapce/lapce/blob/b012cef466741528f338879fa708355217fc40bd/lapce-app/src/app/grammars.rs#L22-L25)) | n/a | | |
| rerun 0.38.1 (egui) | egui_extras **without** `syntect` ([Cargo.toml](https://github.com/rerun-io/rerun/blob/b08c599e934b0dedee1e95fd74a989a1582ce3d5/Cargo.toml#L216)) | n/a | | |
| egui demo 0.36.2 | `egui_extras::syntax_highlighting`, `syntect` feature = `default-fancy` ([Cargo.toml](https://github.com/emilk/egui/blob/49682f8baa058bf49e011035cfbd6e825f88a5ef/crates/egui_extras/Cargo.toml#L98)) | n/a | whole text per frame, memoised by text ([syntax_highlighting.rs](https://github.com/emilk/egui/blob/49682f8baa058bf49e011035cfbd6e825f88a5ef/crates/egui_extras/src/syntax_highlighting.rs#L58-L120)) | name, then extension |
| TortoiseGitMerge | **none**; its Colors page has Normal/Added/Removed/Modified/Conflicted only ([manual](https://tortoisegit.org/docs/tortoisegitmerge/tmerge-dug-settings.html#tmerge-dug-settings-color); no `SetLexer` in `src/TortoiseMerge`) | No | | |
| TortoiseGitBlame | Scintilla + Lexilla lexers: `python` with a keyword list, `cpp` for most extensions, `pascal` ([TortoiseGitBlameView.cpp](https://gitlab.com/tortoisegit/tortoisegit/-/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseGitBlame/TortoiseGitBlameView.cpp#L1063-L1080)) | n/a | | extension switch |
| Sublime Merge, VS Code, GitKraken, Fork | Sublime syntaxes; TextMate grammars (Oniguruma); "syntax highlighting" in the diff viewer; "syntax highlighting" since 1.0.59 ([sublimemerge.com](https://www.sublimemerge.com/), [VS Code guide](https://code.visualstudio.com/api/language-extensions/syntax-highlight-guide), [GitKraken](https://help.gitkraken.com/gitkraken-client/diff/), [Fork notes](https://git-fork.com/releasenotes)) | yes (engines **unverified** for the last two) | | |

Reading: the Rust git tools that colour anything use syntect with bat's assets; the only one
that colours diffs (delta) does it per hunk line and accepts the errors; TortoiseGit, the
behavioural reference, colours blame but not diffs. Editors use tree-sitter and each solves
grammar distribution differently (dylibs, WASM, 62 statically linked grammars), which is the
cost parterre would take on.

## 6. External tools at run time

Shelling out to an installed highlighter was rejected for the diff itself in the earlier
research; for colour alone it is no better **(derived)**: `bat` and `highlight` emit ANSI or
HTML with a theme baked in, so the colours would not be parterre's and parsing SGR sequences
back into runs is the only way to get spans ([bat clap_app.rs](https://github.com/sharkdp/bat/blob/979ba22628bc9d8171f2cffca2bd5c90c9fc0a9e/src/bin/bat/clap_app.rs#L113-L116),
[highlight man page](https://gitlab.com/saalen/highlight/-/blob/master/man/highlight.1#L82-L84));
`tree-sitter highlight` needs grammars compiled into `~/.config/tree-sitter` first
([CLI docs](https://tree-sitter.github.io/tree-sitter/cli/highlight.html)). Only Pygments
(`pygmentize -f tokens`: "tokentype<TAB>repr(tokenstring)" per token
([formatters](https://pygments.org/docs/formatters/))) and chroma (`-f json`: `{type, value}`
objects ([json.go](https://github.com/alecthomas/chroma/blob/master/formatters/json.go#L13-L32)))
give a token stream with reconstructible byte offsets, and neither is installed by default on
any platform. One process per file version per view, with the output's encoding and the tool's
version unknown, for a feature that must work out of the box, is the wrong trade. An "open in
external diff tool" command remains the right place for external programs.

## 7. Recommendation and open questions

**(derived)** In order of preference:

1. **syntect (minimal features, fancy-regex) with two-face's syntax set, behind a cargo feature
   in `parterre-core`.** It is the only option that covers the whole language list in pure
   Rust, it is what every Rust git tool with colour uses (bat, delta, gitui, GitButler's CLI),
   its per-line state model fits "whole versions, then lines", and the cost — 2.9 MiB, 12
   crates, 200 ms for 3,450 lines on a background thread — is the smallest of the complete
   options. Map `ScopeStack` prefixes to a `Kind` enum in core; colour `Kind` per `Palette` in
   the app; add the Packages notice and two-face's `acknowledgement` listing to the
   third-party notices. Carry the `bincode` advisory knowingly: it is a serialiser of a file
   compiled into the binary, not a network or user-input path, and syntect's fix is queued.
2. **Fallback: syntect with its own default set.** −0.7 MiB and one owner fewer, at the price of
   TypeScript, TOML and Dockerfile; the gap can be filled later by two-face without changing
   any code but the `SyntaxSet` constructor.
3. **tree-sitter with hand-picked grammar crates, if 10+ MiB is acceptable.** Nine times faster
   and more precise, with capture names that are easy to map, and the C compiler is already a
   given. The price is the size (C# and TypeScript tables), one crate per language with thin
   ownership, a per-grammar capture vocabulary, and real work to get Markdown (two grammars
   plus injections) right. Worth revisiting if syntect's 0.2–0.6 s per file ever shows, or if
   the project wants structural features (folding by syntax, semantic word diff) that need a
   tree anyway.
4. **Not recommended:** egui_extras's `syntect` feature (forces `default-fancy`, its own
   themes, egui types in the highlighter); inkjet (archived, runtime 0.23); syntastica (72
   crates, MPL-2.0); arborium and lumis (young, single-author, heavy builds, same C and size
   costs as plain tree-sitter); synoptic (regex quality); external tools (§6).

Open questions for the human:

- **Size.** Is +2.9 MiB (16 % of 18.2 MB) acceptable for colour? Is +12.5 MiB? The earlier
  research's "18 %" objection was to 2.2 MiB on a 12.8 MB binary; the ratio is similar now.
- **The C compiler.** The build needs one today because of `ring` (`github` feature). Is that
  accepted as permanent, which would make `docs/architecture.md`'s "free of C dependencies"
  claim due for an update, or is `ring` the exception and tree-sitter would be one too many?
- **`bincode` 1.3.3.** Accept an unmaintained-but-complete serialiser in the tree until syntect
  6.0.0, or wait for it? `cargo deny`/`cargo audit` will warn until then; CI does not run them
  today.
- **Licences.** Add the Sublime Packages grant and WTFPL (two assets) to the accepted list for
  bundled assets, with a hand-written section in `THIRD-PARTY-NOTICES.html`? Or use syntect's
  default set only (Sublime grant plus MIT themes)?
- **Language set.** Ship the fixed set (two-face: 213 syntaxes) or also load user-provided
  `.sublime-syntax` files from the config directory at run time (`yaml-load`, +`yaml-rust`
  today)? gitui lets users drop a `.tmTheme`; nothing in the request asks for this.
- **Colours.** One palette of about twelve `Kind` colours per theme, chosen for parterre, or
  a user-selectable theme list? The former keeps the diff backgrounds readable and is what
  the terse-UI rule suggests.
- **When.** The roadmap says "last". The diff and blame windows need a `Kind`-span field and a
  background job either way; the engine is one module.

## 8. Probe log

All throwaway, on this worktree at `dc673c7`, Linux x86_64 (16 threads), rustc 1.98.1,
cargo 1.98.1, gcc 13.3.0; nothing committed. Each variant: the dependency added to
`crates/parterre-core/Cargo.toml` (or `crates/parterre/Cargo.toml` for egui_extras), a
`probe` module in parterre-core (`probe_egui` in parterre) that highlights the file named by
`PARTERRE_PROBE` and prints timings, called at the top of `main()` so thin LTO keeps the
engine; `cargo build --release -p parterre`; size = `ls -l target/release/parterre`; new
crates = `cargo tree -p parterre -e normal --prefix none | sort -u` minus the baseline's 238;
C compiler = `cargo tree -i cc -e normal,build`; build time = wall clock of the build after
the dependency was added, against 44.2 s for the probe baseline, which rebuilds
parterre-core and parterre with LTO. Timings are the best of three runs inside the release
binary; files: `crates/parterre-core/src/physics.rs` (3,450 lines, 136,581 bytes) and the
same file repeated to 10,000 lines (396,340 bytes); small samples in Markdown, TypeScript and
C# for the token dumps. Cargo.toml and Cargo.lock were restored with `git checkout` between
variants.

| Variant | Binary (bytes) | Added | New crates | C | Build | 3,450 lines | 10,000 lines |
|---|---|---|---|---|---|---|---|
| release build of `dc673c7` | 18,240,888 | | 238 | ring, embed-resource (not used on Linux), wayland-backend (shim not compiled here) | | | |
| probe baseline (hook + empty probe) | 18,243,288 | +2,400 | 238 | same | 44.2 s | | |
| syntect `default-syntaxes`, `default-themes`, `regex-fancy` | 20,558,648 | +2,315,360 (2.21 MiB) | 11 | no | 63.5 s (+19 s) | parse 192 ms, styled 196 ms | 559 / 567 ms |
| + two-face `syntect-fancy` | 21,269,440 | +3,026,152 (2.89 MiB) | 12 | no | +3 s over syntect | 202 / 211 ms | 600–694 / 612–620 ms |
| tree-sitter 0.27 + highlight + rust, java, c-sharp, typescript, md, javascript, python | 31,325,976 | +13,082,688 (12.48 MiB) | 15 | yes (runtime + 7 grammars) | C stage ~19 s; clean rebuild of the 10 crates 48.9 s | parse 9.8 ms, highlight 22.5 ms (+108 ms once for 9 query configs) | 28 / 64 ms |
| inkjet 0.11.1, 7 languages (no Markdown available) | 31,109,760 | +12,866,472 (12.27 MiB) | 9 | yes | 54.2 s (+10 s) | 25.8 ms | 73 ms |
| syntastica 0.6.1 + syntastica-parsers, 9 features | 32,543,536 | +14,300,248 (13.64 MiB) | 72 | yes | 50.4 s after the grammars had built | 43 ms | 125 ms |
| synoptic 2.2.9 | 19,946,096 | +1,702,808 (1.62 MiB) | 8 | no | 47.1 s (+3 s) | 24.7 ms | 71.6 ms |
| egui_extras 0.36.2 without `syntect` (in the app crate) | 18,284,728 | +41,440 (40 KiB) | 3 | no | 34.8 s (nothing added) | 4.2 ms (Rust); Markdown and TypeScript: one grey section | 7.5 ms |
| arborium 2.18.2, 8 `lang-*` (pulls html, css, jsdoc, toml, yaml too) | 31,714,248 | +13,470,960 (12.85 MiB) | 23 | yes (vendored runtime + 13 grammars) | 54.1 s (+10 s) | 31.7 ms (+24 ms per language on first use) | 90.9 ms |

Static libraries the grammars produced (`target/release/build/*/out/*.a`, bytes): tree-sitter
runtime 412,062; c-sharp 5,356,700; typescript (ts + tsx) 2,946,026; rust 1,154,522; markdown
(block + inline) 848,918; python 533,550; javascript 449,726; java 445,682. inkjet's copies:
c_sharp 5,944,868; tsx 1,510,724; typescript 1,473,286; rust 1,066,882; python 531,902; java
445,578; javascript 408,574.

Other commands: `cargo about generate -c packaging/about.toml packaging/about.hbs -o
/tmp/notices-v2.html` on the two-face lock: passes, lists syntect, two-face, fancy-regex and
bincode, nothing about the bundled syntaxes. `cargo-audit` is not installed; instead
`https://rustsec.org/packages/<name>.html` was fetched for every new crate: 404 for all but
`bincode` (RUSTSEC-2025-0141), `yaml-rust` (RUSTSEC-2018-0006, RUSTSEC-2024-0320; not in any
variant) and `regex` (RUSTSEC-2022-0013, fixed in 1.5.5; the tree has 1.13.1). Repository
activity from `gh api repos/<owner>/<repo>` and `…/commits?per_page=1`. Licence
classification of two-face's 83 acknowledgement entries by a script matching the licence
texts. Token dumps of the samples are summarised in §3; the raw output is not kept.

Deviations from the brief: `cargo tree -e build -i cc` prints nothing here because `cc` is
reached through normal edges (`ring` → `rustls` → `ureq`); `-e normal,build` is the query that
works. inkjet 0.11.1 has no `language-markdown` feature, so it was measured with seven
languages. syntastica's `Lang::for_name` takes language names, not extensions, so its samples
were all processed as Rust; only the Rust timings are reported. The probe files were deleted;
nothing is kept for the follow-up because the engine code will be written against the real
`DiffLine`/`BlameLine` types.
