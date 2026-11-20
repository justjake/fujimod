/** Accept only the read-only capability the upload service returned. */
const CONTRIBUTION =
  /^\/contribution\/[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/;

/** The capability as an absolute URL on `origin`, or undefined for anything else. */
export function resolveShareUrl(value: unknown, origin: string): string | undefined {
  if (typeof value !== 'string' || !origin) return undefined;
  let url: URL, base: URL;
  try {
    base = new URL(origin);
    url = new URL(value, base);
  } catch {
    return undefined;
  }
  return CONTRIBUTION.test(url.pathname) && url.origin === base.origin ? url.href : undefined;
}
