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
   feature names link into the manual and show the feature hover card.

   A reel is one continuous recording of a short scripted story on the
   emulated camera, not a cut of per-feature walkthroughs. It sets a few
   settings, takes a picture and shows the result; it never steps through a
   whole option list. As the story moves to a feature, a caption naming it
   appears over the video. Captions are timed cues recorded by the reel's
   walkthrough with the video and drawn by the site over it.

   A reel is edited from its recording by what each moment shows:

   - Holds: a state the viewer must read (a picker with the chosen option
     highlighted, the row showing its new value, the result in live view or
     playback) stays on screen at least 1.2 s.
   - Cuts: navigation between features (scrolling menu pages, switching
     tabs, opening menus to reach a row) is cut out. The video cuts from the
     last hold to the first frame of the next feature's own screen.
   - Waits: idle time (live view settling, a menu opening, a page that is not
     the subject) is trimmed to at most 0.3 s.
   - Choices in a list scroll at a readable pace, about 0.15 s per row, and
     hold on the chosen option before it is confirmed.
   - Dial turns show several detents, about 0.4 s apart, with the value
     indicator visible.

   A caption appears at the cut into a feature's screen and stays until the
   next feature's cut. It is a title in the top left of the video, not a
   subtitle.

   - Shooting tools: ASPECT RATIO to 65:24, PHOTOMETRY to HIGHLIGHT, IMAGE
     QUALITY to FINE+RAW, zoom in with the digital teleconverter, shoot, and
     playback shows the zoomed 65:24 picture.
   - Image looks and recipes: pick a recipe from the recipe library by name,
     turn on HALATION under FILM EFFECTS, shoot, and playback shows the look.
   - Controls and system: assign ISO to the EV dial under UNLOCKED CONTROLS,
     turn the dial in live view to change ISO, then open CHECK FOR UPDATES.
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

A page says each fact once. The page's menu path appears once under its
title, and its cameras once at the top; a section shows cameras only when they
differ ("X70 only"). The page's own row is not a section: its summary and
options join the introduction, and its summary is left out when the page has
an introduction. Sections are titled by the row name alone, with rows inside a
row nested beneath it. A section shows a path only for a route outside the
page's menu ("Also in SET-UP › SCREEN SET-UP"). Choices without descriptions
are one inline list ("ON · OFF").

The menu index (`/docs/manual/menus/`) lists every menu row Fujimod adds or
changes, in camera menu order, using the same rows and order as the home
page outline. Each row is a link to its place on the manual page that covers
it. One mapping from menu paths to manual pages drives both the index and
the chapters, and every shown row maps to a page.

### Photo comparisons

A section whose recordings kept photographs shows them between the section's
title (with its summary) and its options. The photographs sit side by side in
one row with a narrow gap, from the text column's left edge to the window's
right edge. Each photograph is captioned with its settings as the camera shows
them, and the default setting is always among them, so readers see what they
get without changing anything.

With the pointer over a photograph, every photograph shows its 1:1 pixels at
the same point, following the pointer, and a black one-pixel cross replaces
the pointer at that point on every photograph. Leaving the row returns to the
whole photographs. On touch screens a tap zooms, dragging follows the finger,
and another tap returns.

## Other pages

- `/safety`: the risks of custom firmware and how Fujimod mitigates them.
- `/credits`: GitHub Contributors (from `contributors/`), Testing & Feedback,
  Recipe Authors (each author with their recipes, linked to each recipe's
  source), Photographs (a row of each photographer's scene photographs;
  a photograph opens in a lightbox) and Open Source (generated from a CycloneDX SBOM of everything the
  camera image, website and emulator ship, with each license's full text and
  a link to the source at the shipped version).
- `/releases`: all release notes, read from the GitHub releases at build time.

The personal note and the safety text are written in the website itself.

Text pages (`/safety`, `/credits`, release notes) share one layout with an
outline of their headings beside the text. The manual and the SDK docs share
one docs layout with the chapter sidebar.
