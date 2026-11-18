# Website structure

The website introduces Fujimod, hands out the SD card files and documents
every feature. This is the agreed design for the redesign in progress.

## Home page (`/`)

In this order:

1. The personal note for the current release. Its date and the day and
   contributor counts come from GitHub releases and `contributors/`, never
   typed in. "Everyone" shows the contributors on hover and links to
   `/credits`.
2. Three feature reels side by side: shooting tools, image looks and recipes,
   controls and system. Each is a video above a short description whose
   feature names link into the manual and show the feature hover card. The
   first reels cut between existing walkthrough clips; later they are built
   from walkthrough media.
3. Two columns: the release notes on the left, a small SD card on the right
   with a checkbox per camera, a Recipes checkbox with a recipe list, and the
   download button. The SD card states that Fujimod lives on the SD card; it
   does not list the card's files. On narrow screens the SD card comes first.
4. The emulator.
5. The progress view, unchanged.

## Manual (`/docs/manual/`)

The manual home page is an outline of chapters and their pages, without
videos. A page covers one area of the camera, like a page of Fujifilm's own
manual, and may span several menu rows and several mods. Menu rows are
anchors on their page; options never get their own URL.

Chapters follow the order of Fujifilm's manuals, with every setting that
changes how pictures look in one chapter, followed by the chapter on
organizing them:

1. Getting Started
2. Taking Photographs
3. Image Quality
4. Custom Settings (EDIT/SAVE CUSTOM SETTING, CUSTOM SETTINGS BKT,
   DISPOSABLE)
5. Movies
6. Controls and Display
7. System & Storage (updates, BOOT SETUP, backups, SD card, system changes)

Everything under `/docs/` is rendered from generated data files; the website
code holds no manual text, page lists, chapter order or menu structure. Each
feature page opens with a short introduction, then a section per menu row.

The menu index (`/docs/manual/menus/`) lists every menu row Fujimod adds or
changes, in camera menu order, using the same rows and order as the home
page outline. Each row is a link to its place on the manual page that covers
it. One mapping from menu paths to manual pages drives both the index and
the chapters, and every shown row maps to a page.

## Other pages

- `/safety`: the risks of custom firmware and how Fujimod mitigates them.
- `/credits`: GitHub Contributors (from `contributors/`), Testing & Feedback,
  Recipe Authors (each author with their recipes, linked to each recipe's
  source) and Open Source (generated from a CycloneDX SBOM of everything the
  camera image, website and emulator ship, with each license's full text and
  a link to the source at the shipped version).
- `/releases`: all release notes, read from the GitHub releases at build time.

The personal note and the safety text are written in the website itself.

Text pages (`/safety`, `/credits`, release notes) share one layout with an
outline of their headings beside the text. The manual and the SDK docs share
one docs layout with the chapter sidebar.
