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
      'Glass draws the resolved material: the calibrated fill, the volume as nine slices along the top and bottom edges, the light rims and the dark hairline, with content in flow between them. All ten variants resolve from the stylesheet’s numbers.',
    remaining: 'Attaching the press feedback to the glass.',
  },
  {
    title: 'Indicator',
    upstream: 'https://fluidkit.mogilnikov.dev/indicator/',
    previews: ['fluid-indicator'],
    ported:
      'The thumb flies to the item under a finger on five springs and keeps its speed when turned elsewhere; a tap chooses the item.',
    remaining: 'The click, focus and active props.',
  },
  {
    title: 'Drag',
    upstream: 'https://fluidkit.mogilnikov.dev/drag/',
    previews: ['fluid-drag'],
    ported:
      'A sheet follows the finger exactly, pulls back like rubber past its last snap, and settles on the nearest one at the release’s speed.',
    remaining: 'Bounds, the scroll-view handoff, and following the pointer on a spring.',
  },
  {
    title: 'Morph',
    upstream: 'https://fluidkit.mogilnikov.dev/morph/',
    previews: ['fluid-morph'],
    ported:
      'A button flows into a panel: four springs move the shape’s box and size, and its fill and corners follow the same flight.',
    remaining: 'Matched parts riding between the two states.',
  },
  {
    title: 'Layout',
    upstream: 'https://fluidkit.mogilnikov.dev/layout/',
    previews: ['fluid-layout'],
    ported:
      'Keyed rows start below their slots and faded out, then spring home while their slots keep the list from moving.',
    remaining: 'A caller-declared row, and animating rows that move rather than enter.',
  },
  {
    title: 'Lift',
    upstream: 'https://fluidkit.mogilnikov.dev/lift/',
    previews: ['fluid-lift'],
    ported:
      'A long press shrinks the card and grows its menu above it; a tap on a row chooses it and closes the menu.',
    remaining: 'The dim and blur behind it, dialog placement, and a lifted preview.',
  },
  {
    title: 'Motion',
    upstream: 'https://fluidkit.mogilnikov.dev/motion/',
    previews: ['fluid-motion'],
    ported:
      'The springs, the interruptible animator and Fluid Kit’s transitions run as Focal motion, with host checks beside them.',
    remaining: 'Nothing: the page animates every preset and transition.',
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
