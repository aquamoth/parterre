#!/bin/sh
# Installs parterre for the current user: the binary into ~/.local/bin, the desktop entry and
# icon where GNOME, KDE and the others find them, and Revision Graph in the context menus of
# Nautilus, Dolphin and Nemo (restart the file manager to see it; Nautilus needs
# nautilus-python). Run it after `cargo build --release`, or give it the path of a release
# binary.
#
#   packaging/linux/install.sh [BINARY]     install (default: target/release/parterre)
#   packaging/linux/install.sh --uninstall  remove all of it again
#
# The entry and the menus get the absolute path of the installed binary: a desktop loads an
# entry only if its Exec can be found, and ~/.local/bin is often not on the session's PATH. On Wayland the
# entry is the only source of a window's icon, so a running parterre shows it after a restart.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
bin=$HOME/.local/bin/parterre
id=se.trustfall.parterre
entry=$data/applications/$id.desktop
hicolor=$data/icons/hicolor
fm=$here/file-managers
dolphin=$data/kio/servicemenus/$id.desktop
nemo=$data/nemo/actions/$id.nemo_action
nemo_background=$data/nemo/actions/$id-background.nemo_action
nautilus=$data/nautilus-python/extensions/parterre.py
sizes="16 24 32 48 64 128 256 512"

refresh() {
    # Desktops trust an existing icon cache over the directory, so bring it up to date.
    if [ -f "$hicolor/icon-theme.cache" ] && command -v gtk-update-icon-cache >/dev/null; then
        gtk-update-icon-cache -f -t "$hicolor"
    fi
    if [ -d "$data/applications" ] && command -v update-desktop-database >/dev/null; then
        update-desktop-database "$data/applications"
    fi
}

# Up to 0.5 the entry and the icon were called parterre.
remove_old_names() {
    rm -f "$data/applications/parterre.desktop" "$hicolor/scalable/apps/parterre.svg"
    for s in $sizes; do rm -f "$hicolor/${s}x${s}/apps/parterre.png"; done
}

if [ "${1-}" = --uninstall ]; then
    rm -f "$bin" "$entry" "$hicolor/scalable/apps/$id.svg" \
        "$dolphin" "$nemo" "$nemo_background" "$nautilus"
    for s in $sizes; do rm -f "$hicolor/${s}x${s}/apps/$id.png"; done
    remove_old_names
    refresh
    echo "removed $bin, $entry, the icon and the file managers' menu items"
    exit 0
fi

src=${1:-$root/target/release/parterre}
if [ ! -x "$src" ]; then
    echo "no binary at $src; run 'cargo build --release' first" >&2
    exit 1
fi
install -Dm755 "$src" "$bin"
mkdir -p "$data/applications"
sed "s|^Exec=parterre |Exec=$bin |" "$here/$id.desktop" > "$entry"
mkdir -p "$(dirname "$dolphin")" "$(dirname "$nemo")" "$(dirname "$nautilus")"
sed "s|^Exec=parterre |Exec=$bin |" "$fm/dolphin.desktop" > "$dolphin"
# KDE runs a service menu of the user's own only if it is executable.
chmod 755 "$dolphin"
sed "s|^Exec=parterre |Exec=$bin |; s|^Dependencies=parterre;|Dependencies=$bin;|" \
    "$fm/nemo-folder.nemo_action" > "$nemo"
sed "s|^Exec=parterre |Exec=$bin |; s|^Dependencies=parterre;|Dependencies=$bin;|" \
    "$fm/nemo-background.nemo_action" > "$nemo_background"
install -m644 "$fm/nautilus.py" "$nautilus"
install -Dm644 "$root/packaging/icon/parterre.svg" "$hicolor/scalable/apps/$id.svg"
for s in $sizes; do
    install -Dm644 "$root/packaging/icon/parterre-$s.png" "$hicolor/${s}x${s}/apps/$id.png"
done
remove_old_names
refresh
echo "installed $bin, $entry, the icon and the file managers' menu items"
