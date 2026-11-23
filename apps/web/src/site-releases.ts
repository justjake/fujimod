/** Published releases as the site shows them, shared by the build, the Worker
 * and the browser. */
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

/** The fields of a GitHub release the site reads. */
export interface GitHubReleaseNotes {
  tag_name: string;
  published_at: string | null;
  body: string | null;
  draft: boolean;
}

/** A published GitHub release as the site shows it; undefined for drafts. */
export function siteRelease(release: GitHubReleaseNotes): SiteRelease | undefined {
  if (release.draft || !release.published_at) return undefined;
  return {
    version: release.tag_name,
    publishedAt: new Date(release.published_at).toISOString(),
    notes: release.body ?? '',
  };
}

/** Newest first. */
export function sortReleases(releases: readonly SiteRelease[]): SiteRelease[] {
  return [...releases].sort((a, b) => b.publishedAt.localeCompare(a.publishedAt));
}

/** The release list the Worker serves at `/releases/list`, checked field by field. */
export function validateSiteReleases(value: unknown): SiteRelease[] {
  if (!Array.isArray(value)) throw Error('The release list is not an array.');
  return value.map((entry: unknown) => {
    const release = entry as SiteRelease;
    if (
      !release ||
      typeof release.version !== 'string' ||
      !release.version ||
      typeof release.publishedAt !== 'string' ||
      Number.isNaN(Date.parse(release.publishedAt)) ||
      typeof release.notes !== 'string'
    )
      throw Error('The release list holds an invalid release.');
    return {
      version: release.version,
      publishedAt: release.publishedAt,
      notes: release.notes,
    };
  });
}
