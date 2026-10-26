import { For } from 'solid-js';
import './segmented-control.css';

export interface SegmentedOption<T> {
  value: T;
  label: string;
  /** Extra data-* attributes for one option, such as a test or style hook. */
  data?: Record<`data-${string}`, string>;
}

/** One choice among a few options, shown as joined buttons with the current one lit. */
export function SegmentedControl<T>(props: {
  id?: string;
  label: string;
  options: readonly SegmentedOption<T>[];
  value: T | undefined;
  disabled?: boolean;
  onSelect: (value: T) => void;
}) {
  return (
    <div id={props.id} class="segmented" role="group" aria-label={props.label}>
      <For each={props.options}>
        {(option) => (
          <button
            type="button"
            class={{ 'segmented-option': true, selected: props.value === option.value }}
            aria-pressed={props.value === option.value ? 'true' : 'false'}
            disabled={props.disabled}
            {...option.data}
            onClick={() => props.onSelect(option.value)}
          >
            {option.label}
          </button>
        )}
      </For>
    </div>
  );
}
