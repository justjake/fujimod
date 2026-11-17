import { getRequestEvent, type JSX } from '@solidjs/web';
import { For, Show } from 'solid-js';
import { SiteFrame } from '../ui/SiteFrame';
import './text-page.css';

export type TextSection = {
  id: string;
  title: string;
  level: 2 | 3;
  content: () => JSX.Element;
};

export default function TextPage(props: {
  title: string;
  sections?: readonly TextSection[];
  children?: JSX.Element;
}) {
  const path = new URL(getRequestEvent()?.request.url ?? window.location.href).pathname;
  return (
    <SiteFrame>
      <div class="text-page">
        <Show when={props.sections?.length}>
          <nav class="page-outline" aria-label="On this page">
            <b>On this page</b>
            <ul>
              <For each={props.sections}>
                {(section) => (
                  <li class={section.level === 3 ? 'page-outline-subheading' : undefined}>
                    <a href={`${path}#${section.id}`}>{section.title}</a>
                  </li>
                )}
              </For>
            </ul>
          </nav>
        </Show>
        <main>
          <h1>{props.title}</h1>
          {props.children}
          <For each={props.sections}>
            {(section) => (
              <>
                {section.level === 2 ? (
                  <h2 id={section.id}>{section.title}</h2>
                ) : (
                  <h3 id={section.id}>{section.title}</h3>
                )}
                {section.content()}
              </>
            )}
          </For>
        </main>
      </div>
    </SiteFrame>
  );
}
