import { For, Show, createMemo, createSignal, onCleanup } from 'solid-js';
import { Dynamic, type JSX } from '@solidjs/web';
import type { Credits } from '../home/catalog';
import { Rich } from '../home/catalog-parts';
import { featureMedia, type Media } from '../home/feature-media';
import { Markdown } from './Markdown';
import { PhotoComparison } from './PhotoComparison';
import type { ManualPage, ManualSection } from './manual-data';
import { manualRoot } from './manual-routes';

const mediaForCard = featureMedia();
const mediaRoot = '/artifacts/docs/';

function sectionMedia(section: ManualSection) {
  return section.media ? mediaForCard(section.media.card) : null;
}

function SectionPhotos(props: { section: ManualSection }) {
  const photos = () => {
    const list = sectionMedia(props.section)?.photos;
    return list?.length ? list : undefined;
  };
  return <Show when={photos()}>{(photos) => <PhotoComparison photos={photos()} />}</Show>;
}

function hasCredits(credits: Credits) {
  return credits.premiere || credits.inspired.length > 0 || credits.thanks.length > 0;
}

function ManualCredits(props: { credits: Credits }) {
  return (
    <Show when={hasCredits(props.credits)}>
      <ul class="manual-credits">
        <Show when={props.credits.premiere}>
          <li>World premiere</li>
        </Show>
        <For each={props.credits.inspired}>
          {(line) => (
            <li>
              <b>Inspired by</b> <Rich inline={line} />
            </li>
          )}
        </For>
        <For each={props.credits.thanks}>
          {(line) => (
            <li>
              <b>Thanks</b> <Rich inline={line} />
            </li>
          )}
        </For>
      </ul>
    </Show>
  );
}

function WalkthroughClip(props: { media: Media; label: string; range?: [number, number] }) {
  const fragment = () => (props.range ? `#t=${props.range.join(',')}` : '');
  return (
    <video
      class="manual-video"
      aria-label={props.label}
      src={mediaRoot + props.media.video + fragment()}
      poster={mediaRoot + props.media.still}
      controls
      muted
      playsinline
      preload="none"
    />
  );
}

type ManualRow = { section: ManualSection; children: ManualRow[] };
type MenuPath = readonly string[];

function manualLayout(page: ManualPage) {
  const own: ManualSection[] = [];
  const rows: ManualRow[] = [];
  const byPath = new Map<string, ManualRow>();
  for (const section of page.sections) {
    if (section.title === page.title && section.path?.length) {
      own.push(section);
      continue;
    }
    const row = { section, children: [] };
    rows.push(row);
    if (section.path) byPath.set(JSON.stringify([...section.path, section.title]), row);
  }
  const top: ManualRow[] = [];
  for (const row of rows) {
    let path = row.section.path;
    let parent: ManualRow | undefined;
    while (path?.length && !parent) {
      parent = byPath.get(JSON.stringify(path));
      path = path.slice(0, -1);
    }
    (parent?.children ?? top).push(row);
  }
  return {
    own,
    top,
    base: own[0] ? [...own[0].path!, own[0].title] : [],
    cameras: [...new Set(page.sections.flatMap((section) => section.cameras))].sort(),
  };
}

function AlternatePaths(props: { section: ManualSection; base: MenuPath }) {
  const paths = () => [
    ...new Set(
      props.section.paths
        ?.filter(
          (path) => !props.base.length || !props.base.every((name, index) => path[index] === name),
        )
        .map((path) => path.slice(0, -1).join(' › '))
        .filter(Boolean),
    ),
  ];
  return <For each={paths()}>{(path) => <p class="manual-also">Also in {path}</p>}</For>;
}

function ManualOptions(props: { section: ManualSection }) {
  const options = () => props.section.options ?? [];
  return (
    <Show when={options().length}>
      <Show
        when={options().some((option) => option.summary)}
        fallback={
          <>
            <p class="manual-choices">
              <For each={options()}>
                {(option, index) => (
                  <>
                    {index() ? ' · ' : ''}
                    <span class={{ 'manual-original': !!option.stock }}>{option.label}</span>
                  </>
                )}
              </For>
            </p>
            <For each={options()}>
              {(option) => (
                <Show when={option.credits && hasCredits(option.credits) && option.credits}>
                  {(credits) => (
                    <div class="manual-choice-credits">
                      <b>{option.label}</b>
                      <ManualCredits credits={credits()} />
                    </div>
                  )}
                </Show>
              )}
            </For>
          </>
        }
      >
        <dl class="manual-options">
          <For each={options()}>
            {(option) => (
              <div class={{ 'manual-original': !!option.stock }}>
                <dt>{option.label}</dt>
                <dd>
                  {option.summary}
                  <Show when={option.credits}>
                    {(credits) => <ManualCredits credits={credits()} />}
                  </Show>
                </dd>
              </div>
            )}
          </For>
        </dl>
      </Show>
    </Show>
  );
}

/** A generated table of the row's facts, such as a strength at each ISO. */
function ManualTable(props: { table: NonNullable<ManualSection['table']> }) {
  return (
    <div class="manual-table">
      <table>
        <caption>{props.table.caption}</caption>
        <thead>
          <tr>
            <th />
            <For each={props.table.columns}>{(column) => <th scope="col">{column}</th>}</For>
          </tr>
        </thead>
        <tbody>
          <For each={props.table.rows}>
            {(row) => (
              <tr>
                <th scope="row">{row.label}</th>
                <For each={row.cells}>{(cell) => <td>{cell}</td>}</For>
              </tr>
            )}
          </For>
        </tbody>
      </table>
    </div>
  );
}

function ManualRowContent(props: { section: ManualSection }) {
  return (
    <>
      <Show when={props.section.markdown}>{(text) => <Markdown text={text()} />}</Show>
      <ManualOptions section={props.section} />
      <Show when={props.section.table}>{(table) => <ManualTable table={table()} />}</Show>
      <Show when={props.section.credits}>{(credits) => <ManualCredits credits={credits()} />}</Show>
      <Show when={sectionMedia(props.section)}>
        {(recording) => (
          <>
            <WalkthroughClip media={recording()} label={props.section.title} />
            <For each={props.section.media?.options}>
              {(label) => (
                <Show when={recording().clips[label]}>
                  {(range) => (
                    <>
                      <p class="manual-clip-title">{label}</p>
                      <WalkthroughClip media={recording()} label={label} range={range()} />
                    </>
                  )}
                </Show>
              )}
            </For>
          </>
        )}
      </Show>
    </>
  );
}

const rowHeadings = ['h2', 'h3', 'h4', 'h5', 'h6'] as const;

/** A heading that copies a link to itself when clicked. */
function CopyLinkHeading(props: {
  level: 'h1' | (typeof rowHeadings)[number];
  /** The page's path, and the heading's anchor on it. */
  path: string;
  anchor?: string;
  children: JSX.Element;
}) {
  const [copied, setCopied] = createSignal(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const href = () => (props.anchor ? `${props.path}#${props.anchor}` : props.path);
  const copy = async (event: MouseEvent) => {
    event.preventDefault();
    const url = new URL(href(), location.origin);
    history.replaceState(history.state, '', url);
    await navigator.clipboard.writeText(url.href);
    setCopied(true);
    clearTimeout(timer);
    timer = setTimeout(() => setCopied(false), 1500);
  };
  onCleanup(() => clearTimeout(timer));
  return (
    <Dynamic component={props.level} class="manual-heading">
      <a href={href()} onClick={copy}>
        {props.children}
      </a>
      <span
        class={{ 'manual-link-copied': true, shown: copied() }}
        role="status"
        aria-hidden={copied() ? undefined : 'true'}
      >
        Link copied
      </span>
    </Dynamic>
  );
}

function ManualEntry(props: {
  row: ManualRow;
  path: string;
  base: MenuPath;
  cameras: readonly string[];
  depth: number;
}) {
  const section = () => props.row.section;
  const camerasDiffer = () =>
    section().cameras.length && section().cameras.length !== props.cameras.length;
  return (
    <section class={['manual-entry', { 'manual-subentry': props.depth > 0 }]} id={section().anchor}>
      <CopyLinkHeading
        level={rowHeadings[Math.min(props.depth, rowHeadings.length - 1)]}
        path={props.path}
        anchor={section().anchor}
      >
        {section().title}
      </CopyLinkHeading>
      <AlternatePaths section={section()} base={props.base} />
      <Show when={section().summary}>
        <p class="manual-summary">{section().summary}</p>
      </Show>
      <SectionPhotos section={section()} />
      <ManualRowContent section={section()} />
      <Show when={camerasDiffer()}>
        <p class="manual-cameras">{section().cameras.join(' · ')} only</p>
      </Show>
      <For each={props.row.children}>
        {(child) => (
          <ManualEntry
            row={child}
            path={props.path}
            base={props.base}
            cameras={props.cameras}
            depth={props.depth + 1}
          />
        )}
      </For>
    </section>
  );
}

export function ManualFeature(props: { page: ManualPage; path: string; chapter?: string }) {
  const layout = createMemo(() => manualLayout(props.page));
  return (
    <article class="manual-feature" id={layout().own[0]?.anchor}>
      <For each={layout().own.slice(1)}>
        {(section) => <span class="manual-page-anchor" id={section.anchor} />}
      </For>
      <p class="manual-crumb">
        <a href={manualRoot}>Manual</a>
        <Show when={props.chapter}>
          {' › '}
          {props.chapter}
        </Show>
      </p>
      <CopyLinkHeading level="h1" path={props.path}>
        {props.page.title}
      </CopyLinkHeading>
      <Show when={layout().base.length}>
        <p class="manual-page-path">
          {layout().base.slice(0, -1).join(' › ')} {' › '}
          <b>{props.page.title}</b>
        </p>
      </Show>
      <div class="manual-introduction">
        <Markdown
          text={props.page.introduction || layout().own[0]?.summary || props.page.summary}
        />
        <For each={layout().own}>
          {(section) => (
            <>
              <AlternatePaths section={section} base={layout().base} />
              <SectionPhotos section={section} />
              <ManualRowContent section={section} />
            </>
          )}
        </For>
      </div>
      <Show when={layout().cameras.length}>
        <p class="manual-cameras">{layout().cameras.join(' · ')}</p>
      </Show>
      <Show when={layout().top.length}>
        <nav class="manual-entry-links" aria-label="On this page">
          <For each={layout().top}>
            {({ section }) => <a href={`${props.path}#${section.anchor}`}>{section.title}</a>}
          </For>
        </nav>
      </Show>
      <For each={layout().top}>
        {(row) => (
          <ManualEntry
            row={row}
            path={props.path}
            base={layout().base}
            cameras={layout().cameras}
            depth={0}
          />
        )}
      </For>
    </article>
  );
}
