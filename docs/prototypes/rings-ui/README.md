# PROTOTYPE: rings in Settings, the new-release dialog, About and the Help menu (#401)

Throwaway. Three variants in the real app, behind `PARTERRE_PROTOTYPE_RINGS=A|B|C`
(`crates/parterre/src/app/prototype_rings.rs`); `PARTERRE_PROTOTYPE_SCENARIO=1..5` picks the
scenario. A bar at the bottom switches both (F8 / F9, Shift back) and opens the dialog and
Settings.

    PARTERRE_PROTOTYPE_RINGS=A cargo run -p parterre -- <repo>

- A: a row under *Check for updates* ("Tell me about": a dropdown; canary listed only once
  chosen or with Shift held). The dialog as today plus "A nightly build."
- B: a section of explained choices (radio list, "this copy" marked). The dialog titled by
  the ring, with the build date, and "You hear of … · Change".
- C: the ring shown with the version: a ring badge, "Hearing of … · Change…" opening a small
  chooser. A compact dialog with the badge; "Download the newer nightly build".

`variant-A.png`, `variant-B.png`, `variant-C.png`: scenarios 2 (rc copy offered stable), 3
(nightly copy offered the next nightly) and 5 (installed for all users) × Settings, dialog,
About, Help menu.
