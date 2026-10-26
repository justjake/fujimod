import { For, Show, createEffect, createMemo, createSignal } from 'solid-js';
import type { Camera, CameraLogEntry, SessionHolder } from './camera-model';
import { Direction, Layer, Level, exportLog, hex } from './log';

const layers = [Layer.Connection, Layer.Console, Layer.App80, Layer.Ff80, Layer.Usb];
const levels = [Level.Error, Level.Warning, Level.Info, Level.Debug, Level.Trace];

/** Distinct, quiet colors for camera names, in the order cameras appear. */
export const cameraColors = ['#a7d8b4', '#9ab8e8', '#e3c48f', '#d6a2d8', '#9fd6d6', '#e8a5a0'];

function holderText(holder: SessionHolder | undefined) {
  if (!holder) return 'Host';
  return holder.kind === 'local' ? `Local · ${holder.tool}` : `Remote · ${holder.controller}`;
}

function Row(props: { entry: CameraLogEntry; camera?: { name: string; color: string } }) {
  const [expanded, setExpanded] = createSignal(false);
  const bytes = () => props.entry.data?.length ?? 0;
  const sizes = () => props.entry.sizes;
  return (
    <div
      class={{
        'cl-entry': true,
        [`level-${props.entry.level.toLowerCase()}`]: true,
        'cl-connection': props.entry.layer === Layer.Connection,
        'cl-console': props.entry.layer === Layer.Console,
      }}
    >
      <button
        class="cl-row"
        aria-expanded={expanded() ? 'true' : 'false'}
        onClick={() => setExpanded((value) => !value)}
      >
        <time class="cl-time" datetime={new Date(props.entry.time).toISOString()}>
          {new Date(props.entry.time).toLocaleTimeString(undefined, { hour12: false })}
        </time>
        <Show when={props.camera}>
          {(camera) => (
            <span class="cl-camera" style={{ color: camera().color }}>
              {camera().name}
            </span>
          )}
        </Show>
        <span class="cl-layer">{props.entry.layer}</span>
        <span
          class="cl-direction"
          aria-label={
            props.entry.direction === Direction.Out
              ? 'To camera'
              : props.entry.direction === Direction.In
                ? 'From camera'
                : 'Host'
          }
        >
          {props.entry.direction}
        </span>
        <span class="cl-message">{props.entry.message.trimEnd()}</span>
        <span class="cl-size">
          {sizes()
            ? `${sizes()!.sent} / ${sizes()!.received} B`
            : bytes()
              ? `${bytes().toLocaleString()} B`
              : ''}
        </span>
      </button>
      <Show when={expanded()}>
        <div class="cl-detail">
          <div>
            {props.entry.level} · {holderText(props.entry.holder)} ·{' '}
            {new Date(props.entry.time).toISOString()}
          </div>
          <Show when={props.entry.detail}>
            <pre>{props.entry.detail}</pre>
          </Show>
          <Show when={bytes()}>
            <pre>{hex(props.entry.data!)}</pre>
          </Show>
        </div>
      </Show>
    </div>
  );
}

/** The single Dev Tools log. Names cameras only once more than one is known. */
export function ConsoleLog(props: {
  entries: readonly CameraLogEntry[];
  cameras: readonly Camera[];
  onClear: () => void;
}) {
  const [activeLayers, setActiveLayers] = createSignal<ReadonlySet<Layer>>(
    new Set(layers.filter((layer) => layer !== Layer.Usb)),
  );
  const [activeLevels, setActiveLevels] = createSignal<ReadonlySet<Level>>(new Set(levels));
  const [hiddenCameras, setHiddenCameras] = createSignal<ReadonlySet<string>>(new Set());
  const [follow, setFollow] = createSignal(true);
  const several = () => props.cameras.length > 1;
  const named = createMemo(
    () =>
      new Map(
        props.cameras.map((camera, index) => [
          camera.id,
          { name: camera.name, color: cameraColors[index % cameraColors.length] },
        ]),
      ),
  );
  const visible = createMemo(() =>
    props.entries.filter(
      (entry) =>
        activeLayers().has(entry.layer) &&
        activeLevels().has(entry.level) &&
        !(several() && hiddenCameras().has(entry.camera)),
    ),
  );
  function toggle<T>(set: ReadonlySet<T>, value: T) {
    const next = new Set(set);
    if (next.has(value)) next.delete(value);
    else next.add(value);
    return next;
  }
  let viewport!: HTMLDivElement;
  createEffect(
    () => [visible().length, follow()] as const,
    ([, following]) => {
      if (following && viewport) viewport.scrollTop = viewport.scrollHeight;
    },
  );
  function save() {
    const blob = new Blob(
      [exportLog(visible().map((entry) => ({ ...entry, correlation: entry.camera })))],
      {
        type: 'application/jsonl',
      },
    );
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = `fujimod-log-${new Date().toISOString().replaceAll(':', '-')}.jsonl`;
    anchor.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  return (
    <section class="cl" aria-label="Console">
      <div class="cl-filters">
        <div class="cl-group" role="group" aria-label="Log layers">
          <For each={layers}>
            {(layer) => (
              <button
                class="cl-chip"
                aria-pressed={activeLayers().has(layer) ? 'true' : 'false'}
                onClick={() => setActiveLayers((set) => toggle(set, layer))}
              >
                {layer}
              </button>
            )}
          </For>
        </div>
        <Show when={several()}>
          <span class="cl-sep" />
          <div class="cl-group" role="group" aria-label="Log cameras">
            <For each={props.cameras}>
              {(camera) => (
                <button
                  class="cl-chip"
                  aria-pressed={hiddenCameras().has(camera.id) ? 'false' : 'true'}
                  onClick={() => setHiddenCameras((set) => toggle(set, camera.id))}
                >
                  {camera.name}
                </button>
              )}
            </For>
          </div>
        </Show>
        <span class="cl-spacer" />
        <details class="cl-levels">
          <summary>
            Levels {activeLevels().size}/{levels.length}
          </summary>
          <div>
            <For each={levels}>
              {(level) => (
                <label>
                  <input
                    type="checkbox"
                    checked={activeLevels().has(level)}
                    onChange={() => setActiveLevels((set) => toggle(set, level))}
                  />
                  {level}
                </label>
              )}
            </For>
          </div>
        </details>
        <span class="cl-sep" />
        <button class="cl-text" onClick={save} disabled={!visible().length}>
          Save
        </button>
        <button class="cl-text" onClick={() => props.onClear()} disabled={!props.entries.length}>
          Clear
        </button>
      </div>
      <div
        class="cl-viewport"
        ref={(element) => {
          viewport = element;
        }}
        tabindex={0}
        role="region"
        aria-label="Connection log"
        onScroll={() => {
          if (viewport.scrollHeight - viewport.clientHeight - viewport.scrollTop > 48)
            setFollow(false);
        }}
      >
        <Show
          when={visible().length}
          fallback={
            <div class="cl-empty">
              {props.entries.length ? 'No matching entries' : 'No camera activity'}
            </div>
          }
        >
          <div class={{ 'cl-lines': true, 'cl-several': several() }}>
            <For each={visible()}>
              {(entry) => (
                <Row entry={entry} camera={several() ? named().get(entry.camera) : undefined} />
              )}
            </For>
          </div>
        </Show>
      </div>
      <div class="cl-status">
        <span>
          {visible().length.toLocaleString()} / {props.entries.length.toLocaleString()} entries
        </span>
        <label>
          <input
            type="checkbox"
            checked={follow()}
            onChange={(event) => setFollow(event.currentTarget.checked)}
          />
          Follow
        </label>
      </div>
    </section>
  );
}
