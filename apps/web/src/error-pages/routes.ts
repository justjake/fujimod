/** Every error page the site serves, by HTTP status. */
export const errorPages = { 404: '/404', 500: '/500', 503: '/503' } as const;
export type ErrorStatus = keyof typeof errorPages;

/** The page shown for a failed response: upstream outages are temporary. */
export function errorPageStatus(status: number): ErrorStatus {
  return status === 404 ? 404 : [502, 503, 504].includes(status) ? 503 : 500;
}

/** Error pages are served at the failing address, so the page names itself in
 * the document and hydration renders the same page at any address. */
export function servedErrorStatus(path: string): ErrorStatus | undefined {
  if (typeof document !== 'undefined') {
    const status = Number(document.documentElement.dataset.error);
    return status in errorPages ? (status as ErrorStatus) : undefined;
  }
  return renderedErrorStatus(path);
}

/** The error page a prerendered address renders. */
export function renderedErrorStatus(path: string): ErrorStatus | undefined {
  const entry = Object.entries(errorPages).find(([, page]) => page === path);
  return entry ? (Number(entry[0]) as ErrorStatus) : undefined;
}
