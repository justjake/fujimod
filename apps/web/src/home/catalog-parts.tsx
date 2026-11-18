import { For, Show } from 'solid-js';
import { FeatureIcon as Icon } from './FeatureIcon';
import type { Credits, Inline } from './catalog';

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
