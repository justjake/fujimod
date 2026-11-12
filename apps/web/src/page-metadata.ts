import { focalComponents } from './docs/sdk.generated';

const pages = {
  '/': {
    title: 'FujiMod',
    description:
      'New films, film effects and more control for Fujifilm X70, X-E2S and X-E2. Explore the FujiMod 0.1.0 preview, including 255 custom settings on X70 and SD boot v2. Features vary by camera.',
  },
  '/dev-tools/': {
    title: 'Dev Tools: Camera console',
    description:
      'Connect your Fujifilm camera over USB to inspect logs, collect bug reports, and help expand camera support.',
  },
  '/dev-tools/disable/': {
    title: 'Dev Tools: Leave debug mode',
    description:
      'Turn off USB debugging and restore ordinary USB file transfer after restarting your camera.',
  },
  '/dev-tools/service/': {
    title: 'Dev Tools: Enter service mode',
    description: 'Enter service mode to access your camera software’s internals.',
  },
  '/dev-tools/state-captures/': {
    title: 'Dev Tools: Capture camera modes',
    description: 'Collect camera state across operating modes to help expand FujiMod support.',
  },
  '/dev-tools/map-controls/': {
    title: 'Dev Tools: Map controls',
    description:
      'Record button, dial, and switch changes over USB to help FujiMod identify your camera’s controls.',
  },
  '/bug/': {
    title: 'Report a camera bug · FujiMod',
    description:
      'Retrieve a FujiMod bug report from your camera over USB or from its SD card, then download or submit it.',
  },
  '/recovery/': {
    title: 'Recover your camera · FujiMod',
    description:
      'Tried making your own mod but bricked your camera? Build a recovery SD card from official Fujifilm firmware to restore your device. This usually works and it’s cheaper than buying a new one.',
  },
  '/device/': {
    title: 'Camera sign-in · FujiMod',
    description: 'Approve a FujiMod camera connection with the code shown on the camera.',
  },
};

export const siteOrigin = 'https://fujimod.com';

export function pageMetadata(path: string) {
  const normalized = path.endsWith('/') ? path : path + '/';
  const componentSlug = /^\/docs\/sdk\/focal\/components\/([^/]+)\/$/.exec(normalized)?.[1];
  const component = componentSlug
    ? focalComponents.find((entry) => entry.slug === componentSlug)
    : undefined;
  if (component)
    return {
      title: `${component.title} · FocalUI SDK · FujiMod`,
      description: component.summary.replace(/\s+/g, ' ').slice(0, 160) ||
        'FocalUI component reference.',
      canonical: siteOrigin + normalized,
    };
  if (normalized === '/docs/sdk/focal/')
    return {
      title: 'FocalUI SDK · FujiMod',
      description: 'The FocalUI camera interface toolkit component reference.',
      canonical: siteOrigin + normalized,
    };
  const route = Object.hasOwn(pages, normalized) ? (normalized as keyof typeof pages) : '/';
  return { ...pages[route], canonical: siteOrigin + route };
}
