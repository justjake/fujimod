import { For, Show } from 'solid-js';
import { FeatureIcon as Icon } from './FeatureIcon';
import type { Credits, Inline } from './catalog';

/** Pieces the feature catalog and the outline share. */
const RATIO = /^ratio:(\d+(?:\.\d+)?):(\d+(?:\.\d+)?)$/;

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
export function ChipIcon(props: { name: string }) {
  const ratio = () => RATIO.exec(props.name);
  return (
    <Show when={ratio()} fallback={<Icon name={props.name} />}>
      {(match) => <Ratio width={Number(match()[1])} height={Number(match()[2])} />}
    </Show>
  );
}

/** Summary text, keeping slot ranges such as C1–C7 on one line. */
export function Prose(props: { text: string }) {
  return (
    <For each={props.text.split(/(C\d–C\d)/).filter(Boolean)}>
      {(part) => (/^C\d–C\d$/.test(part) ? <span class="nowrap">{part}</span> : part)}
    </For>
  );
}

export function Rich(props: { inline: Inline[] }) {
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
export function Premiere() {
  return (
    <div class="catalog-premiere">
      <Icon name="ticket" />
      WORLD PREMIERE
    </div>
  );
}

export function CreditList(props: { credits: Credits }) {
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
