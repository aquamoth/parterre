# parterre-stamp

What the release pipeline runs on a built parterre binary (#409, docs/releasing.md):

```sh
parterre-stamp version --ring canary|nightly|rc|stable [--date YYYYMMDD] [--rev REV]
parterre-stamp write BINARY VERSION
parterre-stamp read BINARY
```

- `version` prints the version a build of `REV` (default `HEAD`) is published under in a
  ring, from the git tags of the repository it runs in: `1.2.3-canary+a1b2c3d`.
- `write` writes `VERSION` into the binary's version slot, and on Windows into its version
  resource too, in place: nothing moves, so the debug IDs stay as the linker wrote them. It
  checks that every byte it changed lies inside those. A Mac binary must be signed again
  afterwards (`codesign --force --sign -`).
- `read` prints the version in the slot, and `written` if it was written after the build.

Standard library only, and not published: it shares `version.rs` and `slot.rs` with
`crates/parterre`.
