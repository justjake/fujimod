const qrSharing = {
  heading: 'QR Sharing',
  description: 'Share recipes and scan recipe and settings QR codes on the Fujifilm X70.',
};

export const textPages = {
  '/safety': {
    heading: 'Safety',
    description: 'The risks of custom camera firmware and how FujiMod mitigates them.',
  },
  '/credits': {
    heading: 'Credits',
    description: 'Credits for FujiMod.',
  },
  '/releases': {
    heading: 'Releases',
    description: 'FujiMod release notes.',
  },
  '/qr': qrSharing,
  '/recipe': qrSharing,
  '/settings': qrSharing,
};

export function textPagePath(path: string): keyof typeof textPages | undefined {
  const canonical = path.replace(/\/$/, '');
  return Object.hasOwn(textPages, canonical) ? (canonical as keyof typeof textPages) : undefined;
}
