import { For, Show } from 'solid-js';

import { APPEAR, BODY, CAPTION, DIM, EASE, HEADER, POP_MS, RAISED } from './materials';

/** A modal question, as the camera asks one: a title bar, an optional line
 * of explanation, then the actions as a list, the cursor the focus plate and
 * stripe. Nothing is colored; a discarding action is set apart by its place,
 * just before CANCEL. */
export function Confirmation(props: {
  title: string;
  message?: string;
  actions: readonly string[];
  focused: number;
  onSelect: (index: number) => void;
  onCancel?: () => void;
}) {
  return (
    <div
      class="absolute inset-0 z-40 flex items-center justify-center"
      style={{
        background: 'rgb(0 0 0 / 0.6)',
        animation: `fm-fade ${POP_MS}ms ${EASE} both`,
      }}
      onClick={() => props.onCancel?.()}
    >
      <div
        class="flex w-[440px] flex-col overflow-hidden p-[1px]"
        style={{ ...RAISED, ...APPEAR }}
        onClick={(e) => e.stopPropagation()}
      >
        <div
          class={`flex shrink-0 items-center px-[16px] font-semibold ${BODY}`}
          style={{
            height: `${HEADER + 8}px`,
            background: 'var(--m-title)',
            'box-shadow': 'inset 0 -1px 0 var(--m-separator)',
          }}
        >
          {props.title}
        </div>
        <Show when={props.message}>
          <div
            class={`px-[16px] py-[12px] leading-[1.35] ${CAPTION}`}
            style={{
              color: DIM,
              'text-transform': 'none',
              'letter-spacing': '0',
              'box-shadow': 'inset 0 -1px 0 var(--m-separator)',
            }}
          >
            {props.message}
          </div>
        </Show>
        <For each={props.actions}>
          {(action, i) => {
            const active = () => props.focused === i();
            return (
              <button
                type="button"
                class={`flex h-[52px] w-full shrink-0 items-center px-[16px] text-left ${BODY}`}
                style={{
                  background: active() ? 'var(--m-cursor)' : 'transparent',
                  color: active() ? '#fff' : 'rgb(255 255 255 / 0.85)',
                  'box-shadow':
                    [
                      active() ? 'var(--m-cursor-edge)' : '',
                      i() > 0 ? 'inset 0 1px 0 var(--m-separator)' : '',
                    ]
                      .filter(Boolean)
                      .join(', ') || 'none',
                }}
                onClick={() => props.onSelect(i())}
              >
                {action}
              </button>
            );
          }}
        </For>
      </div>
    </div>
  );
}
