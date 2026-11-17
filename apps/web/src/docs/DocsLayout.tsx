import type { JSX } from '@solidjs/web';
import { createMemo, createSignal, For, Show } from 'solid-js';
import './docs-layout.css';

type DocLink = { title: string; href: string };
type DocChapter = { title: string; pages: readonly DocLink[] };

export function DocsLayout(props: {
  home: DocLink;
  chapters: readonly DocChapter[];
  path: string;
  index?: DocLink;
  searchLabel?: string;
  children: JSX.Element;
}) {
  const [query, setQuery] = createSignal('');
  const visibleChapters = createMemo(() => {
    const search = query().trim().toLowerCase();
    if (!search) return props.chapters;
    return props.chapters
      .map((chapter) => ({
        ...chapter,
        pages: chapter.title.toLowerCase().includes(search)
          ? chapter.pages
          : chapter.pages.filter((page) => page.title.toLowerCase().includes(search)),
      }))
      .filter((chapter) => chapter.pages.length);
  });
  const current = (href: string) =>
    props.path === href || props.path + '/' === href ? 'page' : undefined;

  return (
    <div class="docs-layout">
      <nav class="docs-sidebar" aria-label={props.home.title}>
        <a class="docs-home" href={props.home.href} aria-current={current(props.home.href)}>
          {props.home.title}
        </a>
        <Show when={props.searchLabel}>
          <input
            type="search"
            aria-label={props.searchLabel}
            placeholder={props.searchLabel}
            onInput={(event) => setQuery(event.currentTarget.value)}
          />
        </Show>
        <ul>
          <For each={visibleChapters()}>
            {(chapter) => (
              <li class="docs-chapter">
                <span>{chapter.title}</span>
                <ul>
                  <For each={chapter.pages}>
                    {(page) => (
                      <li>
                        <a href={page.href} aria-current={current(page.href)}>
                          {page.title}
                        </a>
                      </li>
                    )}
                  </For>
                </ul>
              </li>
            )}
          </For>
        </ul>
        <Show when={props.index}>
          {(index) => (
            <a class="docs-index" href={index().href} aria-current={current(index().href)}>
              {index().title}
            </a>
          )}
        </Show>
      </nav>
      <main>{props.children}</main>
    </div>
  );
}
