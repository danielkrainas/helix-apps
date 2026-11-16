#!/usr/bin/env python3
"""Check the translations in po/ (CI runs this).

    python3 tools/i18n/check-po.py            check every language in po/LINGUAS
    python3 tools/i18n/check-po.py de fr      check some

    python3 tools/i18n/check-po.py --untranslated   also list untranslated
                                                    and fuzzy messages

For each catalogue: `msgfmt --check` passes, the header names its
language and plural rule, and every translated message keeps what the
app fills in or parses: `{name}` placeholders, printf conversions, Pango
markup tags and entities, one `_` access key if the original has one,
the number of line breaks, the `;`-terminated list form of desktop-file
keywords and, in date and time patterns (`%A, %B %e`), the same kinds of
fields (a language may write `%e` as `%-d`, but a 12-hour time keeps its
AM/PM and a date its weekday). po/LINGUAS and the po/*.po files must
match. Untranslated and fuzzy messages are counted, not errors.

Shared with helix-desktop's tools/i18n/check-po.py, which has the same
placeholder, line-break, header and date-pattern checks but no markup,
access-key or desktop-keyword ones: keep the common checks in step (see
"Tooling shared with helix-desktop" in TRANSLATING.md).
"""

import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
PO = os.path.join(ROOT, "po")

PH = re.compile(r"\{[^{}]*\}")
PRINTF = re.compile(r"%(?:\d+\$)?[-+#0]*\d*(?:\.\d+)?[sdifuxXcp%]")
TAG = re.compile(r"</?[a-zA-Z][^<>]*>")
ENT = re.compile(r"&[a-zA-Z]+;|&#\d+;")
MNEMONIC = re.compile(r"_(?=[^\s_{}])")
# A strftime/GLib DateTime conversion: optional padding flag, optional
# E/O modifier, the field letter.
CONV = re.compile(r"%[-_0^#]?[EO]?([A-Za-z%])")
# The kinds of field a date or time pattern shows.
KIND = {
    "H": "hour", "k": "hour", "I": "hour", "l": "hour",
    "M": "minute", "S": "second",
    "p": "ampm", "P": "ampm",
    "a": "weekday", "A": "weekday", "u": "weekday", "w": "weekday",
    "b": "month", "B": "month", "h": "month", "m": "month",
    "d": "day", "e": "day",
    "Y": "year", "y": "year", "C": "year", "G": "year", "g": "year",
}
# Twelve-hour fields: a 12-hour pattern stays one (and a 24-hour one too).
TWELVE = set("Il")
TWENTY_FOUR = set("Hk")


def unquote(s):
    s = s.strip()
    assert s.startswith('"') and s.endswith('"'), s
    out, i, body = [], 0, s[1:-1]
    simple = {"n": "\n", "t": "\t", '"': '"', "\\": "\\", "r": "\r"}
    while i < len(body):
        c = body[i]
        if c == "\\":
            out.append(simple.get(body[i + 1], body[i + 1]))
            i += 2
        else:
            out.append(c)
            i += 1
    return "".join(out)


def parse(path):
    """Yields entries as dicts: ctx, id, plural, str (list), flags, refs."""
    entry, field = None, None

    def done(e):
        return e is not None and "id" in e

    with open(path, encoding="utf-8") as f:
        lines = f.read().split("\n")
    entries = []
    for line in lines + [""]:
        if not line.strip():
            if done(entry):
                entries.append(entry)
            entry, field = None, None
            continue
        if entry is None:
            entry = {"flags": [], "refs": [], "str": {}, "obsolete": False}
        if line.startswith("#~"):
            entry["obsolete"] = True
            continue
        if line.startswith("#,"):
            entry["flags"] += [x.strip() for x in line[2:].split(",")]
            continue
        if line.startswith("#:"):
            entry["refs"] += line[2:].split()
            continue
        if line.startswith("#"):
            continue
        m = re.match(r'(msgctxt|msgid_plural|msgid|msgstr(?:\[(\d+)\])?)\s+(".*")$', line)
        if m:
            kw, idx, val = m.group(1), m.group(2), unquote(m.group(3))
            if kw == "msgctxt":
                field = ("ctx", None)
            elif kw == "msgid":
                field = ("id", None)
            elif kw == "msgid_plural":
                field = ("plural", None)
            else:
                field = ("str", int(idx) if idx is not None else 0)
            if field[0] == "str":
                entry["str"][field[1]] = val
            else:
                entry[field[0]] = val
            continue
        if line.startswith('"') and field:
            val = unquote(line)
            if field[0] == "str":
                entry["str"][field[1]] += val
            else:
                entry[field[0]] += val
            continue
        raise ValueError(f"{path}: cannot parse line: {line!r}")
    return entries


def is_date_pattern(text):
    """A pattern made only of conversions and punctuation ("%A, %B %e")."""
    return bool(CONV.search(text)) and not re.search(r"[A-Za-z]", CONV.sub("", text))


def date_problems(src, s):
    out = []
    letters = CONV.findall(s)
    bad = [c for c in letters if c not in KIND and c != "%"]
    if bad:
        out.append(f"unknown date fields {bad}")
    kinds = lambda t: sorted({KIND[c] for c in CONV.findall(t) if c in KIND})
    if kinds(s) != kinds(src):
        out.append(f"date fields {kinds(s)} instead of {kinds(src)}")
    src_letters = set(CONV.findall(src))
    if src_letters & TWELVE and not set(letters) & TWELVE:
        out.append("a 12-hour time without a 12-hour field (%l or %I)")
    if src_letters & TWENTY_FOUR and not set(letters) & TWENTY_FOUR:
        out.append("a 24-hour time without a 24-hour field (%H or %k)")
    return out


def problems(e, s, src):
    out = []
    date = is_date_pattern(src)
    if date:
        out += date_problems(src, s)
    def keys(rx, t):
        found = rx.findall(t)
        if rx is PH:
            # `{}` takes the next argument; a translation may number them
            # (`{1} … {0}`) to change their order.
            n = iter(range(len(found)))
            found = [re.sub(r"\{\}", lambda m: "{%d}" % next(n), x) for x in found]
        return sorted(found)
    for name, rx in (("placeholders", PH), ("printf conversions", PRINTF),
                     ("markup tags", TAG)):
        if date and rx is PRINTF:
            continue
        if keys(rx, s) != keys(rx, src):
            out.append(f"{name} {keys(rx, s)} instead of {keys(rx, src)}")
    # Markup: entities may become words ("&amp;" → "and"), but a bare "&"
    # would not parse.
    if ENT.search(src) or TAG.search(src):
        if re.search(r"&(?![a-zA-Z]+;|#\d+;)", s):
            out.append("a bare & in markup (write &amp;)")
    data = any(r.split(":")[0].endswith((".desktop", ".metainfo.xml")) for r in e["refs"])
    if not data:
        a = len(MNEMONIC.findall(PH.sub("", src)))
        b = len(MNEMONIC.findall(PH.sub("", s)))
        if a != b:
            out.append(f"{b} access keys (_) instead of {a}")
    if s.count("\n") != src.count("\n"):
        out.append("a different number of line breaks")
    if data and src.endswith(";") and not s.endswith(";"):
        out.append("keywords must end with ;")
    return out


def check(lang, list_untranslated=False):
    path = os.path.join(PO, f"{lang}.po")
    errors = 0
    r = subprocess.run(["msgfmt", "--check", "--statistics", "-o", os.devnull, path],
                       capture_output=True, text=True)
    stats = r.stderr.strip()
    if r.returncode != 0:
        print(stats)
        return 1
    header = None
    for e in parse(path):
        if e["id"] == "" and "ctx" not in e:
            header = e["str"].get(0, "")
            continue
        if e["obsolete"]:
            continue
        if "fuzzy" in e["flags"] or not all(e["str"].values()):
            if list_untranslated:
                ctx = f"[{e['ctx']}] " if "ctx" in e else ""
                print(f"po/{lang}.po: untranslated: {ctx}{e['id']!r}")
            if "fuzzy" in e["flags"]:
                continue
        forms = [e["str"][i] for i in sorted(e["str"])]
        for i, s in enumerate(forms):
            if not s:
                continue
            src = e.get("plural", e["id"])
            probs = problems(e, s, src)
            # A singular form may leave out the count ("One file").
            if probs and i == 0 and "plural" in e and not problems(e, s, e["id"]):
                probs = []
            for p in probs:
                ctx = f"[{e['ctx']}] " if "ctx" in e else ""
                print(f"po/{lang}.po: {ctx}{e['id'][:60]!r}: form {i} has {p}")
                errors += 1
    if header is None:
        print(f"po/{lang}.po: no header")
        errors += 1
    else:
        if "Plural-Forms: nplurals=INTEGER" in header or "Plural-Forms:" not in header:
            print(f"po/{lang}.po: Plural-Forms is not set")
            errors += 1
        if f"Language: {lang}\n" not in header:
            print(f"po/{lang}.po: the header's Language is not {lang}")
            errors += 1
    print(f"po/{lang}.po: {stats}")
    return errors


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    list_untranslated = "--untranslated" in sys.argv
    with open(os.path.join(PO, "LINGUAS"), encoding="utf-8") as f:
        linguas = [w for line in f if not line.startswith("#") for w in line.split()]
    files = sorted(f[:-3] for f in os.listdir(PO) if f.endswith(".po"))
    errors = 0
    if not args and sorted(linguas) != files:
        print(f"po/LINGUAS lists {sorted(linguas)} but po/ has {files}")
        errors += 1
    for lang in args or linguas:
        errors += check(lang, list_untranslated)
    if errors:
        print(f"{errors} problem(s)")
        sys.exit(1)


if __name__ == "__main__":
    main()
