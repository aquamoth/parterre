# Git with no terminal attached

Research note for [#139](https://github.com/aquamoth/parterre/issues/139): *what does git need
so that it runs well from a GUI with no terminal attached?* Parterre starts git with
`CREATE_NO_WINDOW` on Windows, and from a desktop launcher on Linux and macOS. The operations in
scope are fetch, push, pull, merge, rebase, cherry-pick and revert. Hard constraint: git 2.34
(Ubuntu 22.04).

Sources are official docs, source code at pinned commits, and experiments I ran. A statement
that is my own conclusion is marked **(derived)**. A statement I could not check is marked
**(unverified)**. A statement I checked by running it is marked **(tested)**. The experiments ran
on:

- **Linux:** an `ubuntu:22.04` container with git 2.34.1, OpenSSH 8.9p1, GnuPG 2.2 with
  `pinentry-curses`, and a local `sshd`. Every git call was started the way a desktop launcher
  starts a GUI's children: `setsid` (no controlling terminal), stdin `/dev/null`, stdout and
  stderr captured.
- **Windows 11:** Git for Windows 2.53.0.windows.3 (bundled OpenSSH 10.2p1) and the in-box
  `OpenSSH_for_Windows_9.5p2`. Every git call was started like `Git::command` starts it:
  `CREATE_NO_WINDOW`, stdin closed, stdout and stderr piped, `cmd\git.exe`.

macOS was not tested.

## TL;DR

Git and ssh only prompt on a terminal, or through an *askpass* program. Without one, most
prompts fail at once. The dangerous cases are the ones that **hang forever, invisibly**. All of
these were tested:

- **Windows OpenSSH (`C:\Windows\System32\OpenSSH\ssh.exe`) loops forever on an unknown host
  key** under `CREATE_NO_WINDOW`, even with `SSH_ASKPASS` and `DISPLAY` set. Only
  `SSH_ASKPASS_REQUIRE=force` makes it ask the askpass program (§3).
- **A terminal editor (vim) opened by git waits forever** in the hidden console. `git merge
  --continue` and `git rebase --continue` open the editor even with no terminal (§6).
- **`Child::kill` on Windows kills only `cmd\git.exe`.** The real `git.exe`, `git-remote-https`
  and `index-pack` keep running and downloading, and they hold the output pipes open, so a
  reader waiting for end-of-file never gets it. `SIGKILL` on Linux also orphans the remote
  helper (§8).

What parterre's operation runner should do **(derived from the tests)**:

| Setting | Why |
|---|---|
| stdin null (as now) | Git then never opens the merge or revert message editor by default, and hooks get `/dev/null`. |
| `GIT_TERMINAL_PROMPT=0` | Credential prompts fail at once ("terminal prompts disabled") instead of trying a terminal. It also turns off GCM's text prompts, but not its GUI prompts. |
| `GIT_ASKPASS=<parterre askpass>` | Asks for a username or password when no credential helper has one. Git's order is `GIT_ASKPASS`, then `core.askPass`, then `SSH_ASKPASS`, then the terminal. |
| `SSH_ASKPASS=<parterre askpass>`, `SSH_ASKPASS_REQUIRE=force` | ssh does not read `GIT_ASKPASS`. `force` (OpenSSH ≥ 8.4) is the only setting that worked for all three ssh builds tested: Linux 8.9p1 with no `DISPLAY`, Git for Windows' 10.2p1, and Windows OpenSSH 9.5p2. It also covers SSH commit signing through `ssh-keygen`. Set `DISPLAY` if unset, for ssh older than 8.4. TortoiseGit sets all of these. |
| `GIT_EDITOR=:` | `:` is special-cased by git: no editor is started, and the prepared message is used. Covers `merge --continue`, `rebase --continue` and `commit`. |
| `GIT_SEQUENCE_EDITOR=:` | `pull.rebase=interactive` in the user's config opens the todo editor. |
| `--progress` on fetch, push, pull, switch and merge | Without it, git prints no progress when stderr is a pipe. |
| Unix: own session (`setsid`) or at least own process group | No controlling terminal even when parterre was started from one. Cancel by `SIGTERM` to the group: git removes its lock files on `SIGTERM`. |
| Windows: cancel with `CTRL_C_EVENT` on the child's console, then kill the whole tree | Git for Windows turns Ctrl+C into `SIGINT` and cleans up. It exited 130 in 175 ms, with no processes left behind. TortoiseGit does the same: Ctrl+C, then it kills the tree. |
| Serialize operations per repository | `index.lock` and ref locks fail at once, and don't wait. A held ref lock can leave a half-done merge (§9). |

What can still prompt, and must be handled or reported: credential helpers' own GUIs (GCM,
macOS Keychain), ssh host-key questions and passphrases (through the askpass), GnuPG pinentry
(gpg-agent's own program; a terminal pinentry fails at once), and hooks. A hook that reads the
terminal fails at once. A hook that waits on anything else blocks until the user cancels.

What to avoid: plain `Child::kill` for cancelling, `SIGKILL`, a terminal `core.editor`, relying on
`SSH_ASKPASS` without `SSH_ASKPASS_REQUIRE=force`, `GIT_ASKPASS` alone for ssh, and bypassing the
user's signing or hooks (`--no-gpg-sign`, `--no-verify`).

Everything here is in git 2.34. The only newer piece mentioned is the `key::` form of
`user.signingKey` (git 2.35), which parterre doesn't need.

## Sources (pinned)

| Short name | What | Permalink base |
|---|---|---|
| GIT234 | git `v2.34.0` @ `cd3e6062` | https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/ |
| GIT | git `master` @ `a0189536` (2.56 in development) | https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/ |
| GITDOCS | git reference manual (current) | https://git-scm.com/docs |
| TGIT | TortoiseGit `master` @ `73380787` | https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/ |
| GCM | Git Credential Manager `main` @ `bb83cfcc` | https://github.com/git-ecosystem/git-credential-manager/blob/bb83cfccd994ddda090e33584ef7f7846d67d59e/ |
| SSH | OpenSSH `ssh(1)`, `readpass.c`, release notes 8.4 | https://man.openbsd.org/ssh.1, https://github.com/openssh/openssh-portable/blob/master/readpass.c, https://www.openssh.org/txt/release-8.4 |
| GPG | GnuPG manual, *Invoking GPG-AGENT* | https://www.gnupg.org/documentation/manuals/gnupg/Invoking-GPG_002dAGENT.html |
| MSSSH | Microsoft Learn, *Key-based authentication in OpenSSH for Windows* | https://learn.microsoft.com/en-us/windows-server/administration/openssh/openssh_keymanagement |

## 1. How parterre starts git today

`Git::command` (`crates/parterre-core/src/git.rs`) runs `git -C <dir>` with stdin null,
stdout and stderr piped, `GIT_OPTIONAL_LOCKS=0`, `LC_ALL=C`, and on Windows `CREATE_NO_WINDOW`.
`git/program.rs` picks `git` on PATH or Git for Windows' `cmd\git.exe`.

- **`cmd\git.exe` is a wrapper.** It starts `mingw64\bin\git.exe`, which starts the helpers
  (`git remote-https`, `git-remote-https.exe`, `git index-pack`) **(tested:** process list
  during a fetch). This matters for cancelling (§8).
- **`CREATE_NO_WINDOW` gives the child a console without a window.** Programs that open the
  console for input (`CONIN$`) block on it, or loop on it, and nobody can type into it
  **(tested:** vim waited forever; Windows OpenSSH looped on "read_passphrase: can't open
  conin$"). Git's own terminal prompt on Git for Windows fails at once instead ("bash: line 1:
  /dev/tty: No such device or address") **(tested)**.
- **On Linux, a desktop launcher gives no controlling terminal**, so `/dev/tty` fails with
  `ENXIO` and prompts fail at once **(tested** with `setsid`). But parterre started from a
  terminal (`cargo run`) passes that terminal on to git, so prompts would appear in the terminal
  and block the operation there **(derived)**. Starting operations in a new session makes the
  two cases the same.

## 2. Credentials (HTTPS)

**How git asks.** For a username or password, git first asks the credential helpers
(`credential.helper`). Only if none answers does it prompt
([gitcredentials](https://git-scm.com/docs/gitcredentials);
[GIT234 `credential.c` L185-199](https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/credential.c#L185-L199)).
The prompt goes, in order, to `GIT_ASKPASS`, then `core.askPass`, then `SSH_ASKPASS`, then the
terminal unless `GIT_TERMINAL_PROMPT=0`. Otherwise git dies with `could not read Username for
'<url>': terminal prompts disabled`
([GIT234 `prompt.c` L45-75](https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/prompt.c#L45-L75);
[git(1) `GIT_ASKPASS`, `GIT_TERMINAL_PROMPT`](https://git-scm.com/docs/git);
[`core.askPass`](https://git-scm.com/docs/git-config)).

**The askpass protocol.** The askpass gets the prompt as its only argument, for example
`Username for 'https://github.com': ` or `Password for 'https://x@github.com': `, and prints the
answer on stdout. Git keeps the text up to the first `\r` or `\n`. There is no flag for "don't
echo": the askpass has to tell usernames from passwords by the prompt text. A non-zero exit means
cancel: git prints `unable to read askpass response from '<program>'`, then falls back to the
terminal, or fails when `GIT_TERMINAL_PROMPT=0` **(tested** 2.34.1 and 2.53: `GIT_ASKPASS`,
`core.askPass` and `SSH_ASKPASS` were each called with the same two prompts).

**No helper, no askpass** **(tested)**:

| Setup | Result |
|---|---|
| Linux, `setsid`, prompts enabled | Fails at once: `could not read Username for 'https://github.com': No such device or address`. |
| Windows, `CREATE_NO_WINDOW`, prompts enabled | Fails at once: `bash: line 1: /dev/tty: No such device or address` / `could not read Username … No such file or directory`. |
| Either, `GIT_TERMINAL_PROMPT=0` | `fatal: could not read Username for 'https://github.com': terminal prompts disabled`, exit 128. |

**Git Credential Manager** is Git for Windows' default helper (`credential.helper=manager` in
`C:\Program Files\Git\etc\gitconfig`, tested). It is optional on macOS and Linux.

- It shows **GUI prompts** when it runs in a desktop session and GUI prompts are enabled,
  otherwise text prompts
  ([GCM `GitHubAuthentication.cs` L78](https://github.com/git-ecosystem/git-credential-manager/blob/bb83cfccd994ddda090e33584ef7f7846d67d59e/src/GitHub/GitHubAuthentication.cs#L78)).
- It honours `GIT_TERMINAL_PROMPT=0` for its **text** prompts
  ([GCM FAQ](https://github.com/git-ecosystem/git-credential-manager/blob/bb83cfccd994ddda090e33584ef7f7846d67d59e/docs/faq.md#how-can-i-disable-gui-dialogs-and-prompts);
  [`AuthenticationBase.cs` L111](https://github.com/git-ecosystem/git-credential-manager/blob/bb83cfccd994ddda090e33584ef7f7846d67d59e/src/Core/Authentication/AuthenticationBase.cs#L111)).
  GUI prompts and the browser still work, so parterre's `GIT_TERMINAL_PROMPT=0` doesn't get in
  its way **(derived)**.
- `GCM_INTERACTIVE=0` (`credential.interactive`) makes it fail rather than prompt, and
  `GCM_GUI_PROMPT=0` turns off its GUI
  ([environment.md](https://github.com/git-ecosystem/git-credential-manager/blob/bb83cfccd994ddda090e33584ef7f7846d67d59e/docs/environment.md#gcm_interactive)).
  Parterre should set neither.
- On Linux it needs a credential store to be configured (`secretservice`, `gpg`, `cache`, …).
  There is no default there
  ([environment.md `GCM_CREDENTIAL_STORE`](https://github.com/git-ecosystem/git-credential-manager/blob/bb83cfccd994ddda090e33584ef7f7846d67d59e/docs/environment.md#gcm_credential_store)).
  That is the user's setup, and parterre only shows GCM's error.
- GUI prompts can be replaced per provider (`credential.gitHubHelper`, …) by an app that wants
  its own look (same FAQ). Parterre doesn't need that.

**Other helpers.** `osxkeychain` (macOS), `libsecret`, `store` and `cache` answer without
prompting. The Keychain may show its own "allow access" dialog **(unverified)**. With no
helper, every operation asks again. Parterre's askpass can't store anything: storing is the
helpers' job (`git credential approve`).

**Handling (derived).** Ship an askpass mode in the parterre binary, for example the same
executable detecting an environment variable. It shows a small dialog, or talks to the running
window over a pipe. TortoiseGit ships a separate `SshAskPass.exe` that shows a password box, and
a Yes/No box when the prompt contains `(yes/no`
([TGIT `SshAskPass.cpp` L71-114](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/SshAskPass/SshAskPass.cpp#L71-L114)).
An open decision: whether to overwrite a `GIT_ASKPASS` or `core.askPass` the user already has.
The environment variable beats the config. TortoiseGit always overwrites
([TGIT `Git.cpp` L2421-2436](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Git/Git.cpp#L2421-L2436)).
An inherited `GIT_ASKPASS` from an IDE terminal (VS Code's) only works while that IDE runs
**(unverified)**.

## 3. SSH

Git runs `ssh` (or `core.sshCommand`, `GIT_SSH_COMMAND`, `GIT_SSH`) and sets nothing about
prompting ([git(1) `GIT_SSH_COMMAND`](https://git-scm.com/docs/git)).
**ssh ignores `GIT_ASKPASS`** **(tested:** host key verification failed with only
`GIT_ASKPASS` set).

**When ssh uses `SSH_ASKPASS`** ([ssh(1)](https://man.openbsd.org/ssh.1#ENVIRONMENT);
[`readpass.c`](https://github.com/openssh/openssh-portable/blob/master/readpass.c)): it uses it
only if `DISPLAY` (or, in newer versions, `WAYLAND_DISPLAY`) is set, or if
`SSH_ASKPASS_REQUIRE=force`. `prefer` means "askpass rather than the terminal", but still only
when `DISPLAY` allows it. `SSH_ASKPASS_REQUIRE` is new in **OpenSSH 8.4**
([release notes](https://www.openssh.org/txt/release-8.4)). Ubuntu 22.04 has 8.9p1, Windows
11 has 9.5p2 in-box, and Git for Windows bundles 10.2p1 (all tested). ssh sets
`SSH_ASKPASS_PROMPT=confirm` for yes/no questions from the agent or a FIDO key, and `none` for
notifications such as "touch your key". It passes nothing for the host-key question, which is a
text prompt that must be answered `yes`, `no` or a fingerprint (same source; tested: the
variable was empty for host-key and passphrase prompts).

**Results** **(tested**, `git ls-remote` against a host not in `known_hosts`, then with a
passphrase-protected key**)**:

| ssh | nothing set | `SSH_ASKPASS` only | `SSH_ASKPASS` + `REQUIRE=prefer` | `SSH_ASKPASS` + `DISPLAY` | `SSH_ASKPASS` + `REQUIRE=force` |
|---|---|---|---|---|---|
| Linux 8.9p1 (`setsid`) | Fails at once: "Host key verification failed." A passphrase key is skipped. | Fails at once | Fails at once | Askpass asked (host key, then passphrase) | Askpass asked |
| Git for Windows 10.2p1 | Fails at once | Fails at once | Fails at once | Askpass asked | Askpass asked |
| Windows OpenSSH 9.5p2 | **Hangs** (loops on `can't open conin$`) | **Hangs** | **Hangs** | **Hangs** | Askpass asked |

TortoiseGit sets `SSH_ASKPASS_REQUIRE=force` for this reason ("improve compatibility with
Win32-OpenSSH", [TGIT `Git.cpp` L2434](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Git/Git.cpp#L2434);
[TortoiseGit issue 3996](https://gitlab.com/tortoisegit/tortoisegit/-/issues/3996), "git
hanging on waiting passphrase from stdin"). TortoiseGit also sets `DISPLAY=:9999`. With `force`,
a wrong passphrase gets asked three times, then ssh falls back to password authentication, which
also goes to the askpass **(tested)**.

- **Host keys.** An unknown key is a question for the askpass: ssh writes the key to
  `known_hosts` after `yes` **(tested)**. A **changed** host key is refused with no question
  ("REMOTE HOST IDENTIFICATION HAS CHANGED"), and the user has to fix `known_hosts` **(tested)**.
  Parterre should not set `StrictHostKeyChecking` itself **(derived)**.
- **Agents.** A key already in the agent needs no prompt **(tested)**. Windows OpenSSH uses the
  `ssh-agent` Windows service, which is disabled by default and holds keys across sessions
  ([MSSSH](https://learn.microsoft.com/en-us/windows-server/administration/openssh/openssh_keymanagement)).
  Git for Windows' own ssh uses `SSH_AUTH_SOCK`. An agent started inside Git Bash is invisible
  to a GUI launched from the Start menu **(derived)**. A key added with `ssh-add -c` asks for
  confirmation in the **agent's** askpass, not parterre's. Here the agent had none, so it
  refused **(tested)**.
- **Which ssh runs on Windows.** Git for Windows' `git.exe` puts its own `usr\bin` before
  `System32\OpenSSH` on PATH **(tested:** `PATH` seen by a git alias**)**, so the bundled ssh runs
  unless `core.sshCommand` or `GIT_SSH(_COMMAND)` points at the Windows one. People who use the
  Windows agent service set that **(derived)**. Parterre must not choose for the user.
- The askpass must be an `.exe` for Windows OpenSSH **(tested:** a Rust `askpass.exe` worked for
  both ssh builds). Git for Windows' ssh and git itself also run `#!` scripts.
- `-o BatchMode=yes` makes ssh never ask, and so never use a passphrase key without an agent
  **(tested)**. It is a fallback for background fetches, not for user operations **(derived)**.

## 4. Signing

- **Which operations sign.** With `commit.gpgSign=true`, every commit git creates is signed:
  merge commits, revert, cherry-pick, and each commit rebase rewrites. A fast-forward creates no
  commit, so nothing is signed
  ([`commit.gpgSign`](https://git-scm.com/docs/git-config)
  warns that rebase can sign "a large number of commits"). **(tested** 2.34.1 with SSH signing:
  merge, revert, cherry-pick and rebase commits carried `BEGIN SSH SIGNATURE`; one passphrase
  prompt per commit.**)**
- **SSH signing** (`gpg.format=ssh`) is new in **git 2.34**. `user.signingKey` is a key file, or
  in 2.34 a literal `ssh-…` public key whose private key is in the agent. The `key::` prefix is
  2.35+ **(tested:** `key::` fails in 2.34.1; [GIT234 `user.txt` L39-44](https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/Documentation/config/user.txt#L39-L44)).
  Git runs `ssh-keygen -Y sign`, which asks for a passphrase the same way ssh does. **The same
  `SSH_ASKPASS` + `SSH_ASKPASS_REQUIRE=force` covers it** **(tested:** askpass got `Enter
  passphrase:`). Without it: `error: Enter passphrase: Load key "…": incorrect passphrase
  supplied to decrypt private key?` / `fatal: failed to write commit object`, at once
  **(tested)**.
- **GnuPG.** gpg asks `gpg-agent`, which starts a *pinentry*. Parterre can't answer it. A
  terminal pinentry needs `GPG_TTY` ([GPG](https://www.gnupg.org/documentation/manuals/gnupg/Invoking-GPG_002dAGENT.html)).
  With no terminal it fails at once with `error: gpg failed to sign the data` **(tested:**
  `pinentry-curses`, with and without `GPG_TTY`). A GUI pinentry (pinentry-gnome3/qt,
  pinentry-mac, Gpg4win, or Git for Windows' own `pinentry-w32.exe`, which it ships) shows its
  own window **(unverified**, not tested). Parterre can only show gpg's error and say that a GUI
  pinentry or a cached passphrase is needed **(derived)**.
- **What a failed signature leaves behind** **(tested)**:
  - A **merge** keeps `MERGE_HEAD` and the staged result, so it is an operation in progress.
    `merge --continue` finishes it, `merge --abort` undoes it.
  - A **cherry-pick** keeps `CHERRY_PICK_HEAD`.
  - A **single-commit revert leaves the reverted changes staged, with no `REVERT_HEAD`** (2.34.1
    and 2.53). Parterre's "operation in progress" check wouldn't see it. Parterre should say so
    in the operation dialog **(derived)**.
- Never add `--no-gpg-sign` to work around a failure: that bypasses the user's policy
  **(derived)**.

## 5. Hooks

**Which hooks run** **(tested** 2.34.1, and the same on 2.53**)**:

| Operation | Hooks |
|---|---|
| `merge` (merge commit) | `pre-merge-commit`, `prepare-commit-msg`, `commit-msg`, `post-merge` |
| `merge` (fast-forward), `pull` that fast-forwards | `post-merge` |
| `merge --no-verify` | skips `pre-merge-commit` and `commit-msg` ([git-merge](https://git-scm.com/docs/git-merge)) |
| `merge --continue` (it is `git commit`) | `pre-commit`, `prepare-commit-msg`, `commit-msg`, `post-commit` (not `pre-merge-commit`, as [githooks](https://git-scm.com/docs/githooks) says) |
| `revert`, `cherry-pick` (clean) | `prepare-commit-msg`, `post-commit` |
| `cherry-pick --continue` | `pre-commit`, `prepare-commit-msg`, `commit-msg`, `post-commit` |
| `rebase` | `pre-rebase`, `post-checkout` (once, on detaching to the new base), `prepare-commit-msg` + `post-commit` per commit, `post-rewrite` |
| `switch`, `worktree add` | `post-checkout` |
| `push` | `pre-push` (stdin gets the refs), then the server's hooks, whose output arrives as `remote:` lines |
| `fetch` | none (only `reference-transaction`) |
| every ref update (2.28+) | `reference-transaction` |

- **Their I/O.** Hooks get **stdin `/dev/null`**. The exceptions are `pre-push`, `post-rewrite`
  and `reference-transaction`, which get data on stdin. Hook **stdout is sent to git's stderr**
  ([GIT234 `run-command.c` L1339-1340](https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/run-command.c#L1339-L1340)).
  The exception is `pre-push`, whose stdout came out on git's stdout **(tested** 2.34.1 and
  2.53**)**. So the operation dialog must show both streams.
- **No terminal.** Under `setsid` (Linux) and `CREATE_NO_WINDOW` with Git for Windows' `sh`
  (Windows), a hook that opens `/dev/tty` fails at once **(tested:** `( : </dev/tty )` failed in
  every hook). A hook that reads stdin gets end-of-file at once.
- **Hooks that wait on anything else** (a network call, a GUI, `git-lfs` pushing objects in
  `pre-push`) block the operation until they finish or the user cancels **(derived)**. A hook
  that needs credentials goes through the same askpass, because the hook inherits the environment
  **(derived)**.
- Hooks see `GIT_EDITOR=:` when parterre sets it. githooks documents this as the sign that no
  editor will open ([githooks](https://git-scm.com/docs/githooks);
  [GIT234 `githooks.txt` L98-100, L119-121](https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/Documentation/githooks.txt#L98-L121)).
- Don't pass `--no-verify` by default **(derived)**.

## 6. Editors

**How git picks the editor:** `GIT_EDITOR`, then `core.editor`, then `VISUAL` (unless
`TERM=dumb`), then `EDITOR`, then `vi`. For rebase todo lists: `GIT_SEQUENCE_EDITOR`, then
`sequence.editor`, then the above. **`:` is special-cased: git starts nothing and uses the
prepared message**
([GIT234 `editor.c` L18-59](https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/editor.c#L18-L59);
[git(1) `GIT_EDITOR`](https://git-scm.com/docs/git)).

**What opens an editor when stdin is not a terminal** **(tested** 2.34.1 and 2.53, with a
logging `core.editor`**)**:

| Command | Editor? | Rule |
|---|---|---|
| `merge` (merge commit) | no | Edits only if stdin and stdout are the same terminal, unless `-e`/`--edit` or `GIT_MERGE_AUTOEDIT=yes` ([GIT234 `merge.c` L1060-1084](https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/builtin/merge.c#L1060-L1084)). |
| `revert` | no | Edits only if stdin is a terminal, unless `-e` ([GIT234 `sequencer.c` L2053-2062](https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/sequencer.c#L2053-L2062)). |
| `cherry-pick` | no | Only with `-e`. |
| `cherry-pick --continue`, `revert --continue` | no | Adds `--no-edit` itself when stdin is not a terminal ([GIT234 `sequencer.c` L4619-4630](https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/sequencer.c#L4619-L4630)). |
| **`merge --continue`** | **yes** | It runs `git commit`, which always edits. `git commit --no-edit` or `GIT_EDITOR=:` avoids it. |
| **`rebase --continue`** after a conflict | **yes** | No `--no-edit` option. Only `GIT_EDITOR=:` avoids it. |
| **`pull` with `pull.rebase=interactive`** | **yes, the todo editor** | Only `GIT_SEQUENCE_EDITOR=:` avoids it. |

**A terminal editor under `CREATE_NO_WINDOW` hangs.** `merge --continue` with `core.editor=vim`
was still waiting after 10 s ("Vim: Warning: Output is not to a terminal"), and had to be killed
**(tested)**. A GUI editor (Notepad, `code --wait`) would open a window and wait for it
**(derived)**. On Linux with no terminal, vi fails at once or waits, depending on the editor
**(unverified)**.

Flags that avoid it: `--no-edit` on `merge`, `pull`, `revert`, `cherry-pick --continue` and
`commit`. `-m`/`-F <file>` give a message instead. `GIT_EDITOR=:` and `GIT_SEQUENCE_EDITOR=:`
cover everything else. `GIT_MERGE_AUTOEDIT=no` is redundant when stdin is not a terminal
([git-merge](https://git-scm.com/docs/git-merge)).

**What TortoiseGit does:**

- **Merge.** Its Merge dialog has an optional message box. If it's filled in, TortoiseGit writes
  it to a temp file and runs `git merge … -F <file>`. Otherwise it runs plain `git merge` and gets
  git's default message
  ([TGIT `AppUtils.cpp` L3231-3278](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/AppUtils.cpp#L3231-L3278)).
  After conflicts, its own Commit dialog finishes the merge with `git commit -F <file>`.
- **Revert.** It runs `git revert --no-edit --no-commit <hash>` for each selected commit
  ([TGIT `Git.cpp` L3357](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Git/Git.cpp#L3357)).
  It then asks OK or *Commit*, and *Commit* opens its Commit dialog with the prepared message
  ([TGIT `GitLogListAction.cpp` L1119-1145](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/GitLogListAction.cpp#L1119-L1145)).
- **Cherry-pick.** Plain `git cherry-pick <hash>`.

TortoiseGit never lets git open an editor. It passes messages in with `-F`.

## 7. Progress

- **`--progress`** forces progress on stderr when it is not a terminal. It applies to `fetch`,
  `pull` (passed on to fetch), `push`, `switch`/`checkout`, and `merge` (strategy permitting)
  ([fetch](https://git-scm.com/docs/git-fetch),
  [push](https://git-scm.com/docs/git-push),
  [switch](https://git-scm.com/docs/git-switch),
  [merge](https://git-scm.com/docs/git-merge)).
  Without it, fetch and push print only their summary lines **(tested)**. Rebase has no such
  flag, but still prints `Rebasing (1/2)` **(tested** 2.34.1**)**.
- **Format:** a line is updated in place with `\r`, and ends with `, done.\n`. For example:
  `Counting objects:  25% (1/4)\rCounting objects:  50% (2/4)\r…, done.` and `Receiving
  objects:   3% (355200/11839968), 190.45 MiB | 23.00 MiB/s` **(tested)**. The server's
  progress arrives as `remote: …` lines. With stderr not a terminal, git pads them with spaces
  instead of an ANSI clear-line code
  ([GIT234 `sideband.c` L129](https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/sideband.c#L129)).
  The dialog must treat `\r` as "replace the last line".
- Some meters appear only after `GIT_PROGRESS_DELAY` seconds (default 2)
  ([git(1)](https://git-scm.com/docs/git)).
- `LC_ALL=C` keeps the text in English, which is good for recognising it. It is also passed on
  to hooks, the askpass and pinentry. Whether operations should use `C.UTF-8` instead is open
  **(derived)**.

## 8. Cancelling

Git removes its lock files and temp files on exit and on `SIGINT`, `SIGHUP`, `SIGTERM`,
`SIGQUIT` and `SIGPIPE`
([GIT234 `tempfile.c` L114-115](https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/tempfile.c#L114-L115);
[`sigchain.c` L45-52](https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/sigchain.c#L45-L52)).
`SIGKILL` and `TerminateProcess` give it no chance. Git for Windows' own `kill` says so:
terminating a tree gives processes "no chance of cleaning up … removing .lock files"
([GIT `compat/win32/exit-process.h` L16-23](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/compat/win32/exit-process.h#L16-L23)).
Git for Windows turns a console `CTRL_C_EVENT` into `SIGINT`
([GIT `compat/mingw.c` L3726-3768](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/compat/mingw.c#L3726-L3768)).

Cancelling `git fetch --progress https://github.com/torvalds/linux.git` mid-download **(tested)**:

| How | Result |
|---|---|
| Windows, `Child::kill` (= `TerminateProcess` on `cmd\git.exe`) | The wrapper dies, but **`git.exe`, `git-remote-https` and `index-pack` keep downloading**. The pipes stay open, so the reader thread waited until the tree was killed by hand. Leftover: a 1.2 GB `objects/pack/tmp_pack_*`. |
| Windows, `AttachConsole(child)` + `GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0)` | Exit 130 after 175 ms, **no processes left**. Leftover: `tmp_pack_*` (the partial download). |
| Linux, `SIGKILL` to git | Exit 137, **`git remote-https` keeps running**. |
| Linux, `SIGTERM` to the process group (`setsid`) | Exit 143, no processes left. Leftover: `tmp_pack_*`. |

- **Leftovers.** A `tmp_pack_*` is harmless. Nothing uses it, and `git prune` (run by `gc`)
  deletes `tmp_*` files older than the prune expiry
  ([GIT234 `builtin/prune.c` L122-123](https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/builtin/prune.c#L122-L123)).
  Refs are only updated at the end of a fetch, so a cancelled fetch changes no refs **(tested:**
  no ref or lock was left**)**.
- **Push.** A push that is cancelled is either accepted by the server as a whole or not at all,
  depending on how far it got. The remote-tracking ref is only updated after the server
  accepted it. So after a cancelled push the remote's state is unknown until the next fetch
  **(derived)**.
- **Local operations** (merge, rebase, …) cancelled with a signal leave whatever state git had
  reached. Cancelling them is best not offered, or only while a hook runs **(derived)**.
- **TortoiseGit** sends `GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0)`, waits up to 10 s, then
  kills the process tree with `TerminateProcess`
  ([TGIT `ProgressDlg.cpp` L665-725](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/TortoiseProc/ProgressDlg.cpp#L665-L725)).
- **For parterre (derived):**
  - Unix: start operations with `setsid` (in `pre_exec`) or `process_group(0)`, and cancel with
    `kill(-pgid, SIGTERM)`, with `SIGKILL` to the group after a timeout.
  - Windows: `CTRL_C_EVENT` on the child's console. A GUI process has no console, so it can
    attach to the child's console, ignore the event itself, send it, and detach. That was
    tested in a small Rust program. As a fallback, kill the tree: a Job Object with
    `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` also cleans up if parterre itself dies.
  - Never rely on end-of-file on the pipes alone.
- **Auto maintenance.** fetch, merge, rebase and commit end with `git maintenance run --auto`
  ([GIT234 `run-command.c` L1849-1860](https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/run-command.c#L1849-L1860)).
  When that runs `gc`, on Unix it detaches into its own session with stdio on `/dev/null`, so
  parterre's pipes close. Windows has no `daemonize`, so gc runs in the foreground and the
  operation takes longer
  ([GIT234 `setup.c` L1433-1451](https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/setup.c#L1433-L1451)).

## 9. Locks

- **`GIT_OPTIONAL_LOCKS=0`** only skips *optional* locks, such as `git status` refreshing the
  index ([git(1)](https://git-scm.com/docs/git), since
  2.15). Operations still take every lock they need. The variable doesn't matter for the
  operations in scope. Leave it out of the operation runner so its meaning stays "background
  read", and keep it on parterre's reads so they never block an operation **(derived)**.
- **Git doesn't wait for `index.lock`.** It fails at once:
  `fatal: Unable to create '…/.git/index.lock': File exists. Another git process seems to be
  running in this repository … remove the file manually to continue.` (exit 128; `switch`,
  `cherry-pick` and `rebase` tested on 2.34.1). **But `git merge` in 2.34.1 printed `error:
  Unable to write index.` and `Automatic merge failed; fix conflicts…` (exit 1), and left
  `MERGE_HEAD`, `MERGE_MSG` and `MERGE_MODE` behind without touching any file.** That is a
  phantom merge in progress, which `merge --abort` clears. Git 2.53 fails cleanly with exit 128
  **(tested)**.
- **Ref locks** retry for `core.filesRefLockTimeout` (100 ms), and `packed-refs` for
  `core.packedRefsTimeout` (1 s)
  ([git-config](https://git-scm.com/docs/git-config)).
  With `refs/heads/main.lock` held, `git merge` **wrote the merge commit and staged it, then
  failed to move the branch** (`fatal: update_ref failed for ref 'HEAD': cannot lock ref`). 2.53
  also left `MERGE_HEAD` **(tested** 2.34.1 and 2.53**)**. A held remote-tracking ref lock makes
  fetch report `! … (unable to update local ref)`, exit 1 **(tested)**.
- **Worktrees:** each has its own index (`.git/worktrees/<name>/index.lock`), but refs are
  shared. So two operations in different worktrees of one repository can still collide on a
  branch ref **(derived)**.
- **For parterre (derived):**
  - Run one operation at a time per repository.
  - Don't start an operation while one of parterre's own reads is running in that worktree:
    reads take no locks, but they could see a half-written state.
  - Never delete a lock file on the user's behalf without a warning. Git can't tell a stale
    lock from a live one, and neither can parterre.
  - After any failed operation, re-check for an operation in progress (`MERGE_HEAD`, …) and for
    staged changes.

## 10. Other prompts

- **`GIT_ASK_YESNO` (Git for Windows only).** When deleting or renaming a file fails, for
  example because another program holds it open during checkout, Git for Windows asks `Unlink of
  file '…' failed. Should I try again? (y/n)`. With stdin not a terminal it answers "no" at once.
  If `GIT_ASK_YESNO` is set, it runs that program with the question, and exit 0 means yes
  ([GIT234 `compat/mingw.c` L202](https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/compat/mingw.c#L202);
  [GIT `compat/mingw.c` L213-241](https://github.com/git/git/blob/a018953688f1b10bddf91bff8747068f5f4746a4/compat/mingw.c#L213-L241)).
  TortoiseGit points it at `SshAskPass.exe`, which shows Yes/No. Parterre can do the same with
  its askpass (optional).
- **Help autocorrect and `bisect` prompts** use `git_prompt` without askpass, but none of them
  are in scope ([GIT234 `help.c` L561](https://github.com/git/git/blob/cd3e606211bb1cf8bc57f7d76bab98cc17a150bc/help.c#L561)).
- **TortoiseGit also sets `LC_ALL=C` if unset, and `HOME`**
  ([TGIT `Git.cpp` L2381-2390](https://github.com/TortoiseGit/TortoiseGit/blob/7338078f8ddd924b8cddee35f512f2286072136d/src/Git/Git.cpp#L2381-L2390)).
  Parterre already sets `LC_ALL`.

## 11. Open points for the decisions

1. Whether the askpass overwrites a user's own `GIT_ASKPASS`, `core.askPass` and `SSH_ASKPASS`
   (TortoiseGit: always), and how it reaches the GUI: its own small window, or IPC to the main
   window.
2. Whether operations keep `LC_ALL=C` or use `C.UTF-8` (it matters for hooks and pinentry, not
   for git).
3. Whether Cancel is offered for local operations, or only for fetch, push and pull (and while
   hooks run).
4. How the operation dialog shows a failed signature on revert, which leaves staged changes and
   no operation in progress.
