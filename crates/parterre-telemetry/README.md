# parterre-telemetry

What [parterre](https://crates.io/crates/parterre), a TortoiseGit-style revision graph viewer,
asks and sends over the network: for now the update check, which asks GitHub whether a newer
release is out and where this build's channel gets it. The HTTPS client sits behind the `send`
feature; without it the crate makes no requests at all, and the rest of parterre never sees it.

It is published only because `parterre` depends on it. It is internal to parterre and makes no
stability promises: any release may change its API. Its version always equals parterre's.

GNU General Public License, version 3 only, with the additional terms in
[NOTICE](https://github.com/aquamoth/parterre/blob/main/NOTICE).
