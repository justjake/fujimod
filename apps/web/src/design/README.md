# Design library

Shared pieces for prototypes of camera screens, published under `/docs/design/`.
A prototype is a page in `src/docs/design/<name>/` built only from these
pieces plus the controls its screen needs alone. When two screens need the
same thing, it belongs here.

- `materials.ts`: colors, text sizes and surfaces (accent, column, tracks, glass).
- `looks.tsx`: the looks (Glass, High contrast, Fujimod menu) and
  `DesignFrame`, which wraps a prototype page with the look switches.
- `screen.tsx`: `CameraScreen` (the 720 by 480 screen the looks apply in),
  `Fit` (scales it to the page), `Card` (glass over the picture), `KeyHint`
  (a key and its action), `CameraKeys` (on-screen camera keys for phones)
  and `States` (a page's review states, each linkable by id).
- `column.tsx`: the column of rows: `Scroller` (native touch scrolling),
  `reveal` (keeps the focused row in view), `Section` (a titled group) and
  `Button` (an action).
- `controls.tsx`: the row controls: `Choice` (a strip of words or artwork)
  and `Dial` (a scale with drag, swipe and settle).
- `artwork.tsx`: `Artwork`, `Wordmark` and `Monogram`, and artwork URLs.
  Stock camera artwork is never published: only the development server
  serves it, and a published page draws the fallback instead.
- `design.css`: Tailwind sources and the look variables.

Add a prototype to `PROTOTYPES` in `src/docs/design/Index.tsx` and to the
routes in `src/app.tsx`, `scripts/prerender.ts` and `src/page-metadata.ts`.
