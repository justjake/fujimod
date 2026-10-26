import { For, Show, createSignal } from 'solid-js';
import type { ControlInput } from 'fujimod/emulator';
import type { ControlSession } from './controls';
import { compatibleVariants } from './default-layout';
import { downloadBlob } from './download';
import { Icon } from './icons';
import type { ControlNode, NodePath } from './layout';
import type { LayoutEditor } from './layout-editor';

const STYLE_NAMES: Record<NonNullable<ControlNode['variant']>, string> = {
  dial: 'Dial',
  ticks: 'Ring',
  segment: 'Switch',
  shutter: 'Shutter',
  key: 'Button',
  record: 'Record button',
};

const SLOTS: Record<'dpad' | 'command', [string, string][]> = {
  dpad: [
    ['up', 'Up'],
    ['down', 'Down'],
    ['left', 'Left'],
    ['right', 'Right'],
    ['center', 'Center'],
  ],
  command: [
    ['left', 'Left'],
    ['center', 'Press'],
    ['right', 'Right'],
  ],
};

function InputPicker(props: {
  label: string;
  value?: string;
  inputs: readonly ControlInput[];
  onChange: (id: string) => void;
}) {
  return (
    <label class="sheet-cell">
      <span>{props.label}</span>
      <select
        aria-label={props.label}
        value={props.value ?? ''}
        onChange={(event) => props.onChange(event.currentTarget.value)}
      >
        <option value="">None</option>
        <For each={props.inputs.filter((input) => input.supported || input.id === props.value)}>
          {(input) => <option value={input.id}>{input.label}</option>}
        </For>
      </select>
    </label>
  );
}

/** The sheet for the selected control: name, style and the inputs it drives. */
function ControlSheet(props: { session: ControlSession; editor: LayoutEditor }) {
  const [drag, setDrag] = createSignal(0);
  const inputs = () => [...props.session.read().inputs.values()];
  const path = (): NodePath => props.editor.selected() ?? [];
  let start: number | undefined;
  return (
    <Show when={props.editor.selectedNode()}>
      {(node) => (
        <div
          class="control-sheet"
          role="dialog"
          aria-label="Control"
          style={{ transform: `translateY(${drag()}px)` }}
        >
          <div
            class="sheet-grabber"
            onPointerDown={(event) => {
              start = event.clientY;
              event.currentTarget.setPointerCapture(event.pointerId);
            }}
            onPointerMove={(event) => {
              if (start !== undefined) setDrag(Math.max(0, event.clientY - start));
            }}
            onPointerUp={() => {
              if (drag() > 80) props.editor.select(undefined);
              start = undefined;
              setDrag(0);
            }}
          >
            <i />
          </div>
          <div class="sheet-group">
            <label class="sheet-cell">
              <span>Name</span>
              <input
                aria-label="Name"
                value={node().caption ?? node().face?.join('/') ?? ''}
                onChange={(event) =>
                  props.editor.update(path(), { caption: event.currentTarget.value || undefined })
                }
              />
            </label>
            <Show when={node().kind === 'control' && node().id}>
              {(id) => {
                const input = () => props.session.read().inputs.get(id());
                return (
                  <Show when={input()}>
                    {(found) => (
                      <label class="sheet-cell">
                        <span>Style</span>
                        <select
                          aria-label="Style"
                          value={node().variant ?? ''}
                          onChange={(event) =>
                            props.editor.update(path(), {
                              variant: event.currentTarget.value as ControlNode['variant'],
                            })
                          }
                        >
                          <For each={compatibleVariants(found())}>
                            {(variant) => <option value={variant}>{STYLE_NAMES[variant]}</option>}
                          </For>
                        </select>
                      </label>
                    )}
                  </Show>
                );
              }}
            </Show>
          </div>
          <div class="sheet-title">Inputs</div>
          <div class="sheet-group">
            <Show
              when={node().kind === 'dpad' || node().kind === 'command' || node().kind === 'rocker'}
              fallback={
                <InputPicker
                  label="Input"
                  value={node().id}
                  inputs={inputs()}
                  onChange={(id) => props.editor.update(path(), { id: id || undefined })}
                />
              }
            >
              <For each={SLOTS[node().kind as 'dpad' | 'command']}>
                {([slot, label]) => (
                  <InputPicker
                    label={label}
                    value={node().bindings?.[slot]}
                    inputs={inputs()}
                    onChange={(id) => props.editor.bind(path(), slot, id)}
                  />
                )}
              </For>
            </Show>
          </div>
        </div>
      )}
    </Show>
  );
}

/** Floating toolbar and control sheet shown while a layout is edited. */
export function LayoutInspector(props: { session?: ControlSession; editor: LayoutEditor }) {
  const [unknown, setUnknown] = createSignal<string[]>([]);
  const [error, setError] = createSignal('');
  let picker: HTMLInputElement | undefined;
  return (
    <Show when={props.editor.editing() && props.session}>
      {(session) => (
        <>
          <Show when={unknown().length || error()}>
            <div class="layout-notice" role="status">
              {error() || `Not on this camera: ${unknown().join(', ')}`}
            </div>
          </Show>
          <div class="layout-toolbar">
            <div class="glass pill">
              <button type="button" aria-label="Import layout" onClick={() => picker?.click()}>
                <Icon name="download" size={21} />
              </button>
              <button
                type="button"
                aria-label="Export layout"
                onClick={() => {
                  const json = props.editor.exportJson();
                  const body = session().read().data?.body ?? 'camera';
                  if (json)
                    downloadBlob(
                      new Blob([json], { type: 'application/json' }),
                      `${body}.layout.json`,
                    );
                }}
              >
                <Icon name="share" size={21} />
              </button>
            </div>
            <button
              type="button"
              class="confirm"
              aria-label="Done"
              onClick={() => {
                setUnknown([]);
                setError('');
                props.editor.end();
              }}
            >
              <Icon name="check" size={22} />
            </button>
            <input
              ref={picker}
              class="visually-hidden"
              type="file"
              accept=".json,application/json"
              aria-label="Layout file"
              onChange={async (event) => {
                const file = event.currentTarget.files?.[0];
                event.currentTarget.value = '';
                if (!file) return;
                try {
                  setUnknown(props.editor.importJson(await file.text()));
                  setError('');
                } catch (failure) {
                  setError(failure instanceof Error ? failure.message : String(failure));
                }
              }}
            />
          </div>
          <ControlSheet session={session()} editor={props.editor} />
        </>
      )}
    </Show>
  );
}
