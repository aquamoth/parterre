#!/bin/sh
# Installs a .deb or .rpm with the system's package manager, as a user would, checks that
# parterre runs and that its desktop files are in place, and removes it again. For CI's
# containers (Debian, Ubuntu, Fedora, openSUSE), run as root.
#
#   packaging/linux/test-package.sh PACKAGE
set -eu

pkg=$(realpath "$1")
id=se.trustfall.parterre
case $pkg in
*.deb)
    apt-get update -qq
    apt-get install -y -qq "$pkg"
    ;;
*.rpm)
    if command -v zypper >/dev/null; then
        zypper --non-interactive install --allow-unsigned-rpm "$pkg"
    else
        dnf install -y "$pkg"
    fi
    ;;
*)
    echo "not a .deb or .rpm: $pkg" >&2
    exit 2
    ;;
esac

# With every dependency the package declares, but no display: --version doesn't open one. (Not
# /usr/share/doc: container images leave out documentation.)
version=$(parterre --version)
echo "$version"
case $version in
"parterre "*) ;;
*) echo "unexpected --version output: $version" >&2; exit 1 ;;
esac
command -v git
for f in \
    /usr/share/applications/$id.desktop \
    /usr/share/metainfo/$id.metainfo.xml \
    /usr/share/icons/hicolor/scalable/apps/$id.svg \
    /usr/share/icons/hicolor/256x256/apps/$id.png \
    /usr/share/kio/servicemenus/$id.desktop \
    /usr/share/nemo/actions/$id.nemo_action \
    /usr/share/nemo/actions/$id-background.nemo_action \
    /usr/share/nautilus-python/extensions/parterre.py; do
    test -f "$f" || { echo "missing: $f" >&2; exit 1; }
done
if command -v desktop-file-validate >/dev/null; then
    desktop-file-validate /usr/share/applications/$id.desktop
fi
if command -v appstreamcli >/dev/null; then
    appstreamcli validate --no-net /usr/share/metainfo/$id.metainfo.xml
fi

case $pkg in
*.deb) apt-get remove -y -qq parterre ;;
*.rpm) if command -v zypper >/dev/null; then zypper --non-interactive remove parterre; else dnf remove -y parterre; fi ;;
esac
if [ -e /usr/bin/parterre ]; then
    echo "/usr/bin/parterre is still there after removing the package" >&2
    exit 1
fi
echo "installed, ran and removed $pkg"
