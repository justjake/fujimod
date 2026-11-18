/** Official firmware download locations accepted by the relay. */
export function firmwareSource(value: string): URL | undefined {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return undefined;
  }
  if (url.protocol !== 'https:' || url.port || url.username || url.password || url.hash)
    return undefined;
  const global =
    url.hostname === 'dl.fujifilm-x.com' &&
    /^\/support\/firmware\/[A-Za-z0-9_-]+\/[A-Za-z0-9_-]+\.(dat|zip)$/i.test(url.pathname);
  const korea =
    ['fujifilm-korea.co.kr', 'www.fujifilm-korea.co.kr'].includes(url.hostname) &&
    /^\/support\/download\/attachment\/\d+\/\d+$/.test(url.pathname);
  return global || korea ? url : undefined;
}
