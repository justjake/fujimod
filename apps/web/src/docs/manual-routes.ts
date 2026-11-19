export const manualRoot = '/docs/manual/';
export const manualMenus = manualRoot + 'menus/';

export function manualPagePath(slug: string) {
  return `${manualRoot}${slug}/`;
}

export function isManualPath(path: string) {
  return path === manualRoot.slice(0, -1) || path.startsWith(manualRoot);
}
