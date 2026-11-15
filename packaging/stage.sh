#!/bin/sh
# Lay out an installed helix-apps under DESTDIR, as the packages and the
# release tarball ship it.
#
#   packaging/stage.sh DESTDIR
#
# Binaries come from $BIN_DIR (default target/release). Needs jq, cargo,
# glib-compile-schemas and GNU gettext (msgfmt).
#
# Where each app's data/ files go:
#   *-autostart.desktop      /etc/xdg/autostart/<id>.desktop
#   *.desktop                /usr/share/applications/
#   *.service                /usr/share/dbus-1/services/
#   *.policy                 /usr/share/polkit-1/actions/
#   apparmor.d/*             /etc/apparmor.d/
#   *.gschema.xml            /usr/share/helix-apps/schemas/ (compiled there)
#   *.metainfo.xml           /usr/share/metainfo/
# Desktop files and metainfo are merged with the translations in po/ on
# the way (msgfmt --desktop / --xml), and each language in po/LINGUAS is
# compiled to /usr/share/locale/<lang>/LC_MESSAGES/helix-apps.mo.
#   icons/*-symbolic.svg     /usr/share/icons/hicolor/symbolic/apps/
#   icons/*.svg              /usr/share/icons/hicolor/scalable/apps/
# The schemas are Helix's copies of GNOME app schemas (org.gnome.clocks, …).
# They stay out of /usr/share/glib-2.0/schemas so they never conflict with
# the GNOME packages that own those files; helix-app-common looks there
# when the system has no schema of that id.
set -eu

dest=${1:?usage: packaging/stage.sh DESTDIR}
bin=${BIN_DIR:-target/release}
root=$(cd "$(dirname "$0")/.." && pwd)
po="$root/po"

# Installs desktop file or metainfo $1 as $2 with its translations.
install_translated() {
    install -d "$(dirname "$2")"
    case $1 in
        *.desktop) msgfmt --desktop --template="$1" -d "$po" -o "$2" ;;
        *) msgfmt --xml --template="$1" -d "$po" -o "$2" ;;
    esac
    chmod 644 "$2"
}

# Every binary target in the workspace.
bins=$(cd "$root" && cargo metadata --format-version 1 --no-deps --locked |
    jq -r '.packages[].targets[] | select(.kind | index("bin")) | .name' | sort)
[ -n "$bins" ] || { echo "no binaries found" >&2; exit 1; }

for b in $bins; do
    case $b in
        # Run through pkexec only; the polkit policy names this path.
        helix-system-monitor-helper)
            install -Dm755 "$bin/$b" "$dest/usr/libexec/helix-system-monitor/$b" ;;
        *)
            install -Dm755 "$bin/$b" "$dest/usr/bin/$b" ;;
    esac
done

for data in "$root"/apps/*/data; do
    for f in "$data"/*; do
        [ -f "$f" ] || continue
        name=$(basename "$f")
        case $name in
            *-autostart.desktop)
                install_translated "$f" "$dest/etc/xdg/autostart/${name%-autostart.desktop}.desktop" ;;
            *.desktop)
                install_translated "$f" "$dest/usr/share/applications/$name" ;;
            *.service)
                install -Dm644 "$f" "$dest/usr/share/dbus-1/services/$name" ;;
            *.policy)
                install -Dm644 "$f" "$dest/usr/share/polkit-1/actions/$name" ;;
            *.gschema.xml)
                install -Dm644 "$f" "$dest/usr/share/helix-apps/schemas/$name" ;;
            *.metainfo.xml)
                install_translated "$f" "$dest/usr/share/metainfo/$name" ;;
            *)
                echo "stage.sh: don't know where $f goes" >&2; exit 1 ;;
        esac
    done
    if [ -d "$data/icons" ]; then
        for f in "$data"/icons/*.svg; do
            case $f in
                *-symbolic.svg) size=symbolic ;;
                *) size=scalable ;;
            esac
            install -Dm644 "$f" "$dest/usr/share/icons/hicolor/$size/apps/$(basename "$f")"
        done
    fi
    if [ -d "$data/apparmor.d" ]; then
        for f in "$data"/apparmor.d/*; do
            install -Dm644 "$f" "$dest/etc/apparmor.d/$(basename "$f")"
        done
    fi
done

glib-compile-schemas --strict "$dest/usr/share/helix-apps/schemas"

# Message catalogues (see TRANSLATING.md).
for lang in $(sed 's/#.*//' "$po/LINGUAS"); do
    install -d "$dest/usr/share/locale/$lang/LC_MESSAGES"
    msgfmt --check -o "$dest/usr/share/locale/$lang/LC_MESSAGES/helix-apps.mo" "$po/$lang.po"
done

install -Dm644 "$root/LICENSE" "$dest/usr/share/licenses/helix-apps/LICENSE"
install -Dm644 "$root/README.md" "$dest/usr/share/doc/helix-apps/README.md"
