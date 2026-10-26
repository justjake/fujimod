import { createHash, randomUUID } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import type { AssetResolver, ResolvedAssets } from '@solidjs/web';

/** Mutable development modules must not share a preload URL across revisions.
 * Safari can reuse a preloaded response without revalidating no-cache:
 * https://bugs.webkit.org/show_bug.cgi?id=193533
 */
export function developmentAssets(manifest: unknown, root: string): AssetResolver {
  if (
    !manifest ||
    typeof manifest !== 'object' ||
    !('resolve' in manifest) ||
    typeof manifest.resolve !== 'function'
  ) {
    throw Error('Development rendering requires the Vite asset resolver');
  }
  const resolver = manifest as AssetResolver;
  // A restart can change compiler/configuration output without changing source.
  const session = randomUUID();
  async function version(assets: ResolvedAssets | null | undefined) {
    if (!assets) return assets;
    return {
      ...assets,
      js: await Promise.all(
        assets.js.map(async (entry) => {
          const url = new URL(entry, 'http://development.invalid');
          const file = url.pathname.startsWith('/@fs/')
            ? decodeURIComponent(url.pathname.slice(5))
            : resolve(root, '.' + decodeURIComponent(url.pathname));
          const source = await readFile(file);
          const revision = createHash('sha256').update(session).update(source).digest('hex');
          url.searchParams.set('development-revision', revision);
          return url.pathname + url.search;
        }),
      ),
    };
  }
  // The same versioned URL feeds both modulepreload and Solid's hydration
  // module map. Changing only the hint would let hydration import stale code.
  return { resolve: async (key) => version(await resolver.resolve(key)) };
}
