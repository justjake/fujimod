# Design library

Shared pieces for prototypes of camera screens, published under `/docs/design/`.
A prototype is a page in `src/docs/design/<name>/` that draws a 720 by 480
camera screen with these pieces and the default photo.

- `materials.ts`: the colors and surfaces every screen uses (accent, text,
  glass column, tracks, wells).
- `looks.tsx`: the looks (Glass, High contrast, Fujimod menu) and
  `DesignFrame`, which wraps a prototype page with the look switches.
- `controls.tsx`: the touch controls: `Choice` (a strip of words or artwork)
  and `Dial` (a slider scale with drag, swipe and settle).
- `screen.tsx`: `Fit` scales the camera screen to the page; `CameraKeys` and
  `KeyHint` are the on-screen camera keys for phones.
- `artwork.ts`: icons. Stock camera artwork is never published: it is served
  only by the development server, and a published page draws a word instead.
- `design.css`: Tailwind sources and the look variables.

Add a prototype to `PROTOTYPES` in `src/docs/design/Index.tsx` and to the
routes in `src/app.tsx`, `scripts/prerender.ts` and `src/page-metadata.ts`.
