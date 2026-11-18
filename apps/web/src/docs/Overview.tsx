import { For } from 'solid-js';
import { SiteFrame } from '../ui/SiteFrame';
import { focalComponents } from './sdk.generated';
import { manualChapters } from './manual-data';
import { manualRoot } from './manual-routes';
import './docs-overview.css';

const chapters = manualChapters();

export default function DocsOverview() {
  return (
    <SiteFrame class="docs-overview">
      <article>
        <h1>Docs</h1>
        <div class="docs-overview-grid">
          <section>
            <h2>
              <a href={manualRoot}>Manual</a>
            </h2>
            <p>Every Fujimod feature on the camera, organized like Fujifilm's own manual.</p>
            <ol>
              <For each={chapters}>
                {(chapter) => (
                  <li>
                    <a href={chapter.pages[0]?.href ?? manualRoot}>{chapter.title}</a>
                  </li>
                )}
              </For>
            </ol>
          </section>
          <section>
            <h2>
              <a href="/docs/sdk/focal/">FocalUI SDK</a>
            </h2>
            <p>The toolkit mods use to draw camera screens, with live previews of each component.</p>
            <ul>
              <For each={focalComponents}>
                {(component) => (
                  <li>
                    <a href={`/docs/sdk/focal/components/${component.slug}/`}>{component.title}</a>
                  </li>
                )}
              </For>
            </ul>
          </section>
        </div>
      </article>
    </SiteFrame>
  );
}
