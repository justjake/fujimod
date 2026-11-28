import { For, Show } from 'solid-js';

export interface ConfirmationAction {
  label: string;
  /** Destructive action (e.g. Discard): muted warm/red tint when unfocused. */
  destructive?: boolean;
}

/** Focal UI's Confirmation dialog: native --m-popup surface with --m-title,
 * explanatory message, and --m-cursor / --m-cursor-edge action rows so it
 * matches Fujimod, High contrast, and Glass seamlessly. */
export function Confirmation(props: {
  title: string;
  message?: string;
  actions: readonly ConfirmationAction[];
  focused: number;
  onSelect: (index: number) => void;
  onCancel?: () => void;
}) {
  return (
    <div
      class="absolute inset-0 z-40 flex items-center justify-center"
      style={{ background: 'rgba(0, 0, 0, 0.62)' }}
      onClick={() => props.onCancel?.()}
    >
      <div
        class="flex w-[410px] flex-col overflow-hidden rounded-[12px] p-[1px]"
        style={{
          background: 'var(--m-popup)',
          'box-shadow': 'var(--m-popup-edge), 0 18px 40px rgba(0, 0, 0, 0.72)',
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <div
          class="flex h-[38px] items-center px-[16px] text-[17px] leading-none font-semibold tracking-[0.04em] text-white"
          style={{
            background: 'var(--m-title)',
            'box-shadow': 'inset 0 -1px 0 var(--m-separator)',
          }}
        >
          {props.title}
        </div>
        <Show when={props.message}>
          <div
            class="px-[16px] py-[12px] text-[16px] leading-[1.35] text-white/80"
            style={{ 'text-transform': 'none', 'box-shadow': 'inset 0 -1px 0 var(--m-separator)' }}
          >
            {props.message}
          </div>
        </Show>
        <div class="flex flex-col">
          <For each={props.actions}>
            {(action, i) => {
              const active = () => props.focused === i();
              return (
                <button
                  type="button"
                  class="flex h-[42px] w-full items-center justify-center px-[14px] text-[17px] leading-none font-semibold"
                  style={{
                    background: active() ? 'var(--m-cursor)' : 'transparent',
                    'box-shadow':
                      [
                        active() ? 'var(--m-cursor-edge)' : '',
                        i() > 0 ? 'inset 0 1px 0 var(--m-separator)' : '',
                      ]
                        .filter(Boolean)
                        .join(', ') || 'none',
                    color: active()
                      ? '#fff'
                      : action.destructive
                        ? '#ff7b72'
                        : 'rgb(255 255 255 / 0.88)',
                  }}
                  onClick={() => props.onSelect(i())}
                >
                  {action.label}
                </button>
              );
            }}
          </For>
        </div>
      </div>
    </div>
  );
}
