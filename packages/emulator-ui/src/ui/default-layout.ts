import type { PhysicalControls } from 'fujimod';
import { marking } from './control-part';
import type { ControlInput } from 'fujimod/emulator';
import { DEVICE_PANEL_SCHEMA, type ControlNode, type DevicePanel } from './layout';

export type Variant = NonNullable<ControlNode['variant']>;

export type InputGroup =
  | {
      kind: 'pad';
      id: string;
      up: ControlInput;
      down: ControlInput;
      left: ControlInput;
      right: ControlInput;
      center?: ControlInput;
    }
  | {
      kind: 'command' | 'rocker';
      id: string;
      decrease: ControlInput;
      increase: ControlInput;
      press?: ControlInput;
    }
  | { kind: 'single'; input: ControlInput };

/** The presentations an input allows. Relative motion, positions and stages decide before kind. */
export function compatibleVariants(input: ControlInput): Variant[] {
  if (input.relative) return ['ticks', 'dial'];
  const positions = input.positions?.length ?? 0;
  if (positions > 5) return ['dial', 'segment', 'ticks'];
  if (positions > 0) return ['segment', 'dial', 'ticks'];
  const stages = input.stages?.map((stage) => stage.id) ?? [];
  if (stages.includes('half') && stages.includes('full')) return ['shutter'];
  return ['key', 'record'];
}

/** Group pad directions and command steps by the id prefix before ".". */
export function groupInputs(
  inputs: readonly ControlInput[],
  controls: PhysicalControls,
): InputGroup[] {
  const byPrefix = new Map<string, Map<string, ControlInput>>();
  for (const input of inputs) {
    const dot = input.id.indexOf('.');
    if (dot < 0) continue;
    const prefix = input.id.slice(0, dot);
    const members = byPrefix.get(prefix) ?? new Map<string, ControlInput>();
    members.set(input.id.slice(dot + 1), input);
    byPrefix.set(prefix, members);
  }
  const menuOk = inputs.find((input) => controls.controls[input.id]?.face === 'MENU/OK');
  const grouped = new Set<string>();
  const groups = new Map<string, InputGroup>();
  for (const [prefix, members] of byPrefix) {
    const up = members.get('up'),
      down = members.get('down'),
      left = members.get('left'),
      right = members.get('right');
    const behavior = controls.controls[prefix]?.behavior;
    if (behavior?.kind === 'pad' && up && down && left && right) {
      const center = members.get('press') ?? menuOk;
      groups.set(prefix, { kind: 'pad', id: prefix, up, down, left, right, center });
      for (const input of [up, down, left, right, center]) if (input) grouped.add(input.id);
      continue;
    }
    const decrease = members.get('decrease');
    const increase = members.get('increase');
    if (
      behavior &&
      (behavior.kind === 'rocker' ||
        ((behavior.kind === 'dial' || behavior.kind === 'ring') &&
          behavior.motion.kind === 'steps')) &&
      decrease &&
      increase
    ) {
      const press = members.get('press');
      groups.set(prefix, {
        kind: behavior.kind === 'rocker' ? 'rocker' : 'command',
        id: prefix,
        decrease,
        increase,
        press,
      });
      for (const input of [decrease, increase, press]) if (input) grouped.add(input.id);
    }
  }
  const result: InputGroup[] = [];
  for (const input of inputs) {
    const prefix = input.id.split('.')[0]!;
    const group = groups.get(prefix);
    if (group) {
      if (!result.includes(group)) result.push(group);
    } else if (!grouped.has(input.id)) result.push({ kind: 'single', input });
  }
  return result;
}

function shown(input: ControlInput) {
  return input.supported && input.kind !== 'touch' && !input.id.endsWith('_latch');
}

/** Keys beside the D-pad and command controls, as on the back of the camera. */
const SIDE_KEYS = 6;

function controlNode(input: ControlInput, controls: PhysicalControls): ControlNode {
  const control = controls.controls[input.id.split('.')[0]!];
  if (!control) throw new Error(`Unknown control id ${input.id}`);
  const mark = marking(control);
  const variant = compatibleVariants(input)[0]!;
  return {
    kind: 'control',
    id: input.id,
    variant: mark.icon === 'record' ? 'record' : variant,
    ...(mark.face ? { face: mark.face } : {}),
    ...(mark.icon ? { icon: mark.icon } : {}),
    ...(mark.shortName ? { caption: mark.shortName } : {}),
    ...(input.positions
      ? { labels: Object.fromEntries(input.positions.map((p) => [p.id, p.label])) }
      : {}),
  };
}

function groupNode(
  group: Exclude<InputGroup, { kind: 'single' }>,
  controls: PhysicalControls,
): ControlNode {
  const mark = marking(controls.controls[group.id]!);
  if (group.kind === 'pad') {
    const center = group.center;
    const centerControl = center && controls.controls[center.id];
    return {
      kind: 'dpad',
      face: centerControl ? marking(centerControl).face : mark.face,
      bindings: {
        up: group.up.id,
        down: group.down.id,
        left: group.left.id,
        right: group.right.id,
        ...(center ? { center: center.id } : {}),
      },
    };
  }
  return {
    kind: group.kind,
    caption: mark.shortName,
    face: mark.face,
    icon: mark.icon,
    bindings: {
      left: group.decrease.id,
      right: group.increase.id,
      ...(group.press ? { center: group.press.id } : {}),
    },
  };
}

/** A usable panel for any body: supported controls, grouped, with short faces. */
export function defaultLayout(
  inputs: readonly ControlInput[],
  controls: PhysicalControls,
): DevicePanel {
  const groups = groupInputs(inputs.filter(shown), controls);
  const positional: ControlNode[] = [];
  const clusters: ControlNode[] = [];
  const keys: ControlNode[] = [];
  for (const group of groups) {
    if (group.kind !== 'single') clusters.push(groupNode(group, controls));
    else if (compatibleVariants(group.input)[0] === 'key')
      keys.push(controlNode(group.input, controls));
    else positional.push(controlNode(group.input, controls));
  }
  positional.sort((a, b) => Number(b.variant === 'shutter') - Number(a.variant === 'shutter'));
  const layout: ControlNode[] = [];
  if (positional.length) layout.push({ kind: 'grid', className: 'g2', children: positional });
  if (clusters.length || keys.length) {
    if (layout.length) layout.push({ kind: 'divider' });
    const rest = clusters.length ? keys.slice(SIDE_KEYS) : keys;
    if (clusters.length)
      layout.push({
        kind: 'grid',
        className: 'back',
        children: [
          {
            kind: 'column',
            className: 'c1',
            children: [
              { kind: 'grid', className: 'side-keys', children: keys.slice(0, SIDE_KEYS) },
            ],
          },
          { kind: 'column', className: 'c2 clusters', children: clusters },
        ],
      });
    if (rest.length) layout.push({ kind: 'grid', className: 'key-grid', children: rest });
  }
  return { schema: DEVICE_PANEL_SCHEMA, body: controls.body_id, combos: [], layout };
}
