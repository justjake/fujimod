/** The sharing Worker serves the firmware download proxy, error logs, issue
 * reports and contribution uploads. Production builds use the public server;
 * the development preview runs it locally and proxies its paths under the
 * preview's own origin, so its URLs are site-relative there. */
declare const __FUJIMOD_SHARING_ORIGIN__: string;

/** Empty where the preview serves the Worker under its own origin. */
export const SHARING_ORIGIN = __FUJIMOD_SHARING_ORIGIN__;

/** A sharing-server path: absolute in production, site-relative in the preview. */
export function sharingUrl(path: string): string {
  return `${SHARING_ORIGIN}${path}`;
}

/** The origin a capability returned by the Worker belongs to. */
export function sharingBase(): string {
  return SHARING_ORIGIN || globalThis.location?.origin || '';
}
