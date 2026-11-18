import { createMemo, For, Show } from 'solid-js';
import type { Credits } from '../home/catalog';
import { Rich } from '../home/catalog-parts';
import { featureMedia, type Media } from '../home/feature-media';
import { Markdown } from './Markdown';
import type { ManualPage, ManualSection } from './manual-data';
import { manualRoot } from './manual-routes';

const mediaForCard = featureMedia();
const mediaRoot = '/artifacts/docs/';

function sectionMedia(section: ManualSection) {
  return section.media ? mediaForCard(section.media.card) : null;
}

function ManualCredits(props: { credits: Credits }) {
  return (
    <Show
      when={props.credits.premiere || props.credits.inspired.length || props.credits.thanks.length}
    >
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

function WalkthroughClip(props: {
  media: Media;
  label: string;
  wide?: boolean;
  range?: [number, number];
}) {
  const fragment = () => (props.range ? `#t=${props.range.join(',')}` : '');
  return (
    <video
      class={['manual-video', { 'manual-video-wide': props.wide }]}
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

function ManualEntry(props: { section: ManualSection; wideVideo: boolean }) {
  const media = () => sectionMedia(props.section);
  return (
    <section class="manual-entry" id={props.section.anchor}>
      <Show when={props.section.paths?.length}>
        <ul class="manual-paths">
          <For each={props.section.paths}>{(path) => <li>{path.join(' › ')}</li>}</For>
        </ul>
      </Show>
      <h2>{props.section.title}</h2>
      <Show when={props.section.summary}>
        <p>{props.section.summary}</p>
      </Show>
      <Show when={props.section.markdown}>{(text) => <Markdown text={text()} />}</Show>
      <Show when={media()}>
        {(recording) => (
          <WalkthroughClip media={recording()} label={props.section.title} wide={props.wideVideo} />
        )}
      </Show>
      <Show when={props.section.options?.length}>
        <dl class="manual-options">
          <For each={props.section.options}>
            {(option) => (
              <div>
                <dt>{option.label}</dt>
                <dd>
                  {option.summary}
                  <Show when={option.credits}>
                    {(credits) => <ManualCredits credits={credits()} />}
                  </Show>
                  <Show when={props.section.media?.options.includes(option.label) && media()}>
                    {(recording) => (
                      <Show when={recording().clips[option.label]}>
                        {(range) => (
                          <WalkthroughClip
                            media={recording()}
                            label={option.label}
                            range={range()}
                          />
                        )}
                      </Show>
                    )}
                  </Show>
                </dd>
              </div>
            )}
          </For>
        </dl>
      </Show>
      <Show when={props.section.cameras.length}>
        <p class="manual-cameras">{props.section.cameras.join(' · ')}</p>
      </Show>
      <Show when={props.section.credits}>{(credits) => <ManualCredits credits={credits()} />}</Show>
    </section>
  );
}

export function ManualFeature(props: { page: ManualPage; path: string; chapter?: string }) {
  const leadEntry = createMemo(() => props.page.sections.find(sectionMedia));
  return (
    <article class="manual-feature">
      <p class="manual-crumb">
        <a href={manualRoot}>Manual</a>
        <Show when={props.chapter}>
          {' › '}
          {props.chapter}
        </Show>
      </p>
      <h1>{props.page.title}</h1>
      <Show when={props.page.introduction}>
        <div class="manual-introduction">
          <Markdown text={props.page.introduction} />
        </div>
      </Show>
      <Show when={props.page.sections.length > 1}>
        <nav class="manual-entry-links" aria-label="On this page">
          <For each={props.page.sections}>
            {(section) => <a href={`${props.path}#${section.anchor}`}>{section.title}</a>}
          </For>
        </nav>
      </Show>
      <For each={props.page.sections}>
        {(section) => <ManualEntry section={section} wideVideo={section === leadEntry()} />}
      </For>
    </article>
  );
}
