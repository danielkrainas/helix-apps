#!/bin/sh
# Load the AppArmor profiles that let Helix Web and Help start WebKit's
# sandbox on systems that restrict unprivileged user namespaces.
if command -v apparmor_parser >/dev/null 2>&1 && [ -d /sys/kernel/security/apparmor ]; then
    for p in helix-web helix-help; do
        [ -f "/etc/apparmor.d/$p" ] && apparmor_parser -r -T -W "/etc/apparmor.d/$p" || true
    done
fi
# The app icons went into hicolor.
if command -v gtk-update-icon-cache >/dev/null 2>&1 && [ -f /usr/share/icons/hicolor/index.theme ]; then
    gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
exit 0
