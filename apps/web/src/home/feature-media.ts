/** Preview recording: clean clip, still, and each option's range in the clip. */
export type Media = { video: string; still: string; clips: Record<string, [number, number]> };

export type ReelCue = { start: number; end: number; text: string };
export type ReelMedia = Pick<Media, 'video' | 'still'> & { cues: ReelCue[] };

/** artifacts/docs/media.json carries the newest card and homepage recordings. */
export type FeatureMedia = {
  schema: 1;
  cards: Record<string, Media>;
  reels?: Record<string, ReelMedia>;
};

declare const __FUJIMOD_FEATURE_MEDIA__: FeatureMedia | null;

const validAssets = (media: Pick<Media, 'video' | 'still'>) =>
  /^[0-9a-f]{24}\.mp4$/.test(media.video) && /^[0-9a-f]{24}\.webp$/.test(media.still);

export function validateFeatureMedia(value: unknown): FeatureMedia {
  const manifest = value as FeatureMedia;
  if (manifest?.schema !== 1 || typeof manifest.cards !== 'object')
    throw Error('Invalid feature media manifest');
  for (const [card, media] of Object.entries(manifest.cards))
    if (
      !validAssets(media) ||
      Object.values(media.clips).some(
        (range) => range.length !== 2 || !(range[0] >= 0 && range[1] > range[0]),
      )
    )
      throw Error(`Invalid feature media: ${card}`);
  if (manifest.reels !== undefined && (!manifest.reels || typeof manifest.reels !== 'object'))
    throw Error('Invalid reel media manifest');
  for (const [id, reel] of Object.entries(manifest.reels ?? {})) {
    if (
      !validAssets(reel) ||
      !Array.isArray(reel.cues) ||
      !reel.cues.length ||
      reel.cues.some(
        (cue, i) =>
          !Number.isFinite(cue.start) ||
          !Number.isFinite(cue.end) ||
          cue.start < (reel.cues[i - 1]?.end ?? 0) ||
          cue.end <= cue.start ||
          typeof cue.text !== 'string' ||
          !cue.text.trim(),
      )
    )
      throw Error(`Invalid reel media: ${id}`);
  }
  return manifest;
}

/** Media lookup by card. Recordings may predate the current card text. */
export function featureMedia(manifest = __FUJIMOD_FEATURE_MEDIA__) {
  const cards = manifest?.cards ?? {};
  /** The first of these cards that has media. */
  return (...preference: string[]): Media | null =>
    preference.map((card) => cards[card]).find(Boolean) ?? null;
}

export function reelMedia(id: string, manifest = __FUJIMOD_FEATURE_MEDIA__): ReelMedia | undefined {
  return manifest?.reels?.[id];
}
