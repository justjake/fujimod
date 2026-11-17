import { getRequestEvent } from '@solidjs/web';
import { SiteFrame } from '../ui/SiteFrame';
import { DocsLayout } from './DocsLayout';
import {
  manualChapters,
  manualMenus,
  manualPagePath,
  manualPageTitle,
  manualRoot,
} from './manual-routes';

const chapters = manualChapters.map((chapter, index) => ({
  title: `${index + 1} · ${chapter.title}`,
  pages: chapter.pages.map((page) => ({ title: page.title, href: manualPagePath(page.slug) })),
}));

export default function Manual() {
  const path = new URL(getRequestEvent()?.request.url ?? window.location.href).pathname;
  return (
    <SiteFrame class="manual">
      <DocsLayout
        home={{ title: 'Manual', href: manualRoot }}
        chapters={chapters}
        path={path}
        index={{ title: 'Menu index', href: manualMenus }}
        searchLabel="Search the manual"
      >
        <h1 class="manual-title">{manualPageTitle(path)}</h1>
      </DocsLayout>
    </SiteFrame>
  );
}
