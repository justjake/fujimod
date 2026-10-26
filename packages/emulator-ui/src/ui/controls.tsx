import { For, Show, Switch, Match, createSignal, createEffect, onSettled, untrack } from 'solid-js';
import type { Engine } from '../engine/api';
import type { ControlInput } from 'fujimod/emulator';
import type { LCDViewer } from '../viewer/lcd';
import type { ControlNode, DevicePanel, NodePath, PowerCombo } from './layout';
import type { LayoutEditor } from './layout-editor';
import type { PhysicalControls } from 'fujimod';
import type { JSX } from '@solidjs/web';
import { KeyFace, ShutterFace, ControlFace, PositionsFace } from './control-faces';
import { marking, type Marking } from './control-part';
import { PowerComboKeys } from './power-combo';
import type { DetentSound } from '../feedback/detent-sound';
import { DetentFeedback } from '../feedback/detent';

/** Input protocol and gesture lifetime; Solid owns the panel's rendered state. */
export class ControlSession {
  private revision;
  changed;
  read() {
    this.revision();
    return this;
  }
  data?: DevicePanel;
  physicalControls?: PhysicalControls;
  detent = new DetentFeedback();
  detentSounds: Record<string, DetentSound> = {};
  inputs = new Map<string, ControlInput>();
  relativeSteps = new Map<string, number>();
  positions = new Map<string, string>();
  renders = new Map<string, () => void>();
  holds = new Map<number | string, string>();
  engine?: Engine;
  ready = false;
  powered = true;
  powerID?: string;
  active: string[] = [];
  glowing = false;
  stopShutter?: (release?: boolean) => void;
  stopRotation?: () => void;
  rotation?: { dial: HTMLElement; fraction: number; center?: number };
  private pulseQueue: Promise<unknown> = Promise.resolve();
  constructor(
    readonly viewer: Pick<LCDViewer, 'pressFeedback'>,
    readonly error: (error: unknown) => void,
    readonly combo: (combo: PowerCombo) => Promise<void>,
  ) {
    const [revision, setRevision] = createSignal(0);
    this.revision = revision;
    this.changed = () => setRevision((n) => n + 1);
  }
  listen() {
    const release = (event: PointerEvent) => this.release(event.pointerId);
    const blur = () => this.releaseAll();
    const visibility = () => {
      if (document.hidden) this.releaseAll();
    };
    window.addEventListener('pointerup', release);
    window.addEventListener('pointercancel', release);
    window.addEventListener('blur', blur);
    document.addEventListener('visibilitychange', visibility);
    return () => {
      this.clear();
      window.removeEventListener('pointerup', release);
      window.removeEventListener('pointercancel', release);
      window.removeEventListener('blur', blur);
      document.removeEventListener('visibilitychange', visibility);
    };
  }
  render(
    inputs: ControlInput[],
    engine: Engine,
    data: DevicePanel,
    initial: Record<string, string> = {},
  ) {
    this.clear();
    this.engine = engine;
    this.detentSounds = data.detentSounds ?? {};
    this.ready = true;
    this.inputs = new Map(inputs.map((input) => [input.id, input]));
    for (const input of inputs)
      if (input.positions?.length)
        this.positions.set(input.id, initial[input.id] ?? input.initial ?? input.positions[0].id);
    const visit = (nodes: ControlNode[]) => {
      for (const node of nodes) {
        if (node.operation === 'power') {
          this.powerID = node.id;
          if (node.id) this.positions.set(node.id, this.powered ? 'ON' : 'OFF');
        }
        if (node.variant === 'segment' && node.id && !this.positions.has(node.id)) {
          const first =
            this.inputs.get(node.id)?.positions?.[0]?.id ?? Object.keys(node.labels ?? {})[0];
          if (first !== undefined) this.positions.set(node.id, first);
        }
        visit(node.children ?? []);
      }
    };
    visit(data.layout);
    this.data = data;
    this.changed();
  }
  index(id: string) {
    return Math.max(
      0,
      this.inputs.get(id)?.positions?.findIndex((p) => p.id === this.positions.get(id)) ?? 0,
    );
  }
  disabled(id: string) {
    this.read();
    return (
      !this.ready || (id !== this.powerID && (!this.inputs.get(id)?.supported || !this.powered))
    );
  }
  classes(id: string) {
    this.read();
    return {
      active: this.active.includes(id) && !this.glowing,
      glow: this.active.includes(id) && this.glowing,
    };
  }
  shutterDown(
    id: string,
    tray: HTMLElement,
    state: (stage: string, left: string) => void,
    event: PointerEvent,
  ) {
    if (!this.ready || !this.powered) return;
    event.preventDefault();
    tray.setPointerCapture(event.pointerId);
    const started = performance.now(),
      bounds = tray.getBoundingClientRect(),
      origin = event.clientX,
      engine = this.engine!;
    let full = false,
      moved = false;
    const stage = (value: 'half' | 'full') => {
      full = value === 'full';
      state(value, '');
      void engine.press(id, value).catch((error) => {
        if (this.engine === engine && this.ready) this.error(error);
      });
    };
    stage('half');
    const move = (next: PointerEvent) => {
      if (next.pointerId !== event.pointerId) return;
      const delta = next.clientX - origin;
      moved ||= Math.abs(delta) > 5;
      const travel = Math.max(1, bounds.width - 70);
      if (delta >= travel && !full) stage('full');
      else if (delta < travel * 0.8 && full) stage('half');
      state(full ? 'full' : 'half', full ? '' : `${3 + Math.max(0, Math.min(travel, delta))}px`);
    };
    const cleanup = (release = false) => {
      if (release) {
        const engine = this.engine;
        void engine?.release(id).catch((error) => {
          if (this.engine === engine && this.ready) this.error(error);
        });
      }
      state('idle', '');
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', finish);
      window.removeEventListener('pointercancel', finish);
      this.stopShutter = undefined;
    };
    const finish = (up: PointerEvent) => {
      if (up.pointerId !== event.pointerId) return;
      if (up.type !== 'pointercancel' && !moved && !full && performance.now() - started < 250)
        stage('full');
      const engine = this.engine!;
      void engine.release(id).catch((error) => {
        if (this.engine === engine && this.ready) this.error(error);
      });
      cleanup();
    };
    this.stopShutter = cleanup;
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', finish);
    window.addEventListener('pointercancel', finish);
  }
  shutterKey(id: string, event: KeyboardEvent) {
    if ((event.key === ' ' || event.key === 'Enter') && !event.repeat) {
      event.preventDefault();
      if (this.ready && this.powered) {
        const engine = this.engine!;
        void engine
          .press(id, 'half')
          .then(() => engine.press(id, 'full'))
          .then(() => engine.release(id))
          .catch((error) => {
            if (this.engine === engine && this.ready) this.error(error);
          });
      }
    }
  }
  private clampFraction(id: string, fraction: number) {
    const positions = this.inputs.get(id)?.positions;
    if (!positions?.length) return fraction;
    const index = positions.findIndex((p) => p.id === this.positions.get(id));
    return Math.max(-index, Math.min(positions.length - 1 - index, fraction));
  }
  position(id: string, value: string) {
    if (
      !this.ready ||
      (!this.powered && id !== this.powerID) ||
      (id !== this.powerID && !this.inputs.get(id)?.supported) ||
      this.positions.get(id) === value
    )
      return;
    this.positions.set(id, value);
    this.changed();
    this.renders.get(id)?.();
    this.feedback([id]);
    const engine = this.engine!;
    if (id === this.powerID) {
      this.powered = value === 'ON';
      if (this.powered) this.ready = false;
      this.availability();
    }
    void (
      id === this.powerID
        ? engine.setPosition(id, value).then(() => engine.power(value === 'ON'))
        : engine.setPosition(id, value)
    )
      .then(() => {
        if (this.engine === engine && id === this.powerID && value === 'ON') {
          this.ready = true;
          this.availability();
        }
      })
      .catch((error) => {
        if (this.engine === engine) this.error(error);
      });
  }
  step(id: string, direction: number): boolean {
    const input = this.inputs.get(id);
    if (!this.ready || !this.powered || !input?.supported) return false;
    if (input.relative) {
      this.relativeSteps.set(id, (this.relativeSteps.get(id) ?? 0) + Math.sign(direction));
      this.changed();
      void this.engine!.rotate(id, Math.sign(direction)).catch(this.error);
      return true;
    }
    if (input.positions?.length) {
      const current = input.positions.findIndex((p) => p.id === this.positions.get(id));
      const next = Math.max(
        0,
        Math.min(input.positions.length - 1, current + Math.sign(direction)),
      );
      if (next === current) return false;
      this.position(id, input.positions[next].id);
      return true;
    }
    void this.pulse(id);
    return true;
  }
  pulse(id: string) {
    const engine = this.engine!,
      task = this.pulseQueue
        .then(() => {
          this.viewer.pressFeedback(id, this.inputs.get(id)!.label);
          return engine.press(id);
        })
        .then(() => engine.release(id));
    this.pulseQueue = task.catch((error) => {
      if (this.engine === engine && this.ready) this.error(error);
    });
    return task;
  }
  press(id: string, pointer: number | string, stage?: string) {
    if (
      !this.ready ||
      !this.powered ||
      !this.inputs.get(id)?.supported ||
      [...this.holds.values()].includes(id)
    )
      return;
    this.holds.set(pointer, id);
    this.feedback([id]);
    this.viewer.pressFeedback(id, this.inputs.get(id)!.label);
    void this.engine!.press(id, stage).catch(this.error);
  }
  release(pointer: number | string) {
    const id = this.holds.get(pointer);
    if (!id) return;
    this.holds.delete(pointer);
    const engine = this.engine;
    void engine?.release(id).catch((error) => {
      if (this.engine === engine && this.ready) this.error(error);
    });

    setTimeout(() => {
      if (!this.holds.size) this.feedback([]);
    }, 160);
  }
  releaseAll() {
    this.stopRotation?.();
    this.stopShutter?.(true);
    for (const key of [...this.holds.keys()]) this.release(key);
  }
  feedback(ids: string[], glow = false) {
    this.active = ids;
    this.glowing = glow;
    this.changed();
  }
  beginDialDrag(id: string, dial: HTMLElement, event: PointerEvent) {
    if (event.button !== 0 || !this.ready || !this.powered || !this.inputs.get(id)?.supported)
      return;
    if (this.stopRotation) return;
    this.detent.beginGesture(event);
    event.preventDefault();
    dial.setPointerCapture(event.pointerId);
    const values = dial.querySelector<HTMLElement>('.vals')!;
    const ticks = dial.classList.contains('ticks');
    const initialTicks = parseFloat(values.style.backgroundPositionX) || 0;
    const centers = Array.from(values.children, (value) => {
      const label = value as HTMLElement;
      return label.offsetLeft + label.offsetWidth / 2;
    });
    const rotation: NonNullable<ControlSession['rotation']> = { dial, fraction: 0 };
    if (centers.length) {
      // Start from the rendered position, even when a previous snap is in flight.
      const transform = getComputedStyle(values).transform;
      const x = new DOMMatrixReadOnly(transform).m41;
      rotation.center = dial.clientWidth / 2 - x;
      values.style.transform = `translateX(${x}px)`;
    }
    this.rotation = rotation;
    values.style.transition = 'none';
    let previous = event.clientX;
    let remainder = 0;
    let tickPosition = initialTicks;
    const move = (next: PointerEvent) => {
      if (next.pointerId !== event.pointerId) return;
      const delta = next.clientX - previous;
      previous = next.clientX;
      if (rotation.center !== undefined) {
        // Measure travel in CSS pixels, using the actual, uneven label spacing.
        const center = Math.max(centers[0], Math.min(centers.at(-1)!, rotation.center - delta));
        rotation.center = center;
        const nearest = centers.reduce(
          (best, candidate, index) =>
            Math.abs(candidate - center) < Math.abs(centers[best] - center) ? index : best,
          0,
        );
        const current = this.index(id);
        const direction = Math.sign(nearest - current);
        for (let n = 0; n < Math.abs(nearest - current); n++) {
          if (this.step(id, direction)) this.detent.step(this.detentSounds[id]);
        }
        const index = this.index(id);
        const distance = center - centers[index];
        const neighbor = centers[index + Math.sign(distance)];
        rotation.fraction =
          neighbor === undefined || distance === 0
            ? 0
            : distance / Math.abs(neighbor - centers[index]);
      } else {
        remainder += delta * (ticks ? 1 : -1);
        const steps = Math.trunc(remainder / 16);
        remainder -= steps * 16;
        for (let n = 0; n < Math.abs(steps); n++) {
          if (this.step(id, Math.sign(steps))) {
            tickPosition += Math.sign(steps) * 16;
            this.detent.step(this.detentSounds[id]);
          }
        }
        rotation.fraction = this.clampFraction(id, remainder / 16);
      }
      this.changed();
      if (ticks) values.style.backgroundPositionX = `${tickPosition + remainder}px`;
      else this.renders.get(id)?.();
    };
    const end = (up: PointerEvent) => {
      if (up.pointerId === event.pointerId) cleanup();
    };
    const cleanup = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', end);
      window.removeEventListener('pointercancel', end);
      dial.removeEventListener('lostpointercapture', end);
      this.rotation = undefined;
      this.changed();
      values.style.transition = ticks
        ? 'background-position-x 140ms ease-out'
        : 'transform 140ms ease-out';
      if (ticks) values.style.backgroundPositionX = `${tickPosition}px`;
      else this.renders.get(id)?.();
      if (dial.hasPointerCapture(event.pointerId)) dial.releasePointerCapture(event.pointerId);
      this.stopRotation = undefined;
    };
    this.stopRotation = cleanup;
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', end);
    window.addEventListener('pointercancel', end);
    dial.addEventListener('lostpointercapture', end);
  }
  private availability() {
    this.changed();
  }
  disable() {
    this.ready = false;
    this.holds.clear();
    this.stopRotation?.();
    this.stopShutter?.();
    this.feedback([]);
    this.availability();
  }
  resume() {
    this.ready = true;
    this.powered = true;
    if (this.powerID) {
      this.positions.set(this.powerID, 'ON');
      this.changed();
      this.renders.get(this.powerID)?.();
    }
    this.availability();
  }
  setPowered(on: boolean) {
    this.powered = on;
    if (this.powerID) this.positions.set(this.powerID, on ? 'ON' : 'OFF');
    this.changed();
  }
  synchronize(positions: Record<string, string>) {
    for (const [id, value] of Object.entries(positions)) {
      this.positions.set(id, value);
      this.changed();
      this.renders.get(id)?.();
    }
    if (this.powerID && positions[this.powerID]) this.powered = positions[this.powerID] === 'ON';
    this.availability();
  }
  replacePanel(data: DevicePanel) {
    this.data = data;
    this.syncPanel();
    this.changed();
  }
  private syncPanel() {
    this.powerID = undefined;
    this.detentSounds = this.data?.detentSounds ?? {};
    const visit = (nodes: ControlNode[]) => {
      for (const node of nodes) {
        if (node.operation === 'power' && node.id) {
          this.powerID = node.id;
          this.positions.set(node.id, this.powered ? 'ON' : 'OFF');
        }
        if (node.variant === 'segment' && node.id && !this.positions.has(node.id)) {
          const first =
            this.inputs.get(node.id)?.positions?.[0]?.id ?? Object.keys(node.labels ?? {})[0];
          if (first !== undefined) this.positions.set(node.id, first);
        }
        visit(node.children ?? []);
      }
    };
    visit(this.data?.layout ?? []);
  }
  clear() {
    this.releaseAll();
    this.ready = false;
    this.stopShutter?.();
    this.powered = true;
    this.powerID = undefined;
    this.data = undefined;
    this.changed();
    this.inputs.clear();
    this.detentSounds = {};
    this.positions.clear();
    this.relativeSteps.clear();

    this.renders.clear();
    this.engine = undefined;
  }
}

function nodeMarking(
  node: Pick<ControlNode, 'face' | 'icon' | 'variant'> & { caption?: string },
): Marking {
  const icon = node.variant === 'record' ? 'record' : node.icon;
  return {
    ...(node.face ? { face: node.face } : {}),
    ...(icon ? { icon } : {}),
    ...(node.caption ? { shortName: node.caption } : {}),
  };
}

function panelMarking(
  session: ControlSession,
  id: string,
  node: Pick<ControlNode, 'face' | 'icon' | 'variant' | 'caption'>,
): Marking {
  const control = session.physicalControls?.controls[id.split('.')[0]!];
  return control ? marking(control) : nodeMarking(node);
}

function Key(props: {
  session: ControlSession;
  id: string;
  node: Pick<ControlNode, 'face' | 'icon' | 'hint' | 'variant'>;
  class?: string;
  children?: JSX.Element;
}) {
  return (
    <button
      type="button"
      class={[
        props.children === undefined ? 'key-wrapper' : props.class,
        { rec: props.node.variant === 'record', wifi: !!props.node.hint },
        props.session.classes(props.id),
      ]}
      data-id={props.id}
      aria-label={props.session.inputs.get(props.id)?.label ?? props.id}
      disabled={props.session.disabled(props.id)}
      aria-disabled={props.session.disabled(props.id) ? 'true' : 'false'}
      onPointerDown={(event) => {
        event.preventDefault();
        event.currentTarget.setPointerCapture(event.pointerId);
        props.session.press(props.id, event.pointerId);
      }}
      onClick={(event) => {
        if (event.detail === 0 && !props.session.disabled(props.id))
          void props.session.pulse(props.id);
      }}
      onPointerEnter={() => props.session.feedback([props.id])}
      onPointerLeave={() => {
        if (!props.session.holds.size) props.session.feedback([]);
      }}
    >
      <Show
        when={props.children !== undefined}
        fallback={
          <KeyFace
            part={{ kind: 'key', marking: panelMarking(props.session, props.id, props.node) }}
            size="panel"
            hint={props.node.hint}
            held={
              props.session.read().holds.size > 0 &&
              [...props.session.holds.values()].includes(props.id)
            }
          />
        }
      >
        {props.children}
      </Show>
    </button>
  );
}
function Shutter(props: { session: ControlSession; id: string }) {
  const [stage, setStage] = createSignal('idle'),
    [left, setLeft] = createSignal('');
  const state = (value: string, offset: string) => {
    setStage(value);
    setLeft(offset);
  };
  return (
    <div
      class={['shutter-wrapper', props.session.classes(props.id)]}
      tabindex="0"
      data-id={props.id}
      role="button"
      aria-label={props.session.inputs.get(props.id)?.label ?? props.id}
      aria-disabled={props.session.disabled(props.id) ? 'true' : 'false'}
      onPointerDown={(event) =>
        props.session.shutterDown(props.id, event.currentTarget, state, event)
      }
      onKeyDown={(event) => props.session.shutterKey(props.id, event)}
    >
      <ShutterFace
        part={{
          kind: 'shutter',
          marking: {},
          ...(stage() === 'half' || stage() === 'full'
            ? { stage: stage() as 'half' | 'full' }
            : {}),
        }}
        size="panel"
        left={left()}
      />
    </div>
  );
}
function Dial(props: { session: ControlSession; id: string; node: ControlNode }) {
  let dial!: HTMLDivElement, values!: HTMLSpanElement;
  const positions = untrack(() => props.session.inputs.get(props.id)?.positions ?? []);
  const selected = () => props.session.read().positions.get(props.id);
  let pending = false;
  // Pointer geometry belongs to this ref; selected values and availability belong to JSX.
  const update = () => {
    if (pending || !dial) return;
    pending = true;
    requestAnimationFrame(() => {
      pending = false;
      if (!dial.isConnected || props.node.variant === 'ticks') return;
      const labels = [...values.children] as HTMLElement[];
      for (const label of labels)
        if (!label.style.width) {
          const wasSelected = label.classList.contains('on');
          label.classList.add('on');
          label.style.width = `${label.offsetWidth}px`;
          label.style.flexShrink = '0';
          label.classList.toggle('on', wasSelected);
        }
      const rotation = props.session.rotation;
      if (rotation?.dial === dial && rotation.center !== undefined) {
        values.style.transform = `translateX(${dial.clientWidth / 2 - rotation.center}px)`;
        return;
      }
      const fraction = rotation?.dial === dial ? rotation.fraction : 0;
      const visual = Math.max(
        0,
        Math.min(positions.length - 1, props.session.index(props.id) + fraction),
      );
      const a = labels[Math.floor(visual)],
        b = labels[Math.ceil(visual)];
      if (a && b) {
        const center = (item: HTMLElement) => item.offsetLeft + item.offsetWidth / 2;
        const offset = center(a) + (center(b) - center(a)) * (visual % 1);
        values.style.transform = `translateX(${dial.clientWidth / 2 - offset}px)`;
      }
    });
  };
  createEffect(() => {
    props.session.read();
    return selected();
  }, update);
  onSettled(() => {
    props.session.renders.set(props.id, update);
    const down = (event: PointerEvent) => props.session.beginDialDrag(props.id, dial, event);
    dial.addEventListener('pointerdown', down);
    const observer = new ResizeObserver(update);
    observer.observe(dial);
    update();
    return () => {
      dial.removeEventListener('pointerdown', down);
      observer.disconnect();
      props.session.renders.delete(props.id);
    };
  });
  return (
    <div
      ref={dial}
      class={['dial-wrapper', props.session.classes(props.id)]}
      tabindex="0"
      role="slider"
      aria-label={props.session.inputs.get(props.id)?.label ?? props.node.caption ?? props.id}
      data-id={props.id}
      aria-disabled={props.session.disabled(props.id) ? 'true' : 'false'}
      aria-valuenow={props.session.read().index(props.id)}
      aria-valuemin="0"
      aria-valuemax={Math.max(0, positions.length - 1)}
      aria-valuetext={selected() ?? ''}
      onKeyDown={(event) => {
        if (!['ArrowLeft', 'ArrowDown', 'ArrowRight', 'ArrowUp'].includes(event.key)) return;
        event.preventDefault();
        props.session.detent.beginGesture(event);
        if (props.session.step(props.id, ['ArrowLeft', 'ArrowDown'].includes(event.key) ? -1 : 1))
          props.session.detent.step(props.session.detentSounds[props.id]);
      }}
    >
      <PositionsFace
        part={{
          kind: 'positions',
          marking: panelMarking(props.session, props.id, props.node),
          position: selected(),
        }}
        size="panel"
        variant={props.node.variant === 'ticks' ? 'ticks' : 'dial'}
        positions={positions.map((p) => ({
          id: p.id,
          label: props.node.labels?.[p.id] ?? p.label,
        }))}
        valuesRef={(element) => {
          values = element;
        }}
        valuesStyle={{
          'background-position-x':
            props.node.variant === 'ticks'
              ? `${((props.session.read().relativeSteps.get(props.id) ?? 0) + (props.session.read().rotation?.dial === dial ? props.session.read().rotation!.fraction : 0)) * 16}px`
              : undefined,
        }}
      />
    </div>
  );
}
/** The (−) badge shown on each control while its layout is edited. */
function RemoveBadge(props: { editor?: LayoutEditor; path: NodePath; label: string }) {
  return (
    <Show when={props.editor?.editing()}>
      <button
        type="button"
        class="layout-remove"
        aria-label={`Remove ${props.label}`}
        data-layout-remove={props.path.join('.')}
      >
        <svg viewBox="0 0 10 10" aria-hidden="true">
          <path d="M2 5h6" />
        </svg>
      </button>
    </Show>
  );
}

function Node(props: {
  session: ControlSession;
  editor?: LayoutEditor;
  node: ControlNode;
  path: NodePath;
}) {
  const id = () => props.node.id ?? '';
  const input = () => props.session.inputs.get(id());
  const positions = () =>
    input()?.positions ?? Object.keys(props.node.labels ?? {}).map((id) => ({ id, label: id }));
  const editing = () => props.editor?.editing() ?? false;
  const layoutPath = () => (editing() ? props.path.join('.') : undefined);
  const selected = () => (props.editor?.isSelected(props.path) ? 'true' : undefined);
  const label = () =>
    input()?.label ?? props.node.caption ?? props.node.face?.join('/') ?? props.node.kind;
  return (
    <Switch>
      <Match when={props.node.kind === 'divider'}>
        <hr />
      </Match>
      <Match when={props.node.kind === 'space'}>
        <div class="sp" />
      </Match>
      <Match
        when={
          props.node.kind === 'command' ||
          props.node.kind === 'rocker' ||
          props.node.kind === 'dpad'
        }
      >
        <div
          class="control-cluster"
          data-layout-path={layoutPath()}
          data-layout-selected={selected()}
        >
          <RemoveBadge editor={props.editor} path={props.path} label={label()} />
          <Show when={props.node.caption}>
            <div class="cap">{props.node.caption}</div>
          </Show>
          {(() => {
            const renderKey = (action: string, className: string, content: JSX.Element) => {
              const slot =
                { decrease: 'left', increase: 'right', press: 'center' }[action] ?? action;
              const bound = props.node.bindings?.[slot];
              return bound ? (
                <Key session={props.session} id={bound} node={{}} class={className}>
                  {content}
                </Key>
              ) : (
                <span class={className}>{content}</span>
              );
            };
            return (
              <ControlFace
                part={{
                  kind:
                    props.node.kind === 'dpad'
                      ? 'pad'
                      : props.node.kind === 'rocker'
                        ? 'rocker'
                        : 'command',
                  marking: panelMarking(
                    props.session,
                    props.node.kind === 'dpad'
                      ? (props.node.bindings?.center ?? props.node.bindings?.up ?? '')
                      : (props.node.bindings?.left ?? ''),
                    props.node,
                  ),
                }}
                size="panel"
                renderKey={renderKey}
              />
            );
          })()}
        </div>
      </Match>
      <Match when={props.node.kind !== 'control'}>
        <div class={props.node.className ?? ''}>
          <For each={props.node.children ?? []}>
            {(child, index) => (
              <Node
                node={child}
                session={props.session}
                editor={props.editor}
                path={[...props.path, index()]}
              />
            )}
          </For>
        </div>
      </Match>
      <Match when={props.node.variant === 'key' || props.node.variant === 'record'}>
        <Show
          when={props.node.caption}
          fallback={
            <div
              class={`control-node ${props.node.className ?? ''}`}
              data-id={id()}
              data-layout-path={layoutPath()}
              data-layout-selected={selected()}
              style={editing() ? {} : { display: 'contents' }}
            >
              <RemoveBadge editor={props.editor} path={props.path} label={label()} />
              <Key
                session={props.session}
                id={id()}
                node={props.node}
                class={props.node.className}
              />
            </div>
          }
        >
          <div
            class={`control-node ${props.node.className ?? ''}`}
            data-id={id()}
            data-layout-path={layoutPath()}
            data-layout-selected={selected()}
          >
            <RemoveBadge editor={props.editor} path={props.path} label={label()} />
            <div class="cap">{props.node.caption}</div>
            <Key session={props.session} id={id()} node={props.node} class={props.node.className} />
          </div>
        </Show>
      </Match>
      <Match when={true}>
        <div
          class={`control-node ${props.node.className ?? ''}`}
          data-id={id()}
          data-layout-path={layoutPath()}
          data-layout-selected={selected()}
        >
          <RemoveBadge editor={props.editor} path={props.path} label={label()} />
          <Show when={props.node.caption}>
            <div class="cap">{props.node.caption}</div>
          </Show>
          <Switch>
            <Match when={props.node.variant === 'segment'}>
              <PositionsFace
                part={{
                  kind: 'positions',
                  marking: panelMarking(props.session, id(), props.node),
                  position: props.session.read().positions.get(id()),
                }}
                size="panel"
                positions={positions().map((p) => ({
                  id: p.id,
                  label: props.node.labels?.[p.id] ?? p.label,
                }))}
                renderKey={(position, className, content) => (
                  <button
                    type="button"
                    data-value={position}
                    class={[className, props.session.classes(id())]}
                    aria-pressed={
                      props.session.read().positions.get(id()) === position ? 'true' : 'false'
                    }
                    aria-label={`${input()?.label ?? props.node.caption ?? id()} ${position}`}
                    disabled={props.session.disabled(id())}
                    aria-disabled={props.session.disabled(id()) ? 'true' : 'false'}
                    onClick={() => props.session.position(id(), position)}
                  >
                    {content}
                  </button>
                )}
              />
            </Match>
            <Match when={props.node.variant === 'shutter'}>
              <Shutter session={props.session} id={id()} />
            </Match>
            <Match when={true}>
              <Dial session={props.session} id={id()} node={props.node} />
            </Match>
          </Switch>
        </div>
      </Match>
    </Switch>
  );
}
function Combo(props: { session: ControlSession; item: PowerCombo }) {
  const [held, setHeld] = createSignal(false);
  const glow = () =>
    props.session.feedback(
      props.item.inputs.map((input) => input.id),
      true,
    );
  return (
    <button
      class={['combo', { held: held() }]}
      type="button"
      aria-label={props.item.label}
      disabled={!props.session.read().ready}
      onPointerEnter={glow}
      onPointerLeave={() => props.session.feedback([])}
      onFocus={glow}
      onBlur={() => props.session.feedback([])}
      onPointerDown={() => {
        setHeld(true);
        glow();
      }}
      onPointerUp={() => setHeld(false)}
      onPointerCancel={() => {
        setHeld(false);
        props.session.feedback([]);
      }}
      onClick={() => {
        if (props.session.ready) void props.session.combo(props.item).catch(props.session.error);
      }}
    >
      <Show when={props.session.physicalControls}>
        {(controls) => <PowerComboKeys controls={controls()} modes={props.item.modes} />}
      </Show>
      <span class="what">{props.item.label}</span>
    </button>
  );
}
export function Controls(props: { session?: ControlSession; editor?: LayoutEditor }) {
  /** While editing, a tap selects or removes a control and never reaches the emulator. */
  function editTap(event: Event) {
    const editor = props.editor;
    if (!editor?.editing() || !(event.target instanceof Element)) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.type !== 'click') return;
    const path = (attribute: string) =>
      event.target instanceof Element
        ? event.target
            .closest<HTMLElement>(`[${attribute}]`)
            ?.getAttribute(attribute)
            ?.split('.')
            .map(Number)
        : undefined;
    const removed = path('data-layout-remove');
    if (removed) return editor.remove(removed);
    const picked = path('data-layout-path');
    editor.select(picked);
    // Keep the selected control above its sheet.
    const control = event.target.closest('[data-layout-path]');
    if (picked && control) window.scrollBy({ top: control.getBoundingClientRect().top - 16 });
  }
  return (
    <Show when={props.session}>
      {(session) => (
        <Show when={session().read().data}>
          {(data) => (
            <div
              class={['control-surface', { 'editing-layout': props.editor?.editing() ?? false }]}
              ref={(element) => {
                for (const type of ['pointerdown', 'pointerup', 'click', 'keydown'])
                  element.addEventListener(type, editTap, true);
              }}
            >
              <div class="control-layout" style={{ display: 'contents' }}>
                <For each={data().layout}>
                  {(node, index) => (
                    <Node session={session()} editor={props.editor} node={node} path={[index()]} />
                  )}
                </For>
              </div>
              <hr />
              <Show when={data().combos.length}>
                <div class="cap">POWER-ON COMBOS</div>
                <div class="combos">
                  <For each={data().combos}>
                    {(item) => <Combo session={session()} item={item} />}
                  </For>
                </div>
              </Show>
              <Show when={props.editor && session().read().ready && !props.editor.editing()}>
                <button type="button" class="edit-controls" onClick={() => props.editor?.begin()}>
                  Edit Controls…
                </button>
              </Show>
            </div>
          )}
        </Show>
      )}
    </Show>
  );
}
