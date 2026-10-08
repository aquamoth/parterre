# PROTOTYPE — throwaway: the macOS About panel

A stand-in for parterre.app that only opens AppKit's standard About panel, as the real app's
*About parterre* item would (`orderFrontStandardAboutPanelWithOptions:`). The panel takes the
name and icon from Info.plist, the version from the options, the copyright from
`NSHumanReadableCopyright` and the text under it from `Credits.html` in Resources.

Build it in a bundle made from `packaging/macos/Info.plist` (`@COPYRIGHT@` = `© 2026 Trustfall AB`)
with `parterre.icns` and `Credits.html` in Resources:
`clang -fobjc-arc -framework AppKit main.m -o parterre.app/Contents/MacOS/parterre`,
then `open parterre.app --args --light` (or `--dark`).
