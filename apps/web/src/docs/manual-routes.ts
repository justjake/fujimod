import chapters from './manual-outline.generated.json' with { type: 'json' };

export { chapters as manualChapters };
export const manualRoot = '/docs/manual/';
export const manualMenus = manualRoot + 'menus/';

const manualPages = chapters.flatMap((chapter) => chapter.pages);

export function manualPagePath(slug: string) {
  return `${manualRoot}${slug}/`;
}

export const manualRoutes = [
  manualRoot,
  manualMenus,
  ...manualPages.map((page) => manualPagePath(page.slug)),
];

export function isManualPath(path: string) {
  return path === manualRoot.slice(0, -1) || path.startsWith(manualRoot);
}

export function manualPageTitle(path: string) {
  const canonical = path.endsWith('/') ? path : path + '/';
  if (canonical === manualMenus) return 'Menu index';
  return manualPages.find((page) => manualPagePath(page.slug) === canonical)?.title ?? 'Manual';
}
