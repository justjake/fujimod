import { For, Show } from 'solid-js';
import type { JSX } from '@solidjs/web';
import type { BootHold, BootMode, PhysicalControls } from 'fujimod';
import { heldControlPart } from './control-part';
import { ControlFace, PositionsFace } from './control-faces';
import './control-faces.css';

export function bootSequence(
  controls: PhysicalControls,
  modes: BootMode[],
): { usb?: boolean; hold: BootHold[] } {
  const hold: BootHold[] = [];
  let usb: boolean | undefined;
  for (const mode of modes) {
    const sequence = controls.boot?.[mode];
    if (!sequence) throw new Error(`No ${mode} boot combination for ${controls.body_id}`);
    if (sequence.usb != null) {
      if (usb !== undefined && usb !== sequence.usb)
        throw new Error('Boot modes disagree about USB');
      usb = sequence.usb;
    }
    for (const input of sequence.hold) {
      heldControlPart(controls, input.id, input.stage ?? undefined);
      if (
        !hold.some(
          (other) =>
            other.id === input.id && (other.stage ?? undefined) === (input.stage ?? undefined),
        )
      )
        hold.push(input);
    }
  }
  return { ...(usb !== undefined ? { usb } : {}), hold };
}

export function PowerComboKeys(props: {
  controls: PhysicalControls;
  modes: BootMode[];
}): JSX.Element {
  const sequence = () => bootSequence(props.controls, props.modes);
  return (
    <span class="power-combo-keys">
      <Show when={sequence().usb !== undefined}>
        <span class="combo-usb" aria-label={sequence().usb ? 'USB connected' : 'USB disconnected'}>
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M9 3v5m6-5v5M7 8h10v4a5 5 0 0 1-10 0Zm5 9v5" />
            <Show when={!sequence().usb}>
              <path d="m3 3 18 18" />
            </Show>
          </svg>
        </span>
      </Show>
      <For each={sequence().hold}>
        {(hold) => (
          <ControlFace
            part={heldControlPart(props.controls, hold.id, hold.stage ?? undefined)}
            size="mini"
            held
          />
        )}
      </For>
      <PositionsFace
        part={{ kind: 'positions', marking: {}, position: 'ON' }}
        size="mini"
        positions={[
          { id: 'OFF', label: 'OFF' },
          { id: 'ON', label: 'ON' },
        ]}
      />
    </span>
  );
}
