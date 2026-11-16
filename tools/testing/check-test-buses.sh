#!/bin/sh
# Fails when test code or a test script starts a D-Bus daemon other than
# through helix_app_common::test_bus (Rust) or with
# tools/testing/session.conf (scripts): any other bus may load the system's
# service files and start real services by activation. A script's
# `dbus-run-session`/`dbus-daemon` line must pass `--config-file=` and the
# script must name tools/testing (where the configuration lives).
set -eu
cd "$(dirname "$0")/../.."
bad=$(grep -rnE 'dbus-daemon|dbus-run-session' apps crates tools \
        --include='*.rs' --include='*.sh' --include='*.py' 2>/dev/null |
    grep -vE '^[^:]+:[0-9]+:\s*(//|#)' |
    grep -v '^crates/helix-app-common/src/test_bus.rs:' |
    grep -v '^tools/testing/' |
    while IFS= read -r line; do
        file=${line%%:*}
        case $line in
            *--config-file=*) grep -q 'tools/testing' "$file" && continue ;;
        esac
        printf '%s\n' "$line"
    done)
if [ -n "$bad" ]; then
    echo "Test buses must come from helix_app_common::test_bus or use"
    echo "tools/testing/session.conf (no activatable services):"
    echo "$bad"
    exit 1
fi
echo "test buses: ok"
