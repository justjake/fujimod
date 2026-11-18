import { getRequestEvent } from '@solidjs/web';
import { For } from 'solid-js';
import { SiteFrame } from '../ui/SiteFrame';
import { DocsLayout } from './DocsLayout';
import { ManualFeature } from './ManualFeature';
import { ManualMenus } from './ManualMenus';
import { manualChapters, manualPageAt } from './manual-data';
import { manualMenus } from './manual-routes';
import './manual.css';

const chapters = manualChapters();
const sidebar = chapters.map((chapter, index) => ({
  title: `${index + 1} · ${chapter.title}`,
  pages: chapter.pages,
}));

function ManualOutline() {
  return (
    <>
      <h1>Manual</h1>
      <p class="manual-lede">
        Looking for a menu item? See the <a href={manualMenus}>menu index</a>.
      </p>
      <div class="manual-chapters">
        <For each={chapters}>
          {(chapter, index) => (
            <section>
              <h2>
                <span>{index() + 1}</span>
                {chapter.title}
              </h2>
              <ul>
                <For each={chapter.pages}>
                  {(page) => (
                    <li>
                      <a href={page.href}>{page.title}</a>
                      <span>{page.summary}</span>
                    </li>
                  )}
                </For>
              </ul>
            </section>
          )}
        </For>
      </div>
    </>
  );
}

export default function Manual() {
  const path = new URL(getRequestEvent()?.request.url ?? window.location.href).pathname;
  const canonical = path.endsWith('/') ? path : path + '/';
  const page = manualPageAt(canonical);
  const chapter = sidebar.find((group) => group.pages.some((link) => link.href === canonical));
  return (
    <SiteFrame class="manual">
      <DocsLayout
        title="Manual"
        chapters={sidebar}
        path={canonical}
        index={{ title: 'Menu index', href: manualMenus }}
        searchLabel="Search the manual"
      >
        {canonical === manualMenus ? (
          <ManualMenus />
        ) : page ? (
          <ManualFeature page={page} path={canonical} chapter={chapter?.title} />
        ) : (
          <ManualOutline />
        )}
      </DocsLayout>
    </SiteFrame>
  );
}
