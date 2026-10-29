import { Show } from 'solid-js';
import './bulk-support.css';

/** Explain the published failure beside a control blocked by bulk support. */
export function BulkSupportError(props: { reason?: string }) {
  return (
    <Show when={props.reason}>
      <p class="wb-bulk-error" role="status">
        <strong>{props.reason}</strong>
      </p>
    </Show>
  );
}
