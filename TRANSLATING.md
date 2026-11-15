# Translating the Helix apps

All apps share one gettext domain, `helix-apps`, and one template,
`po/helix-apps.pot`. A string that several apps use ("Cancel",
"Keyboard Shortcuts", "About …") is translated once.

## Adding a language

You need the GNU gettext tools (`msginit`, `msgmerge`, `msgfmt`).

1. Create the catalogue from the template, for example for German:

       msginit -i po/helix-apps.pot -o po/de.po -l de_DE.UTF-8

   `msginit` fills in the language and its plural rule.
2. Add the language code (`de`) on its own line to `po/LINGUAS`.
3. Translate `po/de.po` with a PO editor (Poedit, GNOME Translation
   Editor, Lokalize) or a text editor.
4. Check it: `msgfmt --check --statistics -o /dev/null po/de.po`.

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
  you may move them. Numbered ones (`{0}`, `{1}`) can be reordered.
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
| `trc("context", "Text")` | Short text that needs a different translation elsewhere. |
| `n_("Text")` | Marks text in a `const`/`static` table; call `tr` where it is shown. |

Every app calls `helix_app_common::i18n::init()` first thing in `main`.
Write whole sentences with placeholders rather than gluing pieces
together, and use `i18n::month_name`, `weekday_name`, `format_number`,
or GLib's `DateTime::format` instead of hand-written names and number
formats. Use `Start`/`End` alignment; for label justification use
`i18n::justify_start`/`justify_end`, and in CSS give right-to-left
variants (`:dir(rtl)`) for one-sided borders and padding.

`tools/i18n/update-pot.sh` regenerates `po/POTFILES` and the template;
CI runs it with `--check`, and a unit test checks that `po/POTFILES`
lists every source file with messages. `tools/i18n/mark_strings.py`
is the heuristic marker used to convert the existing code; it can mark
new code in bulk, but review its changes.
