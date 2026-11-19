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
