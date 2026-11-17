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
  const route = Object.hasOwn(pages, normalized) ? (normalized as keyof typeof pages) : '/';
  return { ...pages[route], canonical: siteOrigin + route };
}
