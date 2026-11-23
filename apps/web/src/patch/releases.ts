/** Metadata shared by the release proxy, local server and browser loader. */
export interface ReleaseInfo {
  tag: string;
  name: string;
  published_at: string;
  prerelease: boolean;
}
export const releaseTag = /^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,95}$/;
export const releaseFilename = /^[a-zA-Z0-9][a-zA-Z0-9_.-]*$/;
export function validateReleaseInfo(value: unknown): ReleaseInfo {
  const info = value as ReleaseInfo;
  if (
    !info ||
    typeof info.tag !== 'string' ||
    !releaseTag.test(info.tag) ||
    typeof info.name !== 'string' ||
    !info.name ||
    typeof info.published_at !== 'string' ||
    Number.isNaN(Date.parse(info.published_at)) ||
    typeof info.prerelease !== 'boolean'
  )
    throw Error('The current release metadata is invalid.');
  return info;
}
export type ReleaseRoute =
  | { kind: 'current' }
  | { kind: 'list' }
  | { kind: 'asset'; tag: string; file: string };
export function releaseRoute(path: string): ReleaseRoute | undefined {
  if (path === '/releases/current') return { kind: 'current' };
  if (path === '/releases/list') return { kind: 'list' };
  const match = /^\/releases\/([^/]+)\/([^/]+)$/.exec(path);
  if (!match) return;
  try {
    const tag = decodeURIComponent(match[1]),
      file = decodeURIComponent(match[2]);
    if (releaseTag.test(tag) && releaseFilename.test(file)) return { kind: 'asset', tag, file };
  } catch {
    /* Malformed URL components are not release paths. */
  }
}
