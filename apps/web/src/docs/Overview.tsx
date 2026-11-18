import { For } from 'solid-js';
import { SiteFrame } from '../ui/SiteFrame';
import { manualChapters } from './manual-data';
import { manualRoot } from './manual-routes';
import './docs-overview.css';

const chapters = manualChapters();

/** The developer SDKs, each with its own reference under /docs/sdk/. */
const sdks = [
  {
    title: 'FocalUI',
    href: '/docs/sdk/focal/',
    description:
      'Declarative multi-touch UI framework inspired by SwiftUI and rendered on the GPU.',
  },
];

export default function DocsOverview() {
  return (
    <SiteFrame class="docs-overview">
      <article>
        <span class="docs-overview-badge">Work in progress</span>
        <section>
          <h2>
            <a href={manualRoot}>Manual</a>
          </h2>
          <p>Learn how to use your Fujimod camera.</p>
          <For each={chapters}>
            {(chapter, index) => (
              <a class="docs-overview-card" href={chapter.pages[0]?.href ?? manualRoot}>
                <h3>
                  <span>{index() + 1}</span>
                  {chapter.title}
                </h3>
                <p>{chapter.pages.map((page) => page.title).join(' · ')}</p>
              </a>
            )}
          </For>
        </section>
        <section>
          <h2>Fujimod SDK</h2>
          <p>Documentation for Fujimod firmware developers.</p>
          <For each={sdks}>
            {(sdk) => (
              <a class="docs-overview-card" href={sdk.href}>
                <h3>{sdk.title}</h3>
                <p>{sdk.description}</p>
              </a>
            )}
          </For>
        </section>
      </article>
    </SiteFrame>
  );
}
