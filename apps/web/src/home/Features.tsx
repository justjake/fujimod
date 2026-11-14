import { For, Show, createSignal, onCleanup, onSettled, untrack } from 'solid-js';
import type { JSX } from '@solidjs/web';
import { FeatureIcon as Icon } from './FeatureIcon';
import {
  catalog,
  type Choice,
  type Control,
  type Credits,
  type Feature,
} from './catalog';
import { ChipIcon, CreditList, Premiere, Prose } from './catalog-parts';
import { FeatureTile } from './FeatureTile';
import { fadeLoop } from './fade-loop';
import { featureMedia, type Media } from './feature-media';
import './features.css';

/** What a hover card shows: a control, or one option of it. */
type Target = { card: string; choice?: string; anchor: HTMLElement };

const CARDS = catalog.cards;
const GROUP_TITLES: Record<string, string> = {
  photography: 'Photography',
  videography: 'Videography',
  recipes: 'Recipes',
  system: 'System & Setup',
};
const MEDIA = '/artifacts/docs/';
const media = featureMedia();
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

function CardBody(props: {
  target: Target;
  reference: (card: string, title: string) => JSX.Element;
}) {
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
                  <b>{props.reference(related.card, CARDS[related.card].title)}</b>:{' '}
                  {related.reason}
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
  const [query, setQuery] = createSignal('');
  const features = () => {
    const words = query().trim().toLowerCase().split(/\s+/);
    return catalog.features.filter((feature) => {
      const text = [
        feature.title,
        feature.summary,
        ...feature.path,
        ...feature.controls.flatMap((control) => [
          control.title,
          ...control.choices.map((choice) => choice.label),
        ]),
      ]
        .join(' ')
        .toLowerCase();
      return words.every((word) => text.includes(word));
    });
  };
  const [target, setTarget] = createSignal<Target>();
  const [position, setPosition] = createSignal({ left: 0, top: 0 });
  let timer: ReturnType<typeof setTimeout> | undefined;
  let card: HTMLDivElement | undefined;
  let heading: HTMLDivElement | undefined;
  let pinned = false;
  onCleanup(() => clearTimeout(timer));
  const cardLink = (id?: string) => {
    const params = new URLSearchParams();
    if (query()) params.set('search', query());
    if (id) params.set('card', id);
    history.replaceState(history.state, '', `#features${params.size ? `?${params}` : ''}`);
  };
  const close = (updateLink = true) => {
    clearTimeout(timer);
    pinned = false;
    setTarget();
    if (
      updateLink &&
      location.hash.startsWith('#features?') &&
      new URLSearchParams(location.hash.slice('#features?'.length)).has('card')
    )
      cardLink();
  };
  onSettled(() => {
    const searchFromLink = () => {
      if (location.hash === '#features') {
        setQuery('');
        close();
        return;
      }
      if (!location.hash.startsWith('#features?')) return;
      const params = new URLSearchParams(location.hash.slice('#features?'.length));
      close(false);
      setQuery(params.get('search') ?? '');
      document.getElementById('features')?.scrollIntoView();
      const id = params.get('card');
      if (id && Object.hasOwn(CARDS, id) && heading) {
        pinned = true;
        show({ card: id, anchor: heading });
      }
    };
    searchFromLink();
    window.addEventListener('hashchange', searchFromLink);
    const outside = (event: Event) => {
      const anchor = target()?.anchor;
      if (
        anchor &&
        event.target instanceof Node &&
        !anchor.contains(event.target) &&
        !card?.contains(event.target)
      )
        close();
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') close();
    };
    document.addEventListener('pointerdown', outside);
    document.addEventListener('focusin', outside);
    document.addEventListener('keydown', escape);
    return () => {
      window.removeEventListener('hashchange', searchFromLink);
      document.removeEventListener('pointerdown', outside);
      document.removeEventListener('focusin', outside);
      document.removeEventListener('keydown', escape);
    };
  });

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
    clearTimeout(timer);
    // A Related trigger disappears when its card is replaced; keep the tile's anchor.
    const anchor = (card?.contains(next.anchor) && target()?.anchor) || next.anchor;
    setTarget({ ...next, anchor });
    place(anchor);
    requestAnimationFrame(() => place(anchor));
  };
  const later = (action: () => void, ms: number) => {
    clearTimeout(timer);
    timer = setTimeout(action, ms);
  };
  /** A control or option with a hover card: after a short hover, on focus or on tap. */
  const Ref = (props: { card: string; choice?: string; class?: string; children: JSX.Element }) => {
    let pointerPinned: boolean | undefined;
    const activate = (anchor: HTMLElement, dismiss = pinned && target()?.anchor === anchor) => {
      // Focus can open the card before the same tap delivers its click. Only
      // an already pinned card should toggle closed on activation.
      if (dismiss) close();
      else {
        pinned = true;
        cardLink(props.card);
        show({ card: props.card, choice: props.choice, anchor });
      }
    };
    return (
      <span
        class={props.class ? `${props.class} catalog-ref` : 'catalog-ref'}
        role="button"
        tabindex="0"
        onPointerDown={(event) => {
          // Safari can move focus to the tile during a tap. Keep the state
          // from before that focus change so a second tap still dismisses.
          pointerPinned = pinned && target()?.anchor === event.currentTarget;
        }}
        onPointerCancel={() => {
          pointerPinned = undefined;
        }}
        onPointerEnter={(event) => {
          if (event.pointerType !== 'mouse' || pinned) return;
          const anchor = event.currentTarget;
          later(() => show({ card: props.card, choice: props.choice, anchor }), 250);
        }}
        onPointerLeave={(event) => {
          if (event.pointerType === 'mouse' && !pinned) later(close, 150);
        }}
        onFocus={(event) => {
          // Related references stay focusable until Enter or Space activates them.
          if (card?.contains(event.currentTarget)) {
            clearTimeout(timer);
            return;
          }
          // Opening during pointer focus can cover the trigger before mouseup
          // in Safari, retargeting the click. Let the completed tap open it.
          if (pointerPinned !== undefined) return;
          if (target()?.anchor !== event.currentTarget) pinned = false;
          show({ card: props.card, choice: props.choice, anchor: event.currentTarget });
        }}
        onBlur={(event) => {
          if (
            !pinned &&
            !(event.relatedTarget instanceof Node && card?.contains(event.relatedTarget))
          )
            later(close, 150);
        }}
        onClick={(event) => {
          activate(event.currentTarget, pointerPinned);
          pointerPinned = undefined;
        }}
        onKeyDown={(event) => {
          if ((event.key === 'Enter' || event.key === ' ') && !event.repeat) {
            event.preventDefault();
            activate(event.currentTarget);
          }
        }}
      >
        {props.children}
      </span>
    );
  };

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
          <h3>
            <Icon name={props.feature.icon} />
            <Show when={props.feature.card} fallback={<span>{props.feature.title}</span>}>
              {(card) => <Ref card={card()}>{props.feature.title}</Ref>}
            </Show>
          </h3>
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
                <h4>
                  <Show when={control.card} fallback={control.title}>
                    {(card) => <Ref card={card()}>{control.title}</Ref>}
                  </Show>
                </h4>
                <Chips control={control} />
              </div>
            </Show>
          )}
        </For>
      </Entry>
    </FeatureTile>
  );

  return (
    <div class="catalog" id="features">
      <div class="catalog-heading" ref={(element) => (heading = element)}>
        <div>
          <h2>Explore features</h2>
          <p>{catalog.features.length} features · X70 catalog. Availability varies by camera.</p>
          <p>Menu previews may show earlier builds.</p>
        </div>
        <label class="catalog-search">
          <span>Find a feature</span>
          <input
            type="search"
            value={query()}
            onInput={(event) => {
              close();
              setQuery(event.currentTarget.value);
            }}
            placeholder="Film, focus, custom settings…"
          />
        </label>
      </div>
      <p class="catalog-results" role="status" hidden={!query().trim()}>
        {features().length
          ? `${features().length} of ${catalog.features.length} features`
          : 'No matching features.'}
      </p>
      <For each={catalog.groups.filter((group) => features().some((f) => f.group === group))}>
        {(group) => (
          <section class="catalog-group" aria-labelledby={`catalog-${group}-heading`}>
            <h2 id={`catalog-${group}-heading`}>{GROUP_TITLES[group]}</h2>
            <div class="catalog-grid">
              <For each={features().filter((f) => f.group === group)}>
                {(feature) => <Tile feature={feature} />}
              </For>
            </div>
          </section>
        )}
      </For>
      <div
        ref={(element) => {
          card = element;
        }}
        class={['catalog-card', { on: !!target() }]}
        role="tooltip"
        style={{ left: `${position().left}px`, top: `${position().top}px` }}
        onPointerEnter={(event) => {
          if (event.pointerType === 'mouse') clearTimeout(timer);
        }}
        onPointerLeave={(event) => {
          if (event.pointerType === 'mouse' && !pinned) later(close, 150);
        }}
      >
        <Show when={target()} keyed>
          {(current) => (
            <CardBody
              target={current}
              reference={(card, title) => <Ref card={card}>{title}</Ref>}
            />
          )}
        </Show>
      </div>
    </div>
  );
}
