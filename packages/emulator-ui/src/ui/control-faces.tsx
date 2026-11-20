import { For, Show, Switch, Match, untrack } from 'solid-js';
import type { JSX } from '@solidjs/web';
import { Icon } from './icons';
import type { ControlPart, Marking } from './control-part';
import { CommandDialDrawing, RockerDrawing } from './control-drawings';
import './control-faces.css';

type RenderKey = (action: string, className: string, content: JSX.Element) => JSX.Element;
interface FaceProps {
  part: ControlPart;
  size: 'panel' | 'mini';
  held?: boolean;
  renderKey?: RenderKey;
}
function partLabel(part: ControlPart): string {
  const label = part.marking.face?.join('/') ?? part.marking.icon ?? part.marking.shortName ?? '';
  const held =
    part.kind === 'command' || part.kind === 'rocker' || part.kind === 'pad'
      ? part.held
      : part.kind === 'shutter'
        ? part.stage
        : part.kind === 'positions'
          ? part.position
          : undefined;
  return [label, held].filter(Boolean).join(' ');
}
function PrintedMark(props: { marking: Marking }) {
  return (
    <Show
      when={props.marking.icon}
      fallback={
        <span class={{ two: (props.marking.face?.length ?? 0) > 1 }}>
          {props.marking.face?.join('\n')}
        </span>
      }
    >
      <Show
        when={props.marking.icon === 'record'}
        fallback={
          <Icon
            name={
              props.marking.icon === 'playback'
                ? 'play'
                : props.marking.icon === 'delete'
                  ? 'trash'
                  : 'flash'
            }
          />
        }
      >
        <i />
      </Show>
    </Show>
  );
}
function ShortName(props: FaceProps) {
  return (
    <Show
      when={
        props.size === 'mini' &&
        !props.part.marking.face &&
        !props.part.marking.icon &&
        props.part.marking.shortName
      }
    >
      <span class="control-short-name">{props.part.marking.shortName}</span>
    </Show>
  );
}
function Actuator(props: {
  render?: RenderKey;
  action: string;
  class: string;
  children?: JSX.Element;
}) {
  return (
    <Show
      when={props.render}
      fallback={
        <span class={props.class} data-action={props.action}>
          {props.children}
        </span>
      }
    >
      {(render) => untrack(() => render()(props.action, props.class, props.children))}
    </Show>
  );
}
export function KeyFace(props: FaceProps & { hint?: string }) {
  return (
    <span
      class="control-face"
      data-size={props.size}
      data-part="key"
      role={props.size === 'mini' ? 'img' : undefined}
      aria-label={props.size === 'mini' ? partLabel(props.part) : undefined}
    >
      <Actuator
        render={props.renderKey}
        action="press"
        class={`key ${props.part.marking.icon === 'record' ? 'rec' : ''} ${props.held ? (props.size === 'mini' ? 'glow' : 'active') : ''} ${props.part.marking.face?.some((line) => line.length > 3) ? 'sm' : ''} ${props.hint ? 'wifi' : ''}`}
      >
        <PrintedMark marking={props.part.marking} />
        <Show when={props.hint}>
          <em>{props.hint}</em>
        </Show>
      </Actuator>
      <ShortName {...props} />
    </span>
  );
}
export function ShutterFace(props: FaceProps) {
  const stage = () => (props.part.kind === 'shutter' ? props.part.stage : undefined);
  return (
    <span
      class="control-face"
      data-size={props.size}
      data-part="shutter"
      role={props.size === 'mini' ? 'img' : undefined}
      aria-label={props.size === 'mini' ? partLabel(props.part) : undefined}
    >
      <span class="seg wide">
        <span class="vals">
          <For each={['half', 'full'] as const}>
            {(value) => (
              <Actuator
                render={props.renderKey}
                action={value}
                // A rendered panel key styles its own stage; a changing class here
                // would rebuild that key and drop the pointer holding it.
                class={
                  props.renderKey || stage() !== value
                    ? ''
                    : `on ${props.size === 'mini' && props.held ? 'glow' : ''}`
                }
              >
                {value.toUpperCase()}
              </Actuator>
            )}
          </For>
        </span>
      </span>
      <ShortName {...props} />
    </span>
  );
}
export function CommandFace(props: FaceProps) {
  return (
    <span
      class="control-face"
      data-size={props.size}
      data-part="command"
      role={props.size === 'mini' ? 'img' : undefined}
      aria-label={props.size === 'mini' ? partLabel(props.part) : undefined}
    >
      <Show when={props.size === 'mini'} fallback={<CommandKnurl {...props} />}>
        <CommandDialDrawing held={props.part.kind === 'command' ? props.part.held : undefined} />
      </Show>
      <ShortName {...props} />
    </span>
  );
}
function CommandKnurl(props: FaceProps) {
  const held = (action: string) =>
    (props.part.kind === 'command' || props.part.kind === 'rocker') && props.part.held === action;
  return (
    <span class="cmd">
      <span class="knurl">
        <For each={['decrease', 'press', 'increase']}>
          {(action) => (
            <Actuator
              render={props.renderKey}
              action={action}
              class={`key ${action === 'press' ? 'tab' : `side ${action === 'decrease' ? 'l' : 'r'}`} ${held(action) ? (props.size === 'mini' ? 'glow' : 'active') : ''}`}
            >
              {action === 'decrease' ? (
                '◀'
              ) : action === 'increase' ? (
                '▶'
              ) : (
                <PrintedMark marking={props.part.marking} />
              )}
            </Actuator>
          )}
        </For>
      </span>
    </span>
  );
}
/** A rocker: on the panel, the command control's knurl with its sides and tab; as a mini, the manual's drawing. */
export function RockerFace(props: FaceProps) {
  return (
    <span
      class="control-face"
      data-size={props.size}
      data-part="rocker"
      role={props.size === 'mini' ? 'img' : undefined}
      aria-label={props.size === 'mini' ? partLabel(props.part) : undefined}
    >
      <Show when={props.size === 'mini'} fallback={<CommandKnurl {...props} />}>
        <RockerDrawing held={props.part.kind === 'rocker' ? props.part.held : undefined} />
      </Show>
      <ShortName {...props} />
    </span>
  );
}
export function PadFace(props: FaceProps) {
  const held = (action: string) => props.part.kind === 'pad' && props.part.held === action;
  const keys = [
    { action: 'up', glyph: '▲', class: 'a u' },
    { action: 'down', glyph: '▼', class: 'a d' },
    { action: 'left', glyph: '◀', class: 'a l' },
    { action: 'right', glyph: '▶', class: 'a r' },
    { action: 'press', glyph: '', class: 'ok' },
  ];
  return (
    <span
      class="control-face"
      data-size={props.size}
      data-part="pad"
      role={props.size === 'mini' ? 'img' : undefined}
      aria-label={props.size === 'mini' ? partLabel(props.part) : undefined}
    >
      <span class="dpad">
        <For each={keys}>
          {(key) => (
            <Actuator
              render={props.renderKey}
              action={key.action}
              class={`key ${key.class} ${held(key.action) ? (props.size === 'mini' ? 'glow' : 'active') : ''}`}
            >
              <Show when={key.action === 'press'} fallback={<span>{key.glyph}</span>}>
                <PrintedMark marking={props.part.marking} />
              </Show>
            </Actuator>
          )}
        </For>
      </span>
      <ShortName {...props} />
    </span>
  );
}
export function PositionsFace(
  props: FaceProps & {
    positions?: readonly { id: string; label: string }[];
    variant?: 'segment' | 'dial' | 'ticks';
    valuesRef?: (element: HTMLSpanElement) => void;
    valuesStyle?: JSX.CSSProperties;
  },
) {
  const selected = () => (props.part.kind === 'positions' ? props.part.position : undefined);
  return (
    <span
      class="control-face"
      data-size={props.size}
      data-part="positions"
      role={props.size === 'mini' ? 'img' : undefined}
      aria-label={props.size === 'mini' ? partLabel(props.part) : undefined}
    >
      <span
        class={
          props.variant === 'dial' || props.variant === 'ticks'
            ? `dial ${props.variant === 'ticks' ? 'ticks' : ''}`
            : 'seg wide'
        }
      >
        <span class="vals" ref={props.valuesRef} style={props.valuesStyle}>
          <For each={props.positions ?? [{ id: selected() ?? '', label: selected() ?? '' }]}>
            {(position) => (
              <Actuator
                render={props.renderKey}
                action={position.id}
                class={
                  selected() === position.id
                    ? `on ${props.size === 'mini' && props.held ? 'glow' : ''}`
                    : ''
                }
              >
                {position.label}
              </Actuator>
            )}
          </For>
        </span>
      </span>
      <ShortName {...props} />
    </span>
  );
}
export function ControlFace(props: FaceProps) {
  return (
    <Switch>
      <Match when={props.part.kind === 'key'}>
        <KeyFace {...props} />
      </Match>
      <Match when={props.part.kind === 'shutter'}>
        <ShutterFace {...props} />
      </Match>
      <Match when={props.part.kind === 'command'}>
        <CommandFace {...props} />
      </Match>
      <Match when={props.part.kind === 'rocker'}>
        <RockerFace {...props} />
      </Match>
      <Match when={props.part.kind === 'pad'}>
        <PadFace {...props} />
      </Match>
      <Match when={props.part.kind === 'positions'}>
        <PositionsFace {...props} />
      </Match>
    </Switch>
  );
}
