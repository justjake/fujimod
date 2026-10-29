import { For, Show, createMemo, createSignal } from 'solid-js';
import { Icon } from '@fujimod/emulator-ui/ui/icons';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';
import type { Camera, CameraModel } from '../camera-model';
import { bulkSelection, canceled, captureRanges, contiguous, type CaptureSet } from './capture';
import { controlDiff, type ControlDiff } from './mapping';
import { CapturePanel, emptySet } from './CapturePanel';
import { slug, type AddFile } from '../../files/shelf';
import './captures.css';

const STEPS = ['released', 'held', 'released', 'held'] as const;
const hex = (value: number) => `0x${value.toString(16).padStart(8, '0')}`;

/** Find the RAM words that follow each control the person holds. */
export function MapControls(props: {
  camera?: Camera;
  model: CameraModel;
  set?: CaptureSet;
  onSet: (set: CaptureSet) => void;
  onFile: AddFile;
  onConnect: () => void;
  onClose: () => void;
}) {
  const selection = createMemo(() => bulkSelection(props.camera?.identity));
  const scope = () =>
    selection()?.bulk?.scopes.find((entry) => entry.name === 'quick') ??
    selection()?.bulk?.scopes[0];
  const controls = () =>
    Object.entries(selection()?.physical_controls?.controls ?? {}).filter(
      ([, control]) => !control.mechanical,
    );
  const [control, setControl] = createSignal<string>();
  const [custom, setCustom] = createSignal('');
  const [note, setNote] = createSignal('');
  const [captures, setCaptures] = createSignal<Uint8Array<ArrayBuffer>[]>([]);
  const [running, setRunning] = createSignal(false);
  const [progress, setProgress] = createSignal({ done: 0, total: 1 });
  const [failure, setFailure] = createSignal('');
  let abort: AbortController | undefined;
  const [result, setResult] = createSignal<{ name: string; base: number; diff: ControlDiff }>();
  const set = () => props.set ?? emptySet(selection()!.firmware.firmware_id);
  const name = () => {
    const id = control();
    const chosen = controls().find(([key]) => key === id)?.[1].name;
    return chosen ?? custom().trim();
  };
  const step = () => STEPS[captures().length];
  const mapped = (id: string) => set().mappings.some((mapping) => mapping.control === id);
  const available = () =>
    Boolean(
      props.camera?.present && !props.camera.session && !props.camera.uncertain && !running(),
    );

  async function capture() {
    const camera = props.camera;
    const current = scope();
    const firmwareId = selection()?.firmware.firmware_id;
    if (!camera || !current || !firmwareId || !name() || !available()) return;
    setRunning(true);
    setFailure('');
    abort = new AbortController();
    const signal = abort.signal;
    try {
      const read = await props.model.withSession(camera.id, 'Map controls', (session) =>
        captureRanges(
          session,
          firmwareId,
          current,
          (done, total) => setProgress({ done, total }),
          signal,
        ),
      );
      const taken = [...captures(), contiguous(read)];
      if (taken.length < STEPS.length) {
        setCaptures(taken);
        return;
      }
      const [range] = current.ranges;
      const diff = controlDiff(range.address, [taken[0], taken[1], taken[2], taken[3]]);
      const mapping = {
        control: control(),
        name: name(),
        note: note().trim(),
        time: new Date().toISOString(),
        scope: current.name,
        address: range.address,
        size: range.size,
        diff,
      };
      const facts = {
        firmware: firmwareId,
        control: mapping.control ?? null,
        name: mapping.name,
        note: mapping.note,
        time: mapping.time,
        scope: mapping.scope,
        address: mapping.address,
        size: mapping.size,
      };
      const base = `mapping-${slug(mapping.name)}`;
      props.onFile(
        `${base}-candidates.bin`,
        new Blob([new Uint8Array(diff.candidates.buffer as ArrayBuffer)]),
        {
          ...facts,
          format: 'u32le address',
          count: diff.candidates.length,
        },
      );
      props.onFile(
        `${base}-changed.bin`,
        new Blob([new Uint8Array(diff.changed.buffer as ArrayBuffer)]),
        {
          ...facts,
          format: 'u32le address, released, held, released, held',
          count: diff.changed.length / 5,
          omitted: diff.omitted,
        },
      );
      const previous = set();
      props.onSet({ ...previous, mappings: [...previous.mappings, mapping] });
      setResult({ name: name(), base: range.address, diff });
      setCaptures([]);
      setControl(undefined);
      setCustom('');
      setNote('');
    } catch (error) {
      if (!canceled(error)) setFailure(shownError(error));
    } finally {
      setRunning(false);
    }
  }
  function restart() {
    setCaptures([]);
    setFailure('');
  }
  const candidates = () => {
    const last = result();
    if (!last) return [];
    const values = new Map<number, [number, number]>();
    const changed = last.diff.changed;
    for (let index = 0; index < changed.length; index += 5)
      values.set(changed[index], [changed[index + 1], changed[index + 2]]);
    return Array.from(last.diff.candidates.slice(0, 8), (address) => ({
      address,
      values: values.get(address),
    }));
  };

  return (
    <CapturePanel
      title="Map controls"
      camera={props.camera}
      selection={selection()}
      onConnect={props.onConnect}
      onClose={props.onClose}
    >
      <Show when={result()}>
        {(last) => (
          <div class="wb-tool cap-result" role="status">
            <div class="wb-tool-head">
              {last().name}
              <span class="cap-dim">{last().diff.candidates.length} candidates</span>
            </div>
            <ul class="cap-words">
              <For each={candidates()}>
                {(word) => (
                  <li>
                    <code>{hex(word.address)}</code>
                    <Show when={word.values}>
                      {(values) => (
                        <code class="cap-dim">
                          {hex(values()[0])} → {hex(values()[1])}
                        </code>
                      )}
                    </Show>
                  </li>
                )}
              </For>
            </ul>
          </div>
        )}
      </Show>
      <ul class="cap-controls" aria-label="Controls">
        <For each={controls()}>
          {([id, entry]) => (
            <li>
              <button
                class="cap-control"
                aria-pressed={control() === id ? 'true' : 'false'}
                disabled={running() || captures().length > 0}
                onClick={() => setControl(control() === id ? undefined : id)}
              >
                <span>{entry.name}</span>
                <span class="cap-dim">
                  {mapped(id) ? <Icon name="check" size={13} /> : entry.location}
                </span>
              </button>
            </li>
          )}
        </For>
      </ul>
      <Show when={!control()}>
        <label class="wb-in">
          Control
          <input
            value={custom()}
            disabled={captures().length > 0}
            onInput={(event) => setCustom(event.currentTarget.value)}
          />
        </label>
      </Show>
      <label class="wb-in">
        Note
        <input value={note()} onInput={(event) => setNote(event.currentTarget.value)} />
      </label>
      <ol class="cap-steps" aria-label="Captures">
        <For each={STEPS}>
          {(entry, index) => (
            <li
              class={{
                'cap-step': true,
                'cap-done': index() < captures().length,
                'cap-next': index() === captures().length,
              }}
            >
              {entry === 'held' ? 'Held' : 'Released'}
            </li>
          )}
        </For>
      </ol>
      <Show when={running()}>
        <div class="cap-progress">
          <div class="wb-progress">
            <i style={{ width: `${(100 * progress().done) / progress().total}%` }} />
          </div>
          <Show when={step() === 'held'}>
            <span class="cap-dim">Keep holding</span>
          </Show>
          <button class="wb-btn" onClick={() => abort?.abort()}>
            Cancel
          </button>
        </div>
      </Show>
      <div class="wb-pair">
        <button
          class="wb-btn wb-primary"
          disabled={!available() || !name()}
          onClick={() => void capture()}
        >
          {step() === 'held' ? 'Capture held' : 'Capture released'}
        </button>
        <button class="wb-btn" disabled={running() || !captures().length} onClick={restart}>
          Restart
        </button>
      </div>
      <Show when={failure()}>
        <p class="wb-status" role="alert">
          {failure()}
        </p>
      </Show>
    </CapturePanel>
  );
}
