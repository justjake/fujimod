/* Artwork for prototypes. Stock camera artwork is never published: the
   development server serves it from its local review directory, and a
   published page gets no URL and draws its fallback (a word) instead. Our own
   artwork follows the same path until it is part of the site's assets. */

const local = (path: string): string | undefined => (import.meta.env.DEV ? `/review/${path}` : undefined);

/** A stock camera vector by its resource id, on the development server only. */
export const stockArtwork = (id: number) => local(`stock-icons/x70-110/ff${id.toString(16).padStart(6, '0')}.svg`);

/** A mod's setting icon by its slug. */
export const modArtwork = (slug: string) => local(`recipe-editor/mod-icons/${slug}.svg`);

/** A FILM ICONS pack icon by its slug. */
export const filmArtwork = (slug: string) => local(`film-icons/a-${slug}.svg`);
