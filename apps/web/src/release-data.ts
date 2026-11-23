import { createSignal } from 'solid-js';
import { sortReleases, validateSiteReleases, type SiteReleaseData } from './site-releases';

declare const __FUJIMOD_RELEASE_DATA__: SiteReleaseData;

/** The releases read when the site was built, replaced by the Worker's cached
 * list once `loadLiveReleases` reads it. */
const [releases, setReleases] = createSignal<SiteReleaseData>(__FUJIMOD_RELEASE_DATA__);
export const releaseData = releases;

let loading: Promise<void> | undefined;

/** Read `/releases/list` once per page load. Any failure keeps the releases
 * the page was built with. */
export function loadLiveReleases(fetcher: typeof fetch = fetch): Promise<void> {
  loading ??= (async () => {
    try {
      const response = await fetcher('/releases/list');
      if (!response.ok) return;
      const [current, ...past] = sortReleases(validateSiteReleases(await response.json()));
      if (!current) return;
      setReleases((data) => ({ ...data, current, past }));
    } catch {
      /* The build-time releases stay on the page. */
    }
  })();
  return loading;
}

/** Release dates read in Eastern time, where releases are published. */
export const releaseDate = new Intl.DateTimeFormat('en-US', {
  dateStyle: 'long',
  timeZone: 'America/New_York',
});
