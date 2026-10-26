import type { PhysicalControls } from 'fujimod';
import x70 from '../../../../catalog/bodies/x70/controls.json';
import { render } from '@solidjs/web';
import { flush } from 'solid-js';
import { beforeAll, afterEach, expect, test, vi } from 'vite-plus/test';
import { ControlSession, Controls } from './controls';
import { LayoutEditor } from './layout-editor';
import { LayoutInspector } from './LayoutInspector';
import type { Engine } from '../engine/api';
import type { ControlInput } from 'fujimod/emulator';
import type { DevicePanel } from './layout';
let dispose: (() => void) | undefined;
beforeAll(() => {
  globalThis.ResizeObserver = class {
    observe() {}
    disconnect() {}
    unobserve() {}
  };
});
afterEach(() => {
  dispose?.();
  document.body.replaceChildren();
});
function mount(
  inputs: ControlInput[],
  data: DevicePanel,
  onLayoutChange?: (panel: DevicePanel) => void | Promise<void>,
  physicalControls?: PhysicalControls,
) {
  const host = document.createElement('div');
  document.body.append(host);
  const engine = {
    rotate: vi.fn(async () => {}),
    setPosition: vi.fn(async () => {}),
    power: vi.fn(async () => {}),
    press: vi.fn(async () => {}),
    release: vi.fn(async () => {}),
  };
  const viewer = {
    pressFeedback: vi.fn(),
  };
  const combo = vi.fn(async () => {});
  const session = new ControlSession(
    viewer,
    (error) => {
      throw error;
    },
    combo,
  );
  session.physicalControls = physicalControls;
  session.render(inputs, engine as unknown as Engine, data);
  session.ready = true;
  const editor = new LayoutEditor({ runtime: { panel: session }, onLayoutChange });
  dispose = render(
    () => (
      <>
        <Controls session={session} editor={editor} />
        <LayoutInspector session={session} editor={editor} />
      </>
    ),
    host,
  );
  flush();
  return { host, engine, viewer, session, editor, combo };
}
test('every segment selects through the engine and remains synchronized after power off and rollback', async () => {
  const { host, engine, session } = mount(
    [
      {
        id: 'power',
        label: 'Power',
        kind: 'switch',
        supported: true,
        positions: [
          { id: 'OFF', label: 'OFF' },
          { id: 'ON', label: 'ON' },
        ],
        initial: 'ON',
      },
      {
        id: 'focus_mode',
        label: 'Focus',
        kind: 'switch',
        supported: true,
        positions: [
          { id: 'manual', label: 'M' },
          { id: 'single', label: 'S' },
          { id: 'continuous', label: 'C' },
        ],
      },
      { id: 'q', label: 'Q', kind: 'button', supported: true },
    ],
    {
      schema: 1,
      body: 'x70',
      layout: [
        { kind: 'control', id: 'power', operation: 'power', variant: 'segment' },
        { kind: 'control', id: 'focus_mode', variant: 'segment' },
        { kind: 'control', id: 'q', variant: 'key' },
      ],
      combos: [],
    },
  );
  for (const value of ['single', 'continuous', 'manual']) {
    (
      host.querySelector(
        `[data-id="focus_mode"] button[data-value="${value}"]`,
      ) as HTMLButtonElement
    ).click();
    flush();
    expect(engine.setPosition).toHaveBeenLastCalledWith('focus_mode', value);
    expect(host.querySelector(`[data-value="${value}"]`)?.getAttribute('aria-pressed')).toBe(
      'true',
    );
  }
  (host.querySelector('[data-id="power"] [data-value="OFF"]') as HTMLButtonElement).click();
  flush();
  expect((host.querySelector('button[data-id="q"]') as HTMLButtonElement).disabled).toBe(true);
  expect((host.querySelector('[data-id="focus_mode"] button') as HTMLButtonElement).disabled).toBe(
    true,
  );
  (host.querySelector('[data-id="power"] [data-value="ON"]') as HTMLButtonElement).click();
  await vi.waitFor(() => expect(session.ready).toBe(true));
  flush();
  expect(engine.power).toHaveBeenLastCalledWith(true);
  expect((host.querySelector('button[data-id="q"]') as HTMLButtonElement).disabled).toBe(false);
  session.disable();
  flush();
  expect((host.querySelector('button[data-id="q"]') as HTMLButtonElement).disabled).toBe(true);
  session.resume();
  session.synchronize({ focus_mode: 'single' });
  flush();
  expect(host.querySelector('[data-value="single"]')?.getAttribute('aria-pressed')).toBe('true');
});
test('keyboard dials stop at their physical limits and relative dials preserve detent counts', () => {
  const { host, engine, session } = mount(
    [
      {
        id: 'aperture',
        label: 'Aperture',
        kind: 'ring',
        supported: true,
        positions: [
          { id: '2.8', label: '2.8' },
          { id: '4', label: '4' },
        ],
      },
      { id: 'control_ring', label: 'Command', kind: 'dial', supported: true, relative: true },
    ],
    {
      schema: 1,
      body: 'x70',
      layout: [
        { kind: 'control', id: 'aperture', variant: 'dial' },
        { kind: 'control', id: 'control_ring', variant: 'ticks' },
      ],
      combos: [],
    },
  );
  const aperture = host.querySelector('[role="slider"][data-id="aperture"]')!;
  aperture.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
  flush();
  expect(engine.setPosition).toHaveBeenLastCalledWith('aperture', '4');
  expect(aperture.getAttribute('aria-valuetext')).toBe('4');
  const calls = engine.setPosition.mock.calls.length;
  aperture.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
  flush();
  expect(engine.setPosition.mock.calls.length).toBe(calls);
  const command = host.querySelector('[role="slider"][data-id="control_ring"]')!;
  for (const key of ['ArrowRight', 'ArrowRight', 'ArrowLeft'])
    command.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }));
  flush();
  expect(engine.rotate.mock.calls).toEqual([
    ['control_ring', 1],
    ['control_ring', 1],
    ['control_ring', -1],
  ]);
  expect(session.relativeSteps.get('control_ring')).toBe(1);
  expect((command.querySelector('.vals') as HTMLElement).style.backgroundPositionX).toBe('16px');
});
test('a mounted empty panel reacts to the guest input inventory arriving later', () => {
  const host = document.createElement('div');
  document.body.append(host);
  const viewer = { pressFeedback: vi.fn() };
  const session = new ControlSession(
    viewer,
    (error) => {
      throw error;
    },
    async () => {},
  );
  dispose = render(() => <Controls session={session} />, host);
  flush();
  expect(host.querySelectorAll('button').length).toBe(0);
  session.render([{ id: 'q', label: 'Q', kind: 'button', supported: true }], {} as Engine, {
    schema: 1,
    body: 'x70',
    layout: [{ kind: 'control', id: 'q', variant: 'key' }],
    combos: [],
  });
  flush();
  expect(host.querySelectorAll('button[data-id="q"]').length).toBe(1);
});

function change(host: HTMLElement, label: string, value: string, type = 'change') {
  const field = host.querySelector<HTMLInputElement | HTMLSelectElement>(
    `[aria-label="${label}"]`,
  )!;
  field.value = value;
  field.dispatchEvent(new Event(type, { bubbles: true }));
  flush();
}

const qAndFocus: ControlInput[] = [
  { id: 'q', label: 'Q (quick menu) button', kind: 'button', supported: true },
  {
    id: 'focus',
    label: 'Focus mode selector',
    kind: 'switch',
    supported: true,
    positions: [
      { id: 'manual', label: 'M' },
      { id: 'single', label: 'S' },
    ],
  },
];

test('Edit Controls selects a control and its sheet edits name, style and input without sending input', () => {
  const onLayoutChange = vi.fn();
  const { host, engine, session } = mount(
    qAndFocus,
    { schema: 1, body: 'x70', layout: [{ kind: 'control', id: 'q', variant: 'key' }], combos: [] },
    onLayoutChange,
  );
  host.querySelector<HTMLButtonElement>('.edit-controls')!.click();
  flush();
  expect(host.querySelector('.edit-controls')).toBeNull();
  host.querySelector<HTMLButtonElement>('[data-layout-path="0"] [data-id="q"]')!.click();
  flush();
  expect(host.querySelector('[data-layout-path="0"]')?.getAttribute('data-layout-selected')).toBe(
    'true',
  );
  expect(host.querySelector<HTMLSelectElement>('[aria-label="Input"]')?.value).toBe('q');

  change(host, 'Name', 'Focus mode');
  change(host, 'Input', 'focus');
  change(host, 'Style', 'segment');

  expect(session.data?.layout[0]).toMatchObject({
    kind: 'control',
    id: 'focus',
    variant: 'segment',
    caption: 'Focus mode',
  });
  expect(host.querySelector('[data-id="focus"].control-node .seg button')).not.toBeNull();
  expect(engine.press).not.toHaveBeenCalled();
  expect(onLayoutChange).toHaveBeenLastCalledWith(session.data);
});

test('the minus badge removes a control and Done leaves editing', () => {
  const onLayoutChange = vi.fn();
  const { host, session, editor } = mount(
    qAndFocus,
    {
      schema: 1,
      body: 'x70',
      layout: [
        { kind: 'control', id: 'q', variant: 'key' },
        { kind: 'control', id: 'focus', variant: 'segment' },
      ],
      combos: [],
    },
    onLayoutChange,
  );
  editor.begin();
  flush();
  host.querySelector<HTMLButtonElement>('[aria-label="Remove Q (quick menu) button"]')!.click();
  flush();
  expect(session.data?.layout.map((node) => node.id)).toEqual(['focus']);
  expect(onLayoutChange).toHaveBeenLastCalledWith(session.data);
  host.querySelector<HTMLButtonElement>('[aria-label="Done"]')!.click();
  flush();
  expect(editor.editing()).toBe(false);
  expect(host.querySelector('.layout-remove')).toBeNull();
});

test('a D-pad sheet binds each direction to an input', () => {
  const { host, session, editor, engine } = mount(
    [
      ...['up', 'down', 'left', 'right'].map((direction): ControlInput => ({
        id: `selector.${direction}`,
        label: `Selector / ${direction}`,
        kind: 'button',
        supported: true,
      })),
      { id: 'menu_ok', label: 'MENU/OK button', kind: 'button', supported: true },
    ],
    {
      schema: 1,
      body: 'x70',
      layout: [
        {
          kind: 'dpad',
          face: ['MENU', 'OK'],
          bindings: {
            up: 'selector.up',
            down: 'selector.down',
            left: 'selector.left',
            right: 'selector.right',
          },
        },
      ],
      combos: [],
    },
  );
  editor.begin();
  flush();
  host.querySelector<HTMLElement>('.dpad .a.u')!.click();
  flush();
  expect(editor.selected()).toEqual([0]);
  change(host, 'Center', 'menu_ok');
  expect(session.data?.layout[0]?.bindings?.center).toBe('menu_ok');
  expect(engine.press).not.toHaveBeenCalled();
});

test('importing a layout lists ids this camera does not have', async () => {
  const onLayoutChange = vi.fn();
  const { host, session, editor } = mount(
    qAndFocus,
    { schema: 1, body: 'x70', layout: [{ kind: 'control', id: 'q', variant: 'key' }], combos: [] },
    onLayoutChange,
  );
  editor.begin();
  flush();
  const imported: DevicePanel = {
    schema: 1,
    body: 'x70',
    layout: [{ kind: 'control', id: 'missing-input', variant: 'key', caption: 'Missing' }],
    combos: [],
  };
  const file = new File([JSON.stringify(imported)], 'layout.json', { type: 'application/json' });
  Object.defineProperty(file, 'text', { value: async () => JSON.stringify(imported) });
  const picker = host.querySelector<HTMLInputElement>('[aria-label="Layout file"]')!;
  Object.defineProperty(picker, 'files', { configurable: true, value: [file] });
  picker.dispatchEvent(new Event('change', { bubbles: true }));
  await vi.waitFor(() => {
    flush();
    expect(host.querySelector('.layout-notice')?.textContent).toContain('missing-input');
  });
  expect(host.querySelector('[data-id="missing-input"] .cap')?.textContent).toBe('Missing');
  expect(session.data).toEqual(imported);
  expect(onLayoutChange).toHaveBeenLastCalledWith(imported);
});

test('the live panel uses catalog markings and runs combos through the session', () => {
  const item = {
    label: 'SD card boot',
    modes: ['sd'] as import('fujimod').BootMode[],
    inputs: [{ id: 'command_control.press' }, { id: 'movie_record_button' }],
  };
  const { host, combo, session } = mount(
    [{ id: 'playback_button', label: 'Unrelated manual name', kind: 'button', supported: true }],
    {
      schema: 1,
      body: 'x70',
      layout: [{ kind: 'control', id: 'playback_button', variant: 'key', face: ['OTHER'] }],
      combos: [item],
    },
    undefined,
    x70 as PhysicalControls,
  );
  expect(host.querySelector('button[data-id="playback_button"] .key svg')).not.toBeNull();
  expect(host.textContent).not.toContain('OTHER');
  expect(host.querySelector('.combo .rocker .tab.glow')).not.toBeNull();
  expect(host.querySelector('.combo .control-short-name')?.textContent).toBe('COMMAND');
  const button = host.querySelector<HTMLButtonElement>('.combo')!;
  button.dispatchEvent(new Event('pointerenter'));
  expect(session.active).toEqual(item.inputs.map((input) => input.id));
  button.click();
  expect(combo).toHaveBeenCalledWith(item);
});

test('rocker halves and tab send separate native contact presses and releases', async () => {
  const ids = ['command_control.decrease', 'command_control.press', 'command_control.increase'];
  const { host, engine, session } = mount(
    ids.map((id) => ({ id, label: id, kind: 'button', supported: true })),
    {
      schema: 1,
      body: 'x70',
      combos: [],
      layout: [{ kind: 'rocker', bindings: { left: ids[0]!, center: ids[1]!, right: ids[2]! } }],
    },
    undefined,
    x70 as PhysicalControls,
  );
  const stop = session.listen();
  session.powered = true;
  flush();
  expect(host.querySelector('[data-part="rocker"] .knurl')).toBeTruthy();
  for (const [index, id] of ids.entries()) {
    const button = host.querySelector<HTMLButtonElement>(`button[data-id="${id}"]`)!;
    button.setPointerCapture = vi.fn();
    button.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true, button: 0 }));
    expect(engine.press).toHaveBeenLastCalledWith(id, undefined);
    button.dispatchEvent(new MouseEvent('pointerup', { bubbles: true, button: 0 }));
    expect(engine.release).toHaveBeenLastCalledWith(id);
    session.feedback([id], true);
    flush();
    expect(host.querySelectorAll('.rocker .glow')).toHaveLength(1);
    expect(host.querySelector('.rocker .glow')?.getAttribute('data-id')).toBe(ids[index]);
  }
  expect(engine.rotate).not.toHaveBeenCalled();
  stop();
});
