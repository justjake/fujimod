import { execFileSync } from 'node:child_process';
import { readdir, readFile, stat } from 'node:fs/promises';
import { resolve } from 'node:path';
import type { SiteRelease, SiteReleaseData } from '../src/release-data.ts';

const releasesUrl = 'https://api.github.com/repos/justjake/fujimod/releases';
const personalNoteStartVersion = '0.0.1';

interface GitHubRelease {
  tag_name: string;
  published_at: string | null;
  body: string | null;
  draft: boolean;
}

async function githubReleases(request: typeof fetch, token?: string): Promise<SiteRelease[]> {
  const releases: SiteRelease[] = [];
  const signal = AbortSignal.timeout(10_000);
  let url: string | undefined = releasesUrl + '?per_page=100';
  while (url) {
    const response: Response = await request(url, {
      signal,
      headers: {
        Accept: 'application/vnd.github+json',
        ...(token ? { Authorization: `Bearer ${token}` } : {}),
      },
    });
    if (!response.ok) throw Error(`GitHub releases request failed (HTTP ${response.status})`);
    const page: GitHubRelease[] = await response.json();
    for (const release of page) {
      if (release.draft || !release.published_at) continue;
      releases.push({
        version: release.tag_name,
        publishedAt: new Date(release.published_at).toISOString(),
        notes: release.body ?? '',
      });
    }
    url = response.headers.get('link')?.match(/<([^>]+)>; rel="next"/)?.[1];
  }
  return releases.sort((a, b) => b.publishedAt.localeCompare(a.publishedAt));
}

async function localReleases(directory: string): Promise<SiteRelease[]> {
  const versions = new Intl.Collator('en', { numeric: true });
  const files = (await readdir(directory))
    .filter((name) => /^v?\d+\.\d+\.\d+\.md$/.test(name))
    .sort((a, b) => versions.compare(b.replace(/^v/, ''), a.replace(/^v/, '')));
  return Promise.all(
    files.map(async (file) => {
      const path = resolve(directory, file);
      let addedAt = '';
      try {
        addedAt = execFileSync(
          'git',
          ['log', '-1', '--diff-filter=A', '--format=%aI', '--', file],
          {
            cwd: directory,
            encoding: 'utf8',
            stdio: ['ignore', 'pipe', 'ignore'],
          },
        ).trim();
      } catch {
        // Exported preview notes may be outside a Git checkout.
      }
      return {
        version: file.slice(0, -3),
        publishedAt: (addedAt ? new Date(addedAt) : (await stat(path)).mtime).toISOString(),
        notes: await readFile(path, 'utf8'),
      };
    }),
  );
}

async function contributorNames(directory: string): Promise<string[]> {
  const entries = await readdir(directory, { withFileTypes: true });
  return Promise.all(
    entries
      .filter((entry) => entry.isFile() && entry.name.endsWith('.json'))
      .sort((a, b) => a.name.localeCompare(b.name, 'en'))
      .map(async ({ name }) => {
        const person: { attributionName?: string } = JSON.parse(
          await readFile(resolve(directory, name), 'utf8'),
        );
        if (typeof person.attributionName !== 'string' || !person.attributionName.trim())
          throw Error(`Missing attributionName in ${name}`);
        return person.attributionName;
      }),
  );
}

interface ReleaseDataOptions {
  preview: boolean;
  notesDirectory?: string;
  contributorsDirectory?: string;
  token?: string;
}

export async function loadReleaseData(
  options: ReleaseDataOptions,
  request = fetch,
): Promise<SiteReleaseData> {
  const [releases, names] = await Promise.all([
    options.preview
      ? options.notesDirectory
        ? localReleases(options.notesDirectory)
        : []
      : githubReleases(request, options.token),
    options.contributorsDirectory ? contributorNames(options.contributorsDirectory) : [],
  ]);
  const [current = null, ...past] = releases;
  const start = releases.find(
    ({ version }) => version.replace(/^v/, '') === personalNoteStartVersion,
  );
  const utcDay = (date: string) => Math.floor(Date.parse(date) / 86_400_000);
  return {
    current,
    past,
    personalNote: {
      daysSinceFirstRelease:
        current && start ? Math.max(0, utcDay(current.publishedAt) - utcDay(start.publishedAt)) : 0,
      contributorCount: names.length,
      contributorNames: names,
    },
  };
}
