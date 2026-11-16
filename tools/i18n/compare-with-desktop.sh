#!/bin/sh
# Shows how this repo's i18n tools differ from helix-desktop's copy, with
# the project names (helix-apps / helix-desktop, helix_app_common /
# helix_i18n) masked, so only real drift is left:
#
#   tools/i18n/compare-with-desktop.sh [PATH-TO-HELIX-DESKTOP]
#
# (default: ../helix-desktop next to this checkout). Some differences are
# intended and listed in TRANSLATING.md ("Tooling shared with
# helix-desktop"); port anything else to the other copy.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
other=${1:-$here/../../../helix-desktop}/tools/i18n
[ -d "$other" ] || { echo "no $other" >&2; exit 2; }
mask() {
    sed -e 's/helix-apps/PROJECT/g; s/helix-desktop/PROJECT/g' \
        -e 's/helix_app_common::i18n/I18N/g; s/helix_app_common/I18N/g; s/helix_i18n/I18N/g' \
        -e 's/helix-app-common/I18N_CRATE/g; s/helix-i18n/I18N_CRATE/g' "$1"
}
status=0
for f in extract_rs.py update-pot.sh potfiles.py check-po.py pseudo.py make-pseudo.sh mark_strings.py; do
    [ -f "$other/$f" ] || { echo "== $f: not in helix-desktop"; continue; }
    tmp=$(mktemp)
    mask "$other/$f" > "$tmp"
    if ! mask "$here/$f" | diff -u --label "helix-apps/$f" --label "helix-desktop/$f" - "$tmp"; then
        status=1
    fi
    rm -f "$tmp"
done
exit $status
