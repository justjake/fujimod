# FujiMod translations

FujiMod's camera UI strings are translated with [Weblate](https://hosted.weblate.org/).
The English sources live in `fujimod.pot`; each `<locale>.po` file is one
camera language.

Translators work in Weblate and submit changes as pull requests. The Hosted
Weblate project is [FujiMod](https://hosted.weblate.org/projects/fujimod/).
Keep the English source strings in the C declarations and search them in
`fujimod.pot`; the source text is the translation key, so changing it creates
a new entry.

Translation files use GNU gettext PO catalogs. Their `msgctxt` identifies the
kind of camera text (`menu.label`, `menu.page`, `menu.choice`, or `focal.label`),
and translator comments explain where an ambiguous label appears.

## Repository checks

```sh
python i18n/check.py
python i18n/extract.py --check
```

`check.py` validates the catalogs and rejects a locale that does not match a
camera language. `extract.py` checks marked source strings against the
English catalog when passed `--sources`; use it before adding or removing a
translation key.

The mod build consumes these public catalogs, validates that every
translation entry has a source, and emits its compact on-camera table. No
firmware data is part of this directory.
