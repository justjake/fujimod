import type { JSX } from '@solidjs/web';
import { For, Show } from 'solid-js';
import './docs-layout.css';

type DocLink = { title: string; href: string };
type DocChapter = { title: string; pages: readonly DocLink[] };

export function DocsLayout(props: {
  title: string;
  home?: DocLink;
  chapters: readonly DocChapter[];
  path: string;
  index?: DocLink;
  search?: JSX.Element;
  children: JSX.Element;
}) {
  const current = (href: string) =>
    props.path === href || props.path + '/' === href ? 'page' : undefined;

  return (
    <div class="docs-layout">
      <nav class="docs-sidebar" aria-label={props.title}>
        <Show when={props.home}>
          {(home) => (
            <a class="docs-home" href={home().href} aria-current={current(home().href)}>
              {home().title}
            </a>
          )}
        </Show>
        {props.search}
        <div class="docs-sidebar-pages">
          <ul>
            <For each={props.chapters}>
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
        </div>
      </nav>
      <main>{props.children}</main>
    </div>
  );
}
