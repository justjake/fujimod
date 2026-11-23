import { For, Show, createSignal, onSettled } from 'solid-js';
import type { ManualHit } from './manual-search';
import './manual-search.css';

type Search = typeof import('./manual-search');

/** A word of text, marked when it is one of the matched words. */
function Highlighted(props: {
  text: string;
  terms: readonly string[];
  normalize: Search['normalizeTerm'];
}) {
  const parts = () => props.text.split(/([\p{L}\p{N}-]+)/u);
  return (
    <For each={parts()}>
      {(part) => (props.terms.includes(props.normalize(part)) ? <mark>{part}</mark> : <>{part}</>)}
    </For>
  );
}

/** Search box over every manual page and setting. Results drop below the
 * box; on narrow screens the search fills the screen. Arrow keys choose a
 * result, Return opens it, Escape closes; Command-K or / focuses the box. */
export function ManualSearch(props: { label: string }) {
  const [query, setQuery] = createSignal('');
  const [open, setOpen] = createSignal(false);
  const [active, setActive] = createSignal(0);
  const [search, setSearch] = createSignal<Search>();
  let input!: HTMLInputElement;
  let list: HTMLUListElement | undefined;

  const load = () => {
    if (!search()) void import('./manual-search').then((module) => setSearch(() => module));
  };
  const hits = (): ManualHit[] => search()?.searchManual(query()) ?? [];

  const close = () => {
    setOpen(false);
    input.blur();
  };
  const choose = (hit: ManualHit | undefined) => {
    if (!hit) return;
    setOpen(false);
    location.assign(hit.href);
  };
  const scrollActive = () =>
    list?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });

  const key = (event: KeyboardEvent) => {
    const count = hits().length;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      if (!count) return;
      event.preventDefault();
      setActive((active() + (event.key === 'ArrowDown' ? 1 : count - 1)) % count);
      queueMicrotask(scrollActive);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      choose(hits()[active()]);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      close();
    }
  };

  onSettled(() => {
    const shortcut = (event: KeyboardEvent) => {
      const typing = (event.target as Element | null)?.closest(
        'input, textarea, [contenteditable]',
      );
      if (
        ((event.metaKey || event.ctrlKey) && event.key === 'k') ||
        (event.key === '/' && !typing)
      ) {
        event.preventDefault();
        input.focus();
      }
    };
    const outside = (event: PointerEvent) => {
      if (open() && !(event.target as Element).closest('.manual-search')) setOpen(false);
    };
    window.addEventListener('keydown', shortcut);
    window.addEventListener('pointerdown', outside);
    return () => {
      window.removeEventListener('keydown', shortcut);
      window.removeEventListener('pointerdown', outside);
    };
  });

  return (
    <div class={{ 'manual-search': true, open: open() }}>
      <div class="manual-search-bar">
        <input
          ref={(element) => (input = element)}
          type="search"
          role="combobox"
          aria-label={props.label}
          aria-expanded={open() && hits().length > 0 ? 'true' : 'false'}
          aria-controls="manual-search-results"
          aria-activedescendant={open() && hits().length ? `manual-search-${active()}` : undefined}
          placeholder={props.label}
          autocomplete="off"
          spellcheck={false}
          value={query()}
          onFocus={() => {
            load();
            setOpen(true);
          }}
          onInput={(event) => {
            setQuery(event.currentTarget.value);
            setActive(0);
            setOpen(true);
          }}
          onKeyDown={key}
        />
        <button type="button" class="manual-search-cancel" onClick={close}>
          Cancel
        </button>
      </div>
      <Show when={open() && query().trim() ? search() : undefined}>
        {(module) => (
          <ul
            id="manual-search-results"
            role="listbox"
            aria-label={props.label}
            ref={(element) => (list = element)}
          >
            <For each={hits()} fallback={<li class="manual-search-empty">No results</li>}>
              {(hit, index) => (
                <li
                  id={`manual-search-${index()}`}
                  role="option"
                  aria-selected={index() === active() ? 'true' : 'false'}
                  onPointerMove={() => void setActive(index())}
                >
                  <a href={hit.href} onClick={() => setOpen(false)}>
                    <b>
                      <Highlighted
                        text={hit.title}
                        terms={hit.terms}
                        normalize={module().normalizeTerm}
                      />
                    </b>
                    <Show when={hit.trail.length}>
                      <span class="manual-search-trail">{hit.trail.join(' › ')}</span>
                    </Show>
                    <Show when={hit.snippet}>
                      <span class="manual-search-snippet">
                        <Highlighted
                          text={hit.snippet}
                          terms={hit.terms}
                          normalize={module().normalizeTerm}
                        />
                      </span>
                    </Show>
                  </a>
                </li>
              )}
            </For>
          </ul>
        )}
      </Show>
    </div>
  );
}
