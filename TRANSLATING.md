# Translating the Helix apps

All apps share one gettext domain, `helix-apps`, and one template,
`po/helix-apps.pot`. A string that several apps use ("Cancel",
"Keyboard Shortcuts", "About …") is translated once.

## Languages

| Code | Language | Status |
| --- | --- | --- |
| `de` | German | machine-translated, unreviewed |
| `es` | Spanish | machine-translated, unreviewed |
| `fr` | French | machine-translated, unreviewed |
| `it` | Italian | machine-translated, unreviewed |
| `ja` | Japanese | machine-translated, unreviewed |
| `ko` | Korean | machine-translated, unreviewed |
| `nl` | Dutch | machine-translated, unreviewed |
| `pl` | Polish | machine-translated, unreviewed |
| `pt_BR` | Portuguese (Brazil) | machine-translated, unreviewed |
| `ru` | Russian | machine-translated, unreviewed |
| `uk` | Ukrainian | machine-translated, unreviewed |
| `zh_CN` | Chinese (Simplified) | machine-translated, unreviewed |

The first catalogues were machine-translated and shipped without review
(their header says `Last-Translator: Machine translation (unreviewed)`).
If you are a fluent speaker, reports of wrong or awkward translations
are welcome as issues, and so is a review of a whole catalogue: correct
`po/<lang>.po`, put your name in its `Last-Translator` header, and change
the language's status here to "reviewed".

## Adding a language

You need the GNU gettext tools (`msginit`, `msgmerge`, `msgfmt`).

1. Create the catalogue from the template, for example for German:

       msginit -i po/helix-apps.pot -o po/de.po -l de_DE.UTF-8

   `msginit` fills in the language and its plural rule.
2. Add the language code (`de`) on its own line to `po/LINGUAS`.
3. Translate `po/de.po` with a PO editor (Poedit, GNOME Translation
   Editor, Lokalize) or a text editor.
4. Check it: `python3 tools/i18n/check-po.py de` (CI runs it for every
   language). It runs `msgfmt --check` and makes sure each translation
   keeps the original's placeholders, markup and access key.
5. Add the language to the table above.

`packaging/stage.sh` compiles every language listed in `po/LINGUAS` to
`/usr/share/locale/<lang>/LC_MESSAGES/helix-apps.mo` and merges the
translated names, comments and keywords into the installed desktop
files and the AppStream metainfo.

## Updating a translation

When the sources change, regenerate the template and merge it:

    tools/i18n/update-pot.sh
    msgmerge --update --backup=none po/de.po po/helix-apps.pot

## Notes for translators

- `{name}` and `{}` are placeholders filled in by the app. Keep them;
  you may move them. Numbered ones (`{0}`, `{1}`) can be reordered, and
  plain `{}` may be numbered in order of appearance in the English
  (`{1} … {0}`) to change their order.
- `_` marks the access key of a menu item or button (`_Open`). Keep one
  `_` in the translation, before a letter that is easy to type.
- Some messages contain Pango markup (`<b>…</b>`, `&amp;`). Keep the
  tags and entities well-formed.
- Desktop-file `Keywords` are a `;`-separated list that must end with
  `;`. Add synonyms people would search for in your language.
- Comments that start with `Translators:` explain a message's context.
- Plurals: messages with `msgid_plural` need every form of your
  language's `Plural-Forms`.

## Trying a translation without installing it

    msgfmt -o /tmp/locale/de/LC_MESSAGES/helix-apps.mo po/de.po   # after mkdir -p
    HELIX_LOCALEDIR=/tmp/locale LANGUAGE=de LANG=de_DE.UTF-8 helix-calculator

`tools/i18n/make-pseudo.sh DIR` builds an `en@pseudo` catalogue in which
every message is accented and bracketed (`⟦Ķéýƀöàŕđ Šĥöŕţçûţš⟧`); run an
app with `HELIX_LOCALEDIR=DIR LANGUAGE=en@pseudo LANG=en_US.UTF-8` to
spot text that is not marked for translation and layouts that do not
stretch.

## For developers

Mark every user-visible string, including tooltips, accessible labels,
toasts and announcements, with the functions in
`helix_app_common::i18n` (`crates/helix-app-common/src/i18n.rs`):

| Call | Use |
| --- | --- |
| `tr("Text")` | A literal; returns `&'static str`. |
| `trf!("Open {name}", name = n)` | A message with placeholders. |
| `trnf!("{n} file", "{n} files", count)` | A count; never build plurals with `if n == 1`. |
| `trc("context", "Text")` | Short text that needs a different translation elsewhere (returns a `String`). |
| `tr(nc_!("context", "Text"))` | The same where a `&'static str` is needed. |
| `n_("Text")` | Marks text in a `const`/`static` table; call `tr` where it is shown. |
| `nc_!("context", "Text")` | `n_` with a context; `tr` shows it without the context. |

All apps share one catalogue, so a message is translated once for every
place it appears. Give a message a context (`trc`, `nc_!`) when it is a
word or two that could be read in more than one sense, or that other
languages inflect differently by what it refers to: "Home" (the home
folder, a home page, a home phone number), "Other", "None", "Normal",
"From"/"To", a list separator `", "`. Name the sense in the context
("phone number type", "home folder", "list separator"), and add a
`// Translators:` comment on the line above when the context alone
does not say what the message means. Labels made with `nc_!` carry
their context: compare them with each other, not with a literal.

Every app calls `helix_app_common::i18n::init()` first thing in `main`.
Write whole sentences with placeholders rather than gluing pieces
together, and use `i18n::month_name` (a month on its own) or
`month_name_in_date` (next to a day number, where Russian, Polish and
Ukrainian inflect it), `weekday_name`, `format_number`,
or GLib's `DateTime::format` instead of hand-written names and number
formats (chrono's `%A`, `%B` and `%p` are English in every locale).
Put the order of a date's parts in a translatable message
(`trf!("{weekday}, {day} {month} {year}", …)`), and translate a label
from an `n_` table with `tr` everywhere it is shown, including
accessible labels. Never translate text that is parsed or compared
(`/proc` keys, key names, protocol values). Use `Start`/`End` alignment; for label justification use
`i18n::justify_start`/`justify_end`, and in CSS give right-to-left
variants (`:dir(rtl)`) for one-sided borders and padding.

Library errors (zbus, GLib/GIO, `std::io`) are English and technical:
show them through `helix_app_common::errors` (`detail`, `glib_detail`,
`io_detail`, or `alert` for a dialog with the raw text under "Details"),
which turns the common kinds (not found, permission denied, timed out, a
service that is not running, cancelled, no space, …) into translated
sentences and logs the raw text.

`tools/i18n/update-pot.sh` regenerates `po/POTFILES` and the template;
CI runs it with `--check`, and a unit test checks that `po/POTFILES`
lists every source file with messages. `tools/i18n/mark_strings.py`
is the heuristic marker used to convert the existing code; it can mark
new code in bulk, but review its changes.
Code in `#[cfg(test)] mod … { … }` blocks (wherever they are in a file),
`tests/` and `examples/` directories and test-support crates is not
extracted, so test-only strings never reach the template;
`tools/i18n/test_extract_rs.py` (in CI) checks this.

## Tooling shared with helix-desktop

[helix-desktop](https://github.com/danielkrainas/helix-desktop) has a
copy of these tools in its own `tools/i18n/` for its `helix-desktop`
domain. The two copies are kept in step by hand:

| Shared | What differs on purpose |
| --- | --- |
| `extract_rs.py` | the i18n crate it documents (`helix_app_common::i18n` / `helix-i18n`), `nc_!` (apps only), the template header |
| `update-pot.sh` | the template name; apps also extract AppStream metainfo |
| `potfiles.py` | which directories and data files are listed |
| `check-po.py` | apps also check markup, access keys and desktop keywords |
| `pseudo.py`, `make-pseudo.sh` | the template name |
| `mark_strings.py` | each repo's helper names and crate path |

When you change one of these files, make the same change in the other
repository (or open an issue there), and say so in the commit.
`tools/i18n/compare-with-desktop.sh [PATH]` diffs the two copies with the
project names masked, so only real drift shows.
