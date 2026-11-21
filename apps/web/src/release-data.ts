export interface SiteRelease {
  readonly version: string;
  readonly publishedAt: string;
  readonly notes: string;
}

export interface SiteReleaseData {
  readonly current: SiteRelease | null;
  readonly past: readonly SiteRelease[];
  readonly personalNote: {
    readonly daysSinceFirstRelease: number;
    readonly contributorCount: number;
    readonly contributorNames: readonly string[];
  };
}

declare const __FUJIMOD_RELEASE_DATA__: SiteReleaseData;

export const releaseData = __FUJIMOD_RELEASE_DATA__;

/** Release dates read in Eastern time, where releases are published. */
export const releaseDate = new Intl.DateTimeFormat('en-US', {
  dateStyle: 'long',
  timeZone: 'America/New_York',
});
