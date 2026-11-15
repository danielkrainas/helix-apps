#!/bin/sh
# Copy nfpm.yaml from stdin to stdout, replacing the "# @LOCALES@" line
# with one contents entry per compiled message catalogue in STAGE.
#
#   packaging/nfpm-locales.sh STAGE < nfpm.yaml > out.yaml
set -eu
stage=${1:?usage: packaging/nfpm-locales.sh STAGE}
entries=$(cd "$stage" && find usr/share/locale -name 'helix-apps.mo' 2>/dev/null | sort |
    while read -r f; do
        printf '  - src: ./stage/%s\n    dst: /%s\n' "$f" "$f"
    done)
while IFS= read -r line; do
    case $line in
        *"# @LOCALES@"*) [ -n "$entries" ] && printf '%s\n' "$entries" ;;
        *) printf '%s\n' "$line" ;;
    esac
done
