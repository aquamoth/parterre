# parterre-forge

The pull-request client of [parterre](https://crates.io/crates/parterre), a TortoiseGit-style
revision graph viewer: the open pull requests of the GitHub repository a repository's `origin`
points at, and where each is shown in the graph. The HTTPS client sits behind the `github`
feature, so the rest of parterre never sees it.

It is published only because `parterre` depends on it. It is internal to parterre and makes no
stability promises: any release may change its API. Its version always equals parterre's.

GNU General Public License, version 3 only, with the additional terms in
[NOTICE](https://github.com/aquamoth/parterre/blob/main/NOTICE).
