import manual from './manual.generated.json' with { type: 'json' };
import { manualMenus, manualPagePath, manualRoot } from './manual-routes.ts';

const pages: Readonly<Partial<Record<string, { title: string }>>> = manual.pages;

export function manualRoutes() {
  return [manualRoot, manualMenus, ...Object.keys(pages).map(manualPagePath)];
}

export function manualChapters() {
  return manual.chapters.map((chapter, index) => ({
    title: `${index + 1} · ${chapter.title}`,
    pages: chapter.pages.map((slug) => {
      const page = pages[slug];
      if (!page) throw Error(`Unknown manual page "${slug}" in "${chapter.title}"`);
      return { title: page.title, href: manualPagePath(slug) };
    }),
  }));
}

export function manualPageTitle(path: string) {
  const canonical = path.endsWith('/') ? path : path + '/';
  if (canonical === manualMenus) return 'Menu index';
  const slug = canonical.slice(manualRoot.length, -1);
  return pages[slug]?.title ?? 'Manual';
}
