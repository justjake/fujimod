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
};

export function textPagePath(path: string): keyof typeof textPages | undefined {
  const canonical = path.replace(/\/$/, '');
  return Object.hasOwn(textPages, canonical) ? (canonical as keyof typeof textPages) : undefined;
}
