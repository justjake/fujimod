import { Show } from 'solid-js';
import type { JSX } from '@solidjs/web';
import type { ResolvedSelection } from 'fujimod';
import type { Camera } from '../camera-model';
import type { CaptureSet } from './capture';

export const emptySet = (firmwareId: string): CaptureSet => ({
  firmwareId,
  states: [],
  mappings: [],
});
export const message = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** The capture tools' frame: connect first, then refuse cameras without located bulk reads. */
export function CapturePanel(props: {
  title: string;
  camera?: Camera;
  selection?: ResolvedSelection;
  onConnect: () => void;
  onClose: () => void;
  children: JSX.Element;
}) {
  const identity = () =>
    [props.camera?.identity?.product, props.camera?.identity?.internalVersion]
      .filter(Boolean)
      .join(' ');
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
            <p class="wb-status" role="status">
              {props.camera?.identity
                ? `No located bulk reads for ${identity() || props.camera.name}.`
                : 'Reading camera identity…'}
            </p>
          }
        >
          {props.children}
        </Show>
      </Show>
    </section>
  );
}
