# Screenshots

Each folder holds the screenshots of one version of parterre: `0.6/`, `1.0/`, … (`demo.png`
is from before 0.6). Never move, change or delete a file here, not even an old one: published
releases link to them.

## Why a new folder each time

A release can't be changed once it is out, and some of what it publishes shows pictures from
this folder by a fixed address on `main`:

- the **crates.io** page of each published version shows its `crates/parterre/README.md`, with
  the main screenshot as `https://raw.githubusercontent.com/aquamoth/parterre/main/docs/images/<version>/…`;
- the **AppStream metadata** in each `.deb` and `.rpm`
  (`packaging/linux/se.trustfall.parterre.metainfo.xml`) lists screenshots by the same kind of
  address, for software centres.

Replace `1.0/hero-light.png` with a newer picture, and every published version shows the new
UI as if it were its own. Delete or move it, and their pages show a broken image. A new folder
leaves each release with the pictures of what it shipped.

## New screenshots

1. Take them, of the demo repository (`scripts/readme-demo-repo.py`):

   ```sh
   scripts/readme-images.sh /tmp/parterre-readme
   ```

   That builds the debug binary and takes every image, light and dark, at 2x, under
   `xvfb-run`, in about a minute. On the same commit it makes the same files. Look for egui's
   red id-clash warnings, which only debug builds show. If the UI has changed so that a step no
   longer finds its target, the run says which, and the script is the place to fix it.
2. Copy them into a new folder named for the version they show, `docs/images/<major>.<minor>/`,
   with the platform and theme icons (`platform-*.svg`, `theme-*.svg`) of the last folder.
3. Point all three at it: `README.md`, `crates/parterre/README.md` and the metainfo.
   `grep -rn 'docs/images/' --include='*.md' --include='*.xml' .` finds every link.

The README shows each picture through `<picture>`, so GitHub picks the reader's theme; the icons
under the first one open each theme's version.
