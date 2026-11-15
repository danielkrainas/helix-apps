#!/bin/sh
# Regenerate po/POTFILES and the translation template po/helix-apps.pot.
#
#   tools/i18n/update-pot.sh           rewrite both files
#   tools/i18n/update-pot.sh --check   fail if either is out of date (CI)
#
# Rust sources go through tools/i18n/extract_rs.py (GNU xgettext before
# 0.24 cannot parse Rust); desktop files and AppStream metainfo through
# xgettext, which knows their translatable keys. Needs python3 and the
# GNU gettext tools.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
check=false
[ "${1:-}" = "--check" ] && check=true

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

python3 tools/i18n/potfiles.py > "$tmp/POTFILES"
grep -v '^#' "$tmp/POTFILES" > "$tmp/list"
grep '\.rs$' "$tmp/list" > "$tmp/rs" || true
grep '\.desktop$' "$tmp/list" > "$tmp/desktop" || true
grep '\.metainfo\.xml$' "$tmp/list" > "$tmp/metainfo" || true

# shellcheck disable=SC2046
python3 tools/i18n/extract_rs.py "$tmp/rs.pot" $(cat "$tmp/rs")
xgettext --language=Desktop --from-code=UTF-8 --add-location=file \
    -k -kName -kGenericName -kComment -kKeywords \
    -o "$tmp/desktop.pot" -f "$tmp/desktop"
xgettext --from-code=UTF-8 --add-location=file -o "$tmp/metainfo.pot" -f "$tmp/metainfo"
# Rust messages keep line numbers (they help find the context);
# desktop and metainfo ones are file-only, so regenerating them after an
# unrelated edit does not churn.
msgcat --use-first --sort-by-file "$tmp/rs.pot" "$tmp/desktop.pot" "$tmp/metainfo.pot" \
    | sed '/^"POT-Creation-Date:/d' > "$tmp/helix-apps.pot"

if $check; then
    status=0
    cmp -s "$tmp/POTFILES" po/POTFILES || { echo "po/POTFILES is out of date; run tools/i18n/update-pot.sh" >&2; status=1; }
    # Compare the messages only: line numbers move with every edit.
    strip() { grep -v '^#:' "$1"; }
    strip "$tmp/helix-apps.pot" > "$tmp/new"
    strip po/helix-apps.pot > "$tmp/old"
    cmp -s "$tmp/new" "$tmp/old" || { echo "po/helix-apps.pot is out of date; run tools/i18n/update-pot.sh" >&2; status=1; }
    exit $status
fi

cp "$tmp/POTFILES" po/POTFILES
cp "$tmp/helix-apps.pot" po/helix-apps.pot
echo "po/helix-apps.pot: $(grep -c '^msgid ' po/helix-apps.pot) messages from $(wc -l < "$tmp/list") files"
