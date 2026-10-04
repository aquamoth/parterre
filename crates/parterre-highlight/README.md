# parterre-highlight

The syntax colour of [parterre](https://crates.io/crates/parterre), a TortoiseGit-style revision
graph viewer: a file's language from its path, and language-neutral spans per line through
tree-sitter, for the diff and blame windows to colour. The 23 grammars sit behind the `syntax`
feature, and the app runs them in a child process so that a grammar's crash costs the colours
and nothing else.

It is published only because `parterre` depends on it. It is internal to parterre and makes no
stability promises: any release may change its API. Its version always equals parterre's.

GNU General Public License, version 3 only, with the additional terms in
[NOTICE](https://github.com/aquamoth/parterre/blob/main/NOTICE).
