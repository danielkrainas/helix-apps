#!/bin/sh
# Runs a command (normally `cargo test …`) headless on a private session
# bus that can start no services, with in-memory GSettings:
#
#   tools/testing/run-tests.sh cargo test --workspace
#
# HELIX_TEST_BUS_CHECK makes helix-app-common's test_run_bus_activates_nothing
# check that bus. CI runs the tests this way.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
exec env -u DISPLAY -u WAYLAND_DISPLAY -u DBUS_SESSION_BUS_ADDRESS \
    GSETTINGS_BACKEND=memory HELIX_TEST_BUS_CHECK=1 \
    /usr/bin/dbus-run-session --config-file="$here/session.conf" -- "$@"
