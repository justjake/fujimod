import { For, Show, createEffect, createSignal, onCleanup } from 'solid-js';
import { Portal, type JSX } from '@solidjs/web';
import { manualPageAt } from '../docs/manual-data';
import { manualPagePath } from '../docs/manual-routes';
import { catalog, type Credits } from '../home/catalog';
import { CreditList, Premiere } from '../home/catalog-parts';
import { fadeLoop } from '../home/fade-loop';
import { featureMedia, type Media } from '../home/feature-media';
import './feature-hover-card.css';

const recording = featureMedia();

function CardVideo(props: { clip: Media; range?: [number, number]; fail: () => void }) {
  let element!: HTMLVideoElement;
  let loop: ReturnType<typeof fadeLoop>;
  onCleanup(() => loop?.dispose());
  return (
    <video
      ref={(video) => {
        element = video;
        loop = fadeLoop(video, () => props.range?.[0] ?? 0);
      }}
      class="catalog-card-video"
      src={`/artifacts/docs/${props.clip.video}`}
      poster={`/artifacts/docs/${props.clip.still}`}
      muted
      autoplay
      playsinline
      preload="metadata"
      onLoadedMetadata={() => {
        if (props.range) element.currentTime = props.range[0];
      }}
      onTimeUpdate={() => {
        if (props.range && element.currentTime >= props.range[1]) loop?.restart();
      }}
      onError={props.fail}
    />
  );
}

export function ManualFeatureLink(props: {
  page: string;
  /** The manual menu row's title or anchor. */
  row?: string;
  credits?: Credits;
  card?: string;
  choice?: string;
  children: JSX.Element;
}) {
  const path = () => (props.page.startsWith('/') ? props.page : manualPagePath(props.page));
  const page = () => manualPageAt(path());
  const section = () =>
    page()?.sections.find((entry) => entry.anchor === props.row || entry.title === props.row);
  const href = () => (section() ? `${path()}#${section()!.anchor}` : path());
  const cardId = () => props.card ?? section()?.media?.card;
  const card = () => (cardId() ? catalog.cards[cardId()!] : undefined);
  const media = () => (cardId() ? recording(cardId()!) : null);
  const range = () => (props.choice ? media()?.clips[props.choice] : undefined);
  const credits = () =>
    props.credits ??
    (props.choice ? card()?.choice_credits[props.choice] : card()?.credits) ??
    section()?.credits;
  const title = () => props.choice ?? section()?.title ?? card()?.title ?? page()?.title;
  const summary = () => {
    const choice = props.choice;
    const entry = card();
    return (
      (choice && entry?.choices[choice]) ||
      entry?.changes ||
      entry?.summary ||
      section()?.summary ||
      page()?.summary
    );
  };
  const [open, setOpen] = createSignal(false);
  const [position, setPosition] = createSignal({ left: 8, top: 8 });
  const [failed, setFailed] = createSignal(false);
  let anchor!: HTMLAnchorElement;
  let panel: HTMLElement | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let pressed = false;
  let touch = false;

  function later(value: boolean, delay: number) {
    clearTimeout(timer);
    timer = setTimeout(() => setOpen(value), delay);
  }
  function close() {
    clearTimeout(timer);
    setOpen(false);
  }
  onCleanup(() => clearTimeout(timer));
  createEffect(open, (active) => {
    if (!active) return;
    const place = () => {
      const rect = anchor.getBoundingClientRect();
      const width = Math.min(340, innerWidth - 16);
      const height = panel?.offsetHeight ?? 0;
      const below = rect.bottom + 8;
      const top = below + height <= innerHeight - 8 ? below : rect.top - height - 8;
      setPosition({
        left: Math.max(8, Math.min(rect.left + (rect.width - width) / 2, innerWidth - width - 8)),
        top: Math.max(8, top),
      });
    };
    place();
    const frame = requestAnimationFrame(place);
    const outside = (event: Event) => {
      const node = event.target;
      if (node instanceof Node && !anchor.contains(node) && !panel?.contains(node)) close();
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') close();
    };
    document.addEventListener('pointerdown', outside);
    document.addEventListener('focusin', outside);
    document.addEventListener('keydown', escape);
    window.addEventListener('resize', place);
    window.addEventListener('scroll', place, { passive: true });
    onCleanup(() => {
      cancelAnimationFrame(frame);
      document.removeEventListener('pointerdown', outside);
      document.removeEventListener('focusin', outside);
      document.removeEventListener('keydown', escape);
      window.removeEventListener('resize', place);
      window.removeEventListener('scroll', place);
    });
  });

  return (
    <>
      <a
        ref={(element) => {
          anchor = element;
        }}
        class="manual-feature-link"
        href={href()}
        onPointerEnter={(event) => {
          if (event.pointerType === 'mouse') later(true, 200);
        }}
        onPointerLeave={(event) => {
          if (event.pointerType === 'mouse') later(false, 200);
        }}
        onPointerDown={(event) => {
          pressed = true;
          touch = event.pointerType === 'touch';
        }}
        onPointerCancel={() => {
          pressed = touch = false;
        }}
        onFocus={() => {
          if (pressed) return;
          clearTimeout(timer);
          setOpen(true);
        }}
        onClick={(event) => {
          const tapped = touch || (event.detail > 0 && matchMedia('(hover: none)').matches);
          pressed = touch = false;
          if (tapped && !open()) {
            event.preventDefault();
            clearTimeout(timer);
            setOpen(true);
          }
        }}
      >
        {props.children}
      </a>
      <Show when={open()}>
        <Portal>
          <aside
            ref={(element) => {
              panel = element;
            }}
            class="catalog-card on"
            aria-label={title()}
            style={{ left: `${position().left}px`, top: `${position().top}px` }}
            onPointerEnter={() => clearTimeout(timer)}
            onPointerLeave={(event) => {
              if (event.pointerType === 'mouse') later(false, 200);
            }}
          >
            <Show when={!failed() && media()}>
              {(clip) => <CardVideo clip={clip()} range={range()} fail={() => setFailed(true)} />}
            </Show>
            <Show when={card()?.path.join(' › ') || section()?.paths?.[0]?.join(' › ')}>
              {(crumbs) => <div class="catalog-path">{crumbs()}</div>}
            </Show>
            <div class="catalog-card-title">{title()}</div>
            <Show when={credits()}>
              {(credit) => (
                <Show when={credit().premiere}>
                  <Premiere />
                </Show>
              )}
            </Show>
            <Show when={summary()}>{(text) => <p>{text()}</p>}</Show>
            <Show when={credits()}>{(credit) => <CreditList credits={credit()} />}</Show>
            <Show when={!props.choice && card()?.related.length}>
              <h5 class="catalog-card-heading">Related</h5>
              <ul class="catalog-card-related">
                <For each={card()?.related}>
                  {(related) => (
                    <li>
                      <b>{catalog.cards[related.card]?.title}</b> {related.reason}
                    </li>
                  )}
                </For>
              </ul>
            </Show>
            <a class="catalog-card-manual" href={href()}>
              Read in the manual →
            </a>
          </aside>
        </Portal>
      </Show>
    </>
  );
}
