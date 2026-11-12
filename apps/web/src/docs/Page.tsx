import { For, Show } from 'solid-js';
import { getRequestEvent } from '@solidjs/web';
import { SiteHeader } from '../ui/site-header';
import { focalComponents, type FocalApiDeclaration, type FocalComponent } from './sdk.generated';
import FocalUIPreview from './FocalUIPreview';
import '../home/site.css';
import '../navigation.css';
import './docs.css';

function currentPath(): string {
  return new URL(getRequestEvent()?.request.url ?? window.location.href).pathname;
}

function slugFromPath(path: string): string | undefined {
  return /^\/docs\/sdk\/focal\/components\/([^/]+)\/?$/.exec(path)?.[1];
}

function componentAt(path: string): FocalComponent | undefined {
  const slug = slugFromPath(path);
  return slug ? focalComponents.find((component) => component.slug === slug) : undefined;
}

function declarationSignature(declaration: FocalApiDeclaration): string {
  return declaration.kind === 'macro'
    ? `#define ${declaration.signature}`
    : declaration.signature;
}

function Sidebar(props: { current?: string }) {
  return (
    <nav class="sdk-sidebar" aria-label="FocalUI components">
      <a class="sdk-home" href="/docs/sdk/focal/">
        FocalUI
      </a>
      <ul>
        <For each={focalComponents}>
          {(component) => (
            <li>
              <a
                href={`/docs/sdk/focal/components/${component.slug}/`}
                aria-current={props.current === component.slug ? 'page' : undefined}
              >
                {component.title}
              </a>
            </li>
          )}
        </For>
      </ul>
    </nav>
  );
}

function Members(props: { declaration: FocalApiDeclaration }) {
  return (
    <Show when={props.declaration.members.length}>
      <dl class="sdk-members">
        <For each={props.declaration.members}>
          {(member) => (
            <div class="sdk-member">
              <dt>
                <code>{member.name}</code>
                <Show when={member.type}>
                  <span class="sdk-member-type">{member.type}</span>
                </Show>
              </dt>
              <dd>{member.comment}</dd>
            </div>
          )}
        </For>
      </dl>
    </Show>
  );
}

function ComponentPage(props: { component: FocalComponent }) {
  return (
    <article class="sdk-component">
      <header class="sdk-component-header">
        <h1>{props.component.title}</h1>
        <p class="sdk-header-name">{props.component.header}</p>
      </header>
      <Show when={props.component.summary}>
        <p class="sdk-summary">{props.component.summary}</p>
      </Show>
      <For each={props.component.previews}>{(preview) => <FocalUIPreview preview={preview} />}</For>
      <For each={props.component.declarations}>
        {(declaration) => (
          <section class="sdk-declaration" id={declaration.name}>
            <h2>{declaration.name}</h2>
            <pre>
              <code>{declarationSignature(declaration)}</code>
            </pre>
            <Show when={declaration.comment}>
              <p>{declaration.comment}</p>
            </Show>
            <Members declaration={declaration} />
          </section>
        )}
      </For>
    </article>
  );
}

function Index() {
  return (
    <article class="sdk-index">
      <h1>FocalUI</h1>
      <ul>
        <For each={focalComponents}>
          {(component) => (
            <li>
              <a href={`/docs/sdk/focal/components/${component.slug}/`}>
                <h2>{component.title}</h2>
                <Show when={component.summary}>
                  <p>{component.summary.split('\n')[0]}</p>
                </Show>
              </a>
            </li>
          )}
        </For>
      </ul>
    </article>
  );
}

export default function Docs() {
  const component = () => componentAt(currentPath());
  return (
    <div class="shell sdk-docs">
      <SiteHeader root="/" />
      <div class="sdk-layout">
        <Sidebar current={component()?.slug} />
        <main>
          <Show when={component()} fallback={<Index />}>
            {(found) => <ComponentPage component={found()} />}
          </Show>
        </main>
      </div>
    </div>
  );
}
