"""Revision Graph in the context menu of Nautilus (GNOME Files) for a folder and for the
background of an open one. Installed in /usr/share/nautilus-python/extensions, where Nautilus
loads it when nautilus-python (python3-nautilus) is installed; Nautilus has no other way to add
to its menus. Starts parterre through its desktop entry, as the Applications menu would.
"""

import gi

# Nautilus 43 and later; 42 (Ubuntu 22.04) has the older API, which passes the window first.
try:
    gi.require_version("Nautilus", "4.0")
except ValueError:
    gi.require_version("Nautilus", "3.0")
from gi.repository import GObject, Gio, Nautilus  # noqa: E402

APP_ID = "se.trustfall.parterre"


class RevisionGraph(GObject.GObject, Nautilus.MenuProvider):
    def get_file_items(self, *args):
        files = args[-1]
        if len(files) != 1 or not files[0].is_directory():
            return []
        return self._items("File", files[0])

    def get_background_items(self, *args):
        return self._items("Background", args[-1])

    def _items(self, where, folder):
        app = Gio.DesktopAppInfo.new(f"{APP_ID}.desktop")
        location = folder.get_location()
        # Only folders on this machine: parterre needs a path to run git in.
        if app is None or location.get_path() is None:
            return []
        item = Nautilus.MenuItem(
            name=f"Parterre::RevisionGraph{where}",
            label="Revision Graph",
            tip="Show the revision graph of this folder's repository",
            icon=APP_ID,
        )
        item.connect("activate", lambda _item: app.launch([location], None))
        return [item]
