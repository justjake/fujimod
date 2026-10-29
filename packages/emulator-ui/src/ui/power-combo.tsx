import { For, Show } from 'solid-js';
import type { JSX } from '@solidjs/web';
import type { BootHold, BootMode, PhysicalControls } from 'fujimod';
import { heldControlPart } from './control-part';
import { ControlFace } from './control-faces';
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
          {/* The USB trident: arrowhead, round base, a circle branch and a square branch. */}
          <svg viewBox="0 0 32 32" aria-hidden="true">
            <path class="usb-line" d="M16 25V7M16 21 9.5 16.5V12.5M16 18 22.5 13.5V10.5" />
            <path class="usb-fill" d="M16 2.5 20 8.5H12Z" />
            <circle class="usb-fill" cx="16" cy="26.5" r="3" />
            <circle class="usb-fill" cx="9.5" cy="10.8" r="2.4" />
            <rect class="usb-fill" x="20.4" y="7" width="4.2" height="4.2" />
            <Show when={!sequence().usb}>
              <path class="usb-gap" d="M4.5 4.5 27.5 27.5" />
              <path class="usb-line" d="M4.5 4.5 27.5 27.5" />
            </Show>
          </svg>
          <span>{sequence().usb ? 'Plug into computer' : 'Unplug from computer'}</span>
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
    </span>
  );
}
