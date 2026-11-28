# Translating FujiMod

Translations are managed in [Weblate](https://hosted.weblate.org/). The
project link is added after the maintainer creates the "FujiMod" project and
installs the Hosted Weblate GitHub App for `justjake/fujimod`.

Each language has one `i18n/<locale>.po` file. The locale must be a camera
language in `languages.json`; the camera selects the matching file by its
native language setting. A translation can be submitted while incomplete;
untranslated entries keep the English source visible.

Use `msgctxt` to keep the same English word distinct when it appears in a
different part of the camera interface. Translator comments explain labels
whose meaning is not obvious. Preserve line breaks and any leading or
trailing spaces.

The camera font draws UTF-8 text. Keep menu labels compact because the stock
native rows scale long text down to fit; a translation that does not fit is
still visible but may be smaller than neighboring stock rows.
