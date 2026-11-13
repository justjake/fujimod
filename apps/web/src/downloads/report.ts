import type { DownloadEvent } from './event.ts';

const endpoint = '/api/downloads';
const sessionKey = 'fujimod.download-session';

/** A tab-scoped random ID lets us count distinct download sessions without
 * keeping a visitor identifier across visits. */
export function downloadSession(): string | undefined {
  try {
    const saved = sessionStorage.getItem(sessionKey);
    if (saved) return saved;
    const created = crypto.randomUUID();
    sessionStorage.setItem(sessionKey, created);
    return created;
  } catch {
    return undefined;
  }
}

export function reportDownload(event: Omit<DownloadEvent, 'session'>): void {
  if (typeof window === 'undefined') return;
  const body = JSON.stringify({ ...event, session: downloadSession() });
  try {
    if (navigator.sendBeacon?.(endpoint, new Blob([body], { type: 'application/json' }))) return;
  } catch {
    // Fall through to fetch when the browser refuses a beacon.
  }
  void fetch(endpoint, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body,
    keepalive: true,
  }).catch(() => undefined);
}
