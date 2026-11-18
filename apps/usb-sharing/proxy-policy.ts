/** Server-side URL allowlist. Add sources here without changing the proxy. */
const sources: readonly { hosts: readonly string[]; path: RegExp }[] = [
  {
    hosts: ['dl.fujifilm-x.com'],
    path: /^\/support\/firmware\/[A-Za-z0-9_-]+\/[A-Za-z0-9_-]+\.(dat|zip)$/i,
  },
  {
    hosts: ['fujifilm-korea.co.kr', 'www.fujifilm-korea.co.kr'],
    path: /^\/support\/download\/attachment\/\d+\/\d+$/,
  },
];

export function proxyTarget(value: string): URL | undefined {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return undefined;
  }
  if (url.protocol !== 'https:' || url.port || url.username || url.password || url.hash)
    return undefined;
  return sources.some(
    (source) => source.hosts.includes(url.hostname) && source.path.test(url.pathname),
  )
    ? url
    : undefined;
}
