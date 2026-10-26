import { For, Show, createSignal, onCleanup, untrack } from 'solid-js';
import type { JSX } from '@solidjs/web';
import featureIcons from '../ui/assets/feature-icons.svg?url';
import iconMetrics from '../ui/assets/feature-icon-metrics.generated.json';
import catalogData from './features.generated.json';
import { FeatureTile } from './FeatureTile';
import { fadeLoop } from './fade-loop';
import { featureMedia, type Media } from './feature-media';
import './features.css';

/** Inline Markdown from the docs: text, bold, italic and links. */
type Inline = string | { b: Inline[] } | { i: Inline[] } | { href: string; text: Inline[] };
type Credits = { premiere: boolean; inspired: Inline[][]; thanks: Inline[][] };
/** A hover card: one control, its options and what FujiMod changed. */
type Card = {
  title: string;
  path: string[];
  summary: string | null;
  choices: Record<string, string>;
  related: { card: string; reason: string }[];
  changes: string | null;
  credits: Credits;
  choice_credits: Record<string, Credits>;
};
type Choice = { label: string; stock: boolean; icon: string | null };
type Control = {
  title: string;
  primary: boolean;
  card: string | null;
  choices: Choice[];
  actions: string[];
};
/** A catalog tile: one feature, its controls and the cards its preview may come from. */
type Feature = {
  id: string;
  group: string;
  icon: string;
  title: string;
  path: string[];
  summary: string;
  card: string | null;
  media_cards: string[];
  controls: Control[];
  credits: Credits;
};
/** The docs' website copy (python -m tools.walkthroughs docs --site-data). */
type Catalog = { schema: 1; groups: string[]; features: Feature[]; cards: Record<string, Card> };
/** Ink box and the symbol's own viewBox (symbol units), and the optical width and height (scripts/icon-metrics.ts). */
type IconMetric = { box: number[]; view: number[]; width: number; height: number };
/** What a hover card shows: a control, or one option of it. */
type Target = { card: string; choice?: string; anchor: HTMLElement };

const catalog = catalogData as Catalog;
const CARDS = catalog.cards;
const GROUP_TITLES: Record<string, string> = {
  pictures: 'Making pictures',
  movies: 'Making movies',
  handling: 'Handling and display',
  problems: 'When something goes wrong',
};
const MEDIA = '/artifacts/docs/';
const media = featureMedia();
const RATIO = /^ratio:(\d+):(\d+)$/;

/** A sprite icon cropped to its ink, sized by the context's --icon-size so every icon has the same optical size. */
function Icon(props: { name: string }) {
  const metric = () => (iconMetrics as Record<string, IconMetric>)[props.name];
  return (
    <svg
      class="catalog-icon"
      aria-hidden="true"
      viewBox={metric().box.join(' ')}
      style={{ '--w': metric().width, '--h': metric().height }}
    >
      <use
        href={`${featureIcons}#${props.name}`}
        x={metric().view[0]}
        y={metric().view[1]}
        width={metric().view[2]}
        height={metric().view[3]}
      />
    </svg>
  );
}

/** An aspect ratio choice drawn as its frame. */
function Ratio(props: { width: number; height: number }) {
  const scale = () => 14 / Math.max(props.width, props.height);
  return (
    <svg class="catalog-ratio" viewBox="0 0 14 14" aria-hidden="true">
      <rect
        x={(14 - props.width * scale()) / 2}
        y={(14 - props.height * scale()) / 2}
        width={props.width * scale()}
        height={props.height * scale()}
        rx="1"
      />
    </svg>
  );
}

/** A chip's icon: a sprite symbol, or "ratio:W:H" drawn as its frame. */
function ChipIcon(props: { name: string }) {
  const ratio = () => RATIO.exec(props.name);
  return (
    <Show when={ratio()} fallback={<Icon name={props.name} />}>
      {(match) => <Ratio width={Number(match()[1])} height={Number(match()[2])} />}
    </Show>
  );
}

/** Summary text, keeping slot ranges such as C1–C7 on one line. */
function Prose(props: { text: string }) {
  return (
    <For each={props.text.split(/(C\d–C\d)/).filter(Boolean)}>
      {(part) => (/^C\d–C\d$/.test(part) ? <span class="nowrap">{part}</span> : part)}
    </For>
  );
}

function Rich(props: { inline: Inline[] }) {
  return (
    <For each={props.inline}>
      {(part) =>
        typeof part === 'string' ? (
          part
        ) : 'b' in part ? (
          <b>
            <Rich inline={part.b} />
          </b>
        ) : 'i' in part ? (
          <i>
            <Rich inline={part.i} />
          </i>
        ) : (
          <a href={part.href} target="_blank" rel="noreferrer">
            <Rich inline={part.text} />
          </a>
        )
      }
    </For>
  );
}

/** First offered in any digital camera. */
function Premiere() {
  return (
    <div class="catalog-premiere">
      <Icon name="ticket" />
      WORLD PREMIERE
    </div>
  );
}

function CreditList(props: { credits: Credits }) {
  return (
    <Show when={props.credits.inspired.length || props.credits.thanks.length}>
      <ul class="catalog-credits">
        <For each={props.credits.inspired}>
          {(line) => (
            <li>
              <Icon name="heart" />
              <b>INSPIRED BY</b>: <Rich inline={line} />
            </li>
          )}
        </For>
        <For each={props.credits.thanks}>
          {(line) => (
            <li>
              <Icon name="gift" />
              <b>THANKS</b>: <Rich inline={line} />
            </li>
          )}
        </For>
      </ul>
    </Show>
  );
}

/** What tiles and hover cards share: the menus above the title, the title, WORLD PREMIERE, the description,
    the credits, then the rest (options or Related). */
function Entry(props: {
  path: string[];
  title: JSX.Element;
  description: JSX.Element;
  credits: Credits;
  children?: JSX.Element;
}) {
  return (
    <>
      <Show when={props.path.length}>
        <div class="catalog-path">{`${props.path.join(' › ')} ›`}</div>
      </Show>
      {props.title}
      <Show when={props.credits.premiere}>
        <Premiere />
      </Show>
      {props.description}
      <CreditList credits={props.credits} />
      {props.children}
    </>
  );
}

function CardVideo(props: { media: Media; choice?: string }) {
  let video: HTMLVideoElement | undefined;
  // A site without feature media (or a missing file) shows no video at all.
  const [failed, setFailed] = createSignal(false);
  // Read once: the card's own range, still valid while its video is closing.
  const range = untrack(() => (props.choice ? props.media.clips[props.choice] : undefined));
  const clip = () => range;
  let loop: ReturnType<typeof fadeLoop> | undefined;
  onCleanup(() => loop?.dispose());
  return (
    <Show when={!failed()}>
      <video
        ref={(element) => {
          video = element;
          loop = fadeLoop(element, () => clip()?.[0] ?? 0);
        }}
        class="catalog-card-video"
        src={MEDIA + props.media.video}
        poster={MEDIA + props.media.still}
        muted
        autoplay
        playsinline
        preload="metadata"
        onLoadedMetadata={() => {
          const range = clip();
          if (video && range) video.currentTime = range[0];
        }}
        onTimeUpdate={() => {
          const range = clip();
          if (video && range && video.currentTime >= range[1]) loop?.restart();
        }}
        onError={() => setFailed(true)}
      />
    </Show>
  );
}

const NO_CREDITS: Credits = { premiere: false, inspired: [], thanks: [] };

function CardBody(props: { target: Target }) {
  const card = () => CARDS[props.target.card];
  const choice = () => props.target.choice;
  /** The menus above the title: a control's menus, and for an option its control too. */
  const crumbs = () => (choice() ? [...card().path, card().title] : card().path);
  const credits = () =>
    choice() ? (card().choice_credits[choice()!] ?? NO_CREDITS) : card().credits;
  return (
    <>
      <Show when={media(props.target.card)}>
        {(found) => <CardVideo media={found()} choice={choice()} />}
      </Show>
      <Entry
        path={crumbs()}
        title={<div class="catalog-card-title">{choice() ?? card().title}</div>}
        credits={credits()}
        description={
          <Show
            when={choice()}
            fallback={
              <Show when={card().changes} fallback={<p>{card().summary}</p>}>
                <p>In FujiMod: {card().changes}</p>
              </Show>
            }
          >
            <p>{card().choices[choice()!]}</p>
          </Show>
        }
      >
        <Show when={!choice() && card().related.length}>
          <h5 class="catalog-card-heading">Related</h5>
          <ul class="catalog-card-related">
            <For each={card().related}>
              {(related) => (
                <li>
                  <b>{CARDS[related.card].title}</b>: {related.reason}
                </li>
              )}
            </For>
          </ul>
        </Show>
      </Entry>
    </>
  );
}

export function FeatureCatalog() {
  const [target, setTarget] = createSignal<Target>();
  const [position, setPosition] = createSignal({ left: 0, top: 0 });
  let timer: ReturnType<typeof setTimeout> | undefined;
  let card: HTMLDivElement | undefined;
  onCleanup(() => clearTimeout(timer));

  const place = (anchor: HTMLElement) => {
    const r = anchor.getBoundingClientRect();
    const width = Math.min(340, innerWidth - 16);
    const height = card?.offsetHeight ?? 0;
    let top = r.bottom + 10;
    if (top + height > innerHeight - 8) top = Math.max(8, r.top - height - 10);
    setPosition({
      left: Math.min(Math.max(8, r.left + r.width / 2 - width / 2), innerWidth - width - 8),
      top,
    });
  };
  const show = (next: Target) => {
    setTarget(next);
    place(next.anchor);
    requestAnimationFrame(() => place(next.anchor));
  };
  const later = (action: () => void, ms: number) => {
    clearTimeout(timer);
    timer = setTimeout(action, ms);
  };
  /** A control or option with a hover card: after a short hover, on focus or on tap. */
  const Ref = (props: { card: string; choice?: string; class?: string; children: JSX.Element }) => (
    <span
      class={props.class ? `${props.class} catalog-ref` : 'catalog-ref'}
      tabindex="0"
      onMouseEnter={(event) => {
        const anchor = event.currentTarget;
        later(() => show({ card: props.card, choice: props.choice, anchor }), 250);
      }}
      onMouseLeave={() => later(() => setTarget(), 150)}
      onFocus={(event) =>
        show({ card: props.card, choice: props.choice, anchor: event.currentTarget })
      }
      onBlur={() => later(() => setTarget(), 150)}
      onClick={(event) => {
        const anchor = event.currentTarget;
        if (target()?.anchor === anchor) setTarget();
        else show({ card: props.card, choice: props.choice, anchor });
      }}
    >
      {props.children}
    </span>
  );

  /** An option chip; the X70's own options are dimmed. Options with a card description open it. */
  const Chip = (props: { choice: Choice; card: string | null }) => {
    const cls = () => (props.choice.stock ? 'catalog-chip stock' : 'catalog-chip');
    const body = (
      <>
        <Show when={props.choice.icon}>{(name) => <ChipIcon name={name()} />}</Show>
        {props.choice.label}
      </>
    );
    return (
      <Show
        when={props.card && props.choice.label in CARDS[props.card].choices && props.card}
        fallback={<span class={cls()}>{body}</span>}
      >
        {(card) => (
          <Ref card={card()} choice={props.choice.label} class={cls()}>
            {body}
          </Ref>
        )}
      </Show>
    );
  };

  const Chips = (props: { control: Control }) => (
    <div class="catalog-chips">
      <For each={props.control.choices}>
        {(choice) => <Chip choice={choice} card={props.control.card} />}
      </For>
      <For each={props.control.actions}>
        {(action) => <span class="catalog-chip action">{action}</span>}
      </For>
    </div>
  );

  const Tile = (props: { feature: Feature }) => (
    <FeatureTile media={media(...props.feature.media_cards)}>
      <Entry
        path={props.feature.path}
        title={
          <h4>
            <Icon name={props.feature.icon} />
            <Show when={props.feature.card} fallback={<span>{props.feature.title}</span>}>
              {(card) => <Ref card={card()}>{props.feature.title}</Ref>}
            </Show>
          </h4>
        }
        credits={props.feature.credits}
        description={
          <p>
            <Prose text={props.feature.summary} />
          </p>
        }
      >
        <For each={props.feature.controls}>
          {(control) => (
            <Show when={!control.primary} fallback={<Chips control={control} />}>
              <div class="catalog-sub">
                <h5>
                  <Show when={control.card} fallback={control.title}>
                    {(card) => <Ref card={card()}>{control.title}</Ref>}
                  </Show>
                </h5>
                <Chips control={control} />
              </div>
            </Show>
          )}
        </For>
      </Entry>
    </FeatureTile>
  );

  return (
    <section class="catalog" id="features" aria-labelledby="catalog-heading">
      <h2 id="catalog-heading">Features</h2>
      <For each={catalog.groups.filter((group) => catalog.features.some((f) => f.group === group))}>
        {(group) => (
          <>
            <h3>{GROUP_TITLES[group]}</h3>
            <div class="catalog-grid">
              <For each={catalog.features.filter((f) => f.group === group)}>
                {(feature) => <Tile feature={feature} />}
              </For>
            </div>
          </>
        )}
      </For>
      <div
        ref={(element) => {
          card = element;
        }}
        class={['catalog-card', { on: !!target() }]}
        role="tooltip"
        style={{ left: `${position().left}px`, top: `${position().top}px` }}
        onMouseEnter={() => clearTimeout(timer)}
        onMouseLeave={() => later(() => setTarget(), 150)}
      >
        <Show when={target()}>{(current) => <CardBody target={current()} />}</Show>
      </div>
    </section>
  );
}
