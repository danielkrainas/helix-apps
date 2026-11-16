#!/usr/bin/env python3
"""Check the translations in po/ (CI runs this).

    python3 tools/i18n/check-po.py            check every language in po/LINGUAS
    python3 tools/i18n/check-po.py de fr      check some

For each catalogue: `msgfmt --check` passes, and every translated message
keeps what the app fills in or parses: `{name}` placeholders, printf
conversions, Pango markup tags and entities, one `_` access key if the
original has one, the number of line breaks, and the `;`-terminated list
form of desktop-file keywords. po/LINGUAS and the po/*.po files must
match. Untranslated and fuzzy messages are counted, not errors.
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


def problems(e, s, src):
    out = []
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


def check(lang):
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
        if e["obsolete"] or "fuzzy" in e["flags"]:
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
    if header is not None and "Plural-Forms: nplurals=INTEGER" in header:
        print(f"po/{lang}.po: Plural-Forms is not set")
        errors += 1
    print(f"po/{lang}.po: {stats}")
    return errors


def main():
    with open(os.path.join(PO, "LINGUAS"), encoding="utf-8") as f:
        linguas = [w for line in f if not line.startswith("#") for w in line.split()]
    files = sorted(f[:-3] for f in os.listdir(PO) if f.endswith(".po"))
    errors = 0
    if sorted(linguas) != files:
        print(f"po/LINGUAS lists {sorted(linguas)} but po/ has {files}")
        errors += 1
    for lang in sys.argv[1:] or linguas:
        errors += check(lang)
    if errors:
        print(f"{errors} problem(s)")
        sys.exit(1)


if __name__ == "__main__":
    main()
