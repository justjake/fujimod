import manual from './manual.generated.json' with { type: 'json' };
import type { Credits } from '../home/catalog.ts';
import { manualMenus, manualPagePath, manualRoot } from './manual-routes.ts';

export type ManualSection = {
  title: string;
  anchor: string;
  cameras: readonly string[];
  paths?: readonly (readonly string[])[];
  summary?: string | null;
  options?: readonly { label: string; summary: string | null; credits: Credits | null }[];
  credits?: Credits;
  media?: { card: string; options: readonly string[] } | null;
  markdown?: string;
};

export type ManualPage = {
  title: string;
  summary: string;
  introduction: string;
  sections: readonly ManualSection[];
};

const pages: Readonly<Partial<Record<string, ManualPage>>> = manual.pages;
const featureLinks: Readonly<Partial<Record<string, { page: string; anchor?: string }>>> =
  manual.feature_links;
const releaseFeatures: Readonly<Partial<Record<string, { credits: Credits }>>> = manual.features;

export function releaseNoteFeature(id: string) {
  const destination = Object.hasOwn(featureLinks, id) ? featureLinks[id] : undefined;
  return destination ? { ...destination, credits: releaseFeatures[id]?.credits } : undefined;
}

export const manualMenuRows = manual.menus;

export function manualRoutes() {
  return [manualRoot, manualMenus, ...Object.keys(pages).map(manualPagePath)];
}

export function manualChapters() {
  return manual.chapters.map((chapter) => ({
    title: chapter.title,
    pages: chapter.pages.map((slug) => {
      const page = pages[slug];
      if (!page) throw Error(`Unknown manual page "${slug}" in "${chapter.title}"`);
      return { title: page.title, summary: page.summary, href: manualPagePath(slug) };
    }),
  }));
}

export function manualPageTitle(path: string) {
  const canonical = path.endsWith('/') ? path : path + '/';
  if (canonical === manualMenus) return 'Menu index';
  return manualPageAt(canonical)?.title ?? 'Manual';
}

export function manualPageAt(path: string) {
  if (!path.startsWith(manualRoot)) return;
  const slug = path.slice(manualRoot.length).replace(/\/$/, '');
  return Object.hasOwn(pages, slug) ? pages[slug] : undefined;
}
