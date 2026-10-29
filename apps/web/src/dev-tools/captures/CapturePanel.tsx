import { Show } from 'solid-js';
import type { JSX } from '@solidjs/web';
import type { Camera } from '../camera-model';
import { bulkSupportError, type BulkSelection, type CaptureSet } from './capture';
import { BulkSupportError } from '../BulkSupportError';

export const emptySet = (firmwareId: string): CaptureSet => ({
  firmwareId,
  states: [],
  mappings: [],
});

/** The capture tools' frame: connect first, then refuse cameras without located bulk reads. */
export function CapturePanel(props: {
  title: string;
  camera?: Camera;
  selection?: BulkSelection;
  onConnect: () => void;
  onClose: () => void;
  children: JSX.Element;
}) {
  return (
    <section class="wb-tools-body cap" aria-labelledby="cap-title">
      <div class="cap-head">
        <button class="wb-text" onClick={() => props.onClose()}>
          ← Tools
        </button>
        <h2 id="cap-title" class="cap-title">
          {props.title}
        </h2>
        <Show when={props.selection}>
          {(selection) => (
            <span class="cap-dim">
              {selection().bulk?.product} {selection().firmware.version}
            </span>
          )}
        </Show>
      </div>
      <Show
        when={props.camera?.present}
        fallback={
          <button class="wb-btn wb-primary" onClick={() => props.onConnect()}>
            Connect camera
          </button>
        }
      >
        <Show
          when={props.selection}
          fallback={
            <Show
              when={props.camera?.identity}
              fallback={<p class="wb-status">Reading camera identity…</p>}
            >
              <BulkSupportError reason={bulkSupportError(props.camera?.identity)} />
            </Show>
          }
        >
          {props.children}
        </Show>
      </Show>
    </section>
  );
}
