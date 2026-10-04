# parterre-util

The cancellation handles of [parterre](https://crates.io/crates/parterre), a TortoiseGit-style
revision graph viewer: what stops a git command or a syntax-colouring child process running on
another thread, shared by the crates that run them. Standard library only.

It is published only because `parterre` depends on it. It is internal to parterre and makes no
stability promises: any release may change its API. Its version always equals parterre's.

GNU General Public License, version 3 only, with the additional terms in
[NOTICE](https://github.com/aquamoth/parterre/blob/main/NOTICE).
