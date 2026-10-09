# parterre

Every branch, worktree and pull request of a git repository in one picture, and the git
operations to act on them. A fast, native revision graph for Linux, Windows and macOS,
inspired by TortoiseGit's **Revision Graph**: it shows how your branches, tags, remotes,
worktrees and open GitHub pull requests relate, and lets you rearrange the graph by hand.

![parterre showing a repository with branches, tags, worktrees and pull requests](https://raw.githubusercontent.com/aquamoth/parterre/main/docs/images/1.0/hero-light.png)

```sh
cargo install --locked parterre    # build from source
cargo binstall parterre            # or download the release binary
parterre [PATH]                    # show the repository containing PATH (default: .)
```

parterre needs `git` on `PATH` at runtime; everything it reads and changes, it does with git.
`cargo install` installs only the binary, without a desktop entry or icon; installers and
packages are on the [releases page](https://github.com/aquamoth/parterre/releases).

Features are in the [README on GitHub](https://github.com/aquamoth/parterre#readme), and usage,
keys and options in the [user guide](https://github.com/aquamoth/parterre/blob/main/docs/usage.md).

## License

parterre is free software under the GNU General Public License, version 3 only, with two
additional terms in [NOTICE](https://github.com/aquamoth/parterre/blob/main/NOTICE): works
based on parterre keep its copyright notice and say that they are based on it, and modified
versions are marked as modified.
