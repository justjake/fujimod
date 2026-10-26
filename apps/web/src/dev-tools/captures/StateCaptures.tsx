import { For, Show, createMemo, createSignal } from 'solid-js';
import type { Camera, CameraModel } from '../camera-model';
import { MIB, bulkSelection, captureRanges, stateScope, type CaptureSet } from './capture';
import { slug, type AddFile } from '../../files/shelf';
import { CAPTURE_STATES, type CaptureState } from './states';
import { CapturePanel, emptySet, message } from './CapturePanel';
import './captures.css';

/** Capture RAM in states the person sets with the camera's own controls. */
export function StateCaptures(props: {
  camera?: Camera;
  model: CameraModel;
  set?: CaptureSet;
  onSet: (set: CaptureSet) => void;
  onFile: AddFile;
  onConnect: () => void;
  onClose: () => void;
}) {
  const selection = createMemo(() => bulkSelection(props.camera?.identity));
  const scope = () => stateScope(selection()?.bulk?.scopes ?? []);
  const [running, setRunning] = createSignal<string>();
  const [progress, setProgress] = createSignal({ done: 0, total: 1 });
  const [failure, setFailure] = createSignal('');
  const [names, setNames] = createSignal<Record<string, string>>({});
  const set = () => props.set ?? emptySet(selection()!.firmware.id);
  const available = () =>
    Boolean(
      props.camera?.present && !props.camera.session && !props.camera.uncertain && !running(),
    );

  function label(state: CaptureState) {
    const name = names()[state.id]?.trim();
    return state.named ? name : state.name;
  }
  async function capture(state: CaptureState) {
    const current = scope();
    const camera = props.camera;
    const firmwareId = selection()?.firmware.id;
    const text = label(state);
    const file = props.onFile;
    if (!current || !camera || !firmwareId || !text || !available()) return;
    setRunning(state.id);
    setFailure('');
    try {
      const read = await props.model.withSession(camera.id, 'Camera mode capture', (session) =>
        captureRanges(session, firmwareId, current, (done, total) => setProgress({ done, total })),
      );
      const capture = {
        state: state.id,
        label: state.named ? `${state.name}: ${text}` : text,
        scope: current.name,
        time: new Date().toISOString(),
        ...read,
      };
      for (const range of capture.ranges)
        file(`capture-${slug(capture.label)}-${slug(range.name)}.bin`, new Blob(range.parts), {
          firmware: firmwareId,
          state: capture.state,
          label: capture.label,
          scope: capture.scope,
          time: capture.time,
          address: range.address,
          size: range.size,
          pieces: range.pieces,
          overwritten: capture.overwritten,
        });
      const previous = set();
      props.onSet({ ...previous, states: [...previous.states, capture] });
      if (state.named) setNames((all) => ({ ...all, [state.id]: '' }));
    } catch (error) {
      setFailure(message(error));
    } finally {
      setRunning(undefined);
    }
  }
  function attach(state: CaptureState, photo: File | undefined) {
    const previous = set();
    const index = previous.states.map((entry) => entry.state).lastIndexOf(state.id);
    if (!photo || index < 0) return;
    const states = [...previous.states];
    states[index] = { ...states[index], photo };
    props.onSet({ ...previous, states });
    const extension = photo.name.split('.').pop()?.toLowerCase() ?? 'jpg';
    props.onFile(`capture-${slug(states[index].label)}.${extension}`, photo, {
      state: state.id,
      label: states[index].label,
      time: states[index].time,
    });
  }
  const count = (state: CaptureState) =>
    set().states.filter((entry) => entry.state === state.id).length;

  return (
    <CapturePanel
      title="Camera modes"
      camera={props.camera}
      selection={selection()}
      onConnect={props.onConnect}
      onClose={props.onClose}
    >
      <ul class="cap-list">
        <For each={CAPTURE_STATES}>
          {(state) => (
            <li class="cap-row" aria-busy={running() === state.id ? 'true' : 'false'}>
              <div class="cap-line">
                <span class="cap-name">{state.name}</span>
                <Show when={count(state)}>
                  <span class="cap-count">{count(state)}</span>
                </Show>
                <Show when={count(state) && running() !== state.id}>
                  <label class="cap-photo">
                    Photo
                    <input
                      type="file"
                      accept="image/*"
                      capture="environment"
                      onChange={(event) => attach(state, event.currentTarget.files?.[0])}
                    />
                  </label>
                </Show>
                <button
                  class="wb-btn"
                  disabled={!available() || !label(state)}
                  onClick={() => void capture(state)}
                >
                  Capture
                </button>
              </div>
              <Show when={state.named}>
                <label class="wb-in">
                  {state.id === 'menu-page' ? 'Page' : 'Mode'}
                  <input
                    value={names()[state.id] ?? ''}
                    onInput={(event) =>
                      setNames((all) => ({ ...all, [state.id]: event.currentTarget.value }))
                    }
                  />
                </label>
              </Show>
              <Show when={running() === state.id}>
                <div class="cap-progress">
                  <div class="wb-progress">
                    <i style={{ width: `${(100 * progress().done) / progress().total}%` }} />
                  </div>
                  <span class="cap-dim">
                    <Show when={state.held}>Keep holding · </Show>
                    {Math.floor(progress().done / MIB)} / {Math.ceil(progress().total / MIB)} MiB
                  </span>
                </div>
              </Show>
            </li>
          )}
        </For>
      </ul>
      <Show when={failure()}>
        <p class="wb-status" role="alert">
          {failure()}
        </p>
      </Show>
    </CapturePanel>
  );
}
