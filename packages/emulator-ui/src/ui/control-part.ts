import type { ControlIcon, PhysicalControl, PhysicalControls } from 'fujimod';

export interface Marking {
  face?: string[];
  icon?: ControlIcon;
  shortName?: string;
}
export type ControlPart =
  | { kind: 'key'; marking: Marking }
  | { kind: 'shutter'; marking: Marking; stage?: 'half' | 'full' }
  | { kind: 'rocker'; marking: Marking; held?: 'press' | 'decrease' | 'increase' }
  | { kind: 'command'; marking: Marking; held?: 'press' | 'decrease' | 'increase' }
  | { kind: 'pad'; marking: Marking; held?: 'up' | 'down' | 'left' | 'right' | 'press' }
  | { kind: 'positions'; marking: Marking; position?: string };

export function marking(control: PhysicalControl): Marking {
  return {
    ...(control.face ? { face: control.face.split('/') } : {}),
    ...(control.icon ? { icon: control.icon } : {}),
    ...(control.short_name ? { shortName: control.short_name } : {}),
  };
}

/** Map only the declared mechanism and action; a manual name never selects a drawing. */
export function controlPart(
  control: PhysicalControl,
  action?: string,
  stage?: string,
): ControlPart {
  const behavior = control.behavior;
  const mark = marking(control);
  const invalid = () => new Error(`Undeclared control action ${action ?? stage ?? '(none)'}`);
  if (!behavior) throw new Error('Control has no declared mechanism');
  if (stage !== undefined && behavior.kind !== 'shutter') throw invalid();
  switch (behavior.kind) {
    case 'button':
      if (action !== undefined && action !== 'press') throw invalid();
      return { kind: 'key', marking: mark };
    case 'shutter': {
      if (action !== undefined) throw invalid();
      if (stage !== undefined && stage !== 'half' && stage !== 'full') throw invalid();
      if (stage !== undefined && !(`${stage}_press` in behavior.stages)) throw invalid();
      return { kind: 'shutter', marking: mark, ...(stage ? { stage } : {}) };
    }
    case 'pad':
      if (action === 'press') {
        if (!behavior.press && !control.pushable) throw invalid();
      } else if (action !== undefined && !(action in behavior.directions)) throw invalid();
      if (action !== undefined && !['up', 'down', 'left', 'right', 'press'].includes(action))
        throw invalid();
      return {
        kind: 'pad',
        marking: mark,
        held: action as Extract<ControlPart, { kind: 'pad' }>['held'],
      };
    case 'rocker':
      if (action === 'press') {
        if (!behavior.press && !control.pushable) throw invalid();
      } else if (action !== undefined && action !== 'decrease' && action !== 'increase')
        throw invalid();
      return {
        kind: 'rocker',
        marking: mark,
        held: action as Extract<ControlPart, { kind: 'rocker' }>['held'],
      };
    case 'dial':
    case 'ring':
      if (behavior.motion.kind === 'steps') {
        if (action === 'press') {
          if (!behavior.press && !control.pushable) throw invalid();
        } else if (action !== undefined && action !== 'decrease' && action !== 'increase')
          throw invalid();
        return {
          kind: 'command',
          marking: mark,
          held: action as Extract<ControlPart, { kind: 'command' }>['held'],
        };
      }
      if (action !== undefined && !behavior.motion.positions.some((p) => p.id === action))
        throw invalid();
      return { kind: 'positions', marking: mark, position: action };
    case 'switch':
    case 'lever':
      if (action !== undefined && !behavior.positions.some((p) => p.id === action)) throw invalid();
      return { kind: 'positions', marking: mark, position: action };
    case 'touch':
      throw new Error('Touch controls have no physical face');
  }
}

export function heldControlPart(
  controls: PhysicalControls,
  id: string,
  stage?: string,
): ControlPart {
  const [parent, action, extra] = id.split('.');
  const control = controls.controls[parent!];
  if (!control || extra !== undefined) throw new Error(`Unknown control id ${id}`);
  return controlPart(control, action, stage);
}
