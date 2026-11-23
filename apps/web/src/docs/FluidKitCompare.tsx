import { For, Show } from 'solid-js';
import { getRequestEvent, type JSX } from '@solidjs/web';
import { SiteFrame } from '../ui/SiteFrame';
import { DocsLayout } from './DocsLayout';
import { focalComponents, type FocalPreview } from './sdk-data';
import './docs.css';

type CompareComponent = {
  title: string;
  upstream: string;
  previews: readonly string[];
  ported: string;
  remaining: string;
};

const comparisons: readonly CompareComponent[] = [
  {
    title: 'Blur',
    upstream: 'https://fluidkit.mogilnikov.dev/blur/',
    previews: ['fluid-glass'],
    ported:
      'Glass draws the resolved material: the calibrated fill, the volume at the top and bottom rows, the light rims and the dark edge, with content in flow between them.',
    remaining:
      'The group that melts its children, and attaching press feedback to the glass, come next.',
  },
  {
    title: 'Indicator',
    upstream: 'https://fluidkit.mogilnikov.dev/indicator/',
    previews: ['fluid-indicator'],
    ported: 'The indicator’s props and parts are mapped in the port plan.',
    remaining: 'The engine that follows the selected item is the next component.',
  },
  {
    title: 'Drag',
    upstream: 'https://fluidkit.mogilnikov.dev/drag/',
    previews: ['fluid-drag'],
    ported: 'The drag’s props and parts are mapped in the port plan.',
    remaining: 'The rubber band, snapping and handing a gesture to a scroll view are next.',
  },
  {
    title: 'Morph',
    upstream: 'https://fluidkit.mogilnikov.dev/morph/',
    previews: ['fluid-morph'],
    ported: 'The morph’s props and parts are mapped in the port plan.',
    remaining: 'The flight between two states and its matched parts are next.',
  },
  {
    title: 'Layout',
    upstream: 'https://fluidkit.mogilnikov.dev/layout/',
    previews: ['fluid-layout'],
    ported: 'The layout’s props and parts are mapped in the port plan.',
    remaining: 'Moving, entering and exiting items in place are next.',
  },
  {
    title: 'Lift',
    upstream: 'https://fluidkit.mogilnikov.dev/lift/',
    previews: ['fluid-lift'],
    ported: 'The lift’s props and parts are mapped in the port plan.',
    remaining: 'The lifted picture, its menu and its preview are next.',
  },
  {
    title: 'Motion',
    upstream: 'https://fluidkit.mogilnikov.dev/motion/',
    previews: ['fluid-motion'],
    ported:
      'The springs, the interruptible animator and Fluid Kit’s transitions run as Focal motion, with host checks beside them.',
    remaining: 'A preview that animates them is next.',
  },
];

const root = '/docs/fluid-kit/';
const chapters = [{
  title: 'Fluid Kit',
  pages: comparisons.map((component) => ({
    title: component.title,
    href: root + component.title.toLowerCase() + '/',
  })),
}];

function currentPath(): string {
  return new URL(getRequestEvent()?.request.url ?? window.location.href).pathname;
}

function componentAt(path: string): CompareComponent | undefined {
  const slug = /^\/docs\/fluid-kit\/([^/]+)\/?$/.exec(path)?.[1];
  return slug ? comparisons.find((component) =>
    component.title.toLowerCase() === slug) : undefined;
}

function portPreviews(component: CompareComponent): FocalPreview[] {
  return component.previews.flatMap((slug) => {
    const port = focalComponents.find((entry) => entry.slug === slug);
    return port?.previews ?? [];
  });
}

function Pane(props: { title: string; children: JSX.Element }) {
  return (
    <section class="fluid-kit-pane">
      <h2 class="fluid-kit-pane-title">{props.title}</h2>
      <div class="fluid-kit-frame">{props.children}</div>
    </section>
  );
}

function Comparison(props: { component: CompareComponent }) {
  const previews = portPreviews(props.component);
  return (
    <article class="sdk-component fluid-kit-compare">
      <header class="sdk-component-header">
        <h1>{props.component.title}</h1>
        <p class="sdk-header-name">Fluid Kit, and the Focal port</p>
      </header>
      <div class="fluid-kit-panes">
        <Pane title="Fluid Kit">
          <iframe
            class="fluid-kit-upstream"
            src={props.component.upstream}
            title={'Fluid Kit ' + props.component.title}
            loading="lazy"
          />
        </Pane>
        <Pane title="Focal UI">
          <Show
            when={previews.length}
            fallback={<p class="fluid-kit-note">No Focal preview yet.</p>}
          >
            <For each={previews}>{(preview) => (
              <figure class="fluid-kit-focal">
                <div
                  class="fluid-kit-preview"
                  style={{ 'aspect-ratio': preview.width + ' / ' + preview.height }}
                >
                  <iframe
                    src={preview.page}
                    title={preview.title}
                    loading="lazy"
                  />
                </div>
                <figcaption>
                  <strong>{preview.title}</strong>
                  <Show when={preview.description}>
                    <span>{preview.description}</span>
                  </Show>
                </figcaption>
              </figure>
            )}</For>
          </Show>
        </Pane>
      </div>
      <dl class="fluid-kit-status">
        <dt>Ported</dt>
        <dd>{props.component.ported}</dd>
        <dt>Remaining</dt>
        <dd>{props.component.remaining}</dd>
      </dl>
    </article>
  );
}

function Index() {
  return (
    <article class="sdk-index fluid-kit-index">
      <h1>Fluid Kit</h1>
      <p>
        Fluid Kit’s React components and its core library are being ported to Focal UI in C,
        file by file. Each page puts the library’s published component beside the same
        component built from Focal’s primitives in this repository.
      </p>
      <ul>
        <For each={comparisons}>{(component) => (
          <li>
            <a href={root + component.title.toLowerCase() + '/'}>
              <h2>{component.title}</h2>
            </a>
          </li>
        )}</For>
      </ul>
    </article>
  );
}

export default function FluidKitCompare() {
  const path = currentPath();
  const component = componentAt(path);
  return (
    <SiteFrame class="sdk-docs">
      <DocsLayout
        title="Fluid Kit"
        home={{ title: 'Fluid Kit', href: root }}
        chapters={chapters}
        path={path}
      >
        <Show when={component} fallback={<Index />}>
          {(found) => <Comparison component={found()} />}
        </Show>
      </DocsLayout>
    </SiteFrame>
  );
}
