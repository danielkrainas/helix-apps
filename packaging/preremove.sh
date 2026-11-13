#!/bin/sh
# Unload the AppArmor profiles before their files go away (not on upgrade).
case "${1:-}" in
    upgrade|1) exit 0 ;;  # deb "upgrade", rpm count 1 = upgrade
esac
if command -v apparmor_parser >/dev/null 2>&1 && [ -d /sys/kernel/security/apparmor ]; then
    for p in helix-web helix-help; do
        [ -f "/etc/apparmor.d/$p" ] && apparmor_parser -R "/etc/apparmor.d/$p" 2>/dev/null || true
    done
fi
exit 0
