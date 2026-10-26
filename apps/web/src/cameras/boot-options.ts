import { bootSequence } from '@fujimod/emulator-ui/ui/power-combo';
import type { BootCombination, BootMode, PhysicalControls } from 'fujimod';
import type { DevicePanel, PowerCombo } from '@fujimod/emulator-ui/ui/layout';

export function bootCombination(document: PhysicalControls, mode: BootMode): BootCombination {
  const combination = document.boot?.[mode];
  if (!combination) throw new Error(`No ${mode} boot combination for ${document.body_id}`);
  return combination;
}

export function panelBootCombos(
  document: PhysicalControls,
  modes: BootMode[],
): DevicePanel['combos'] {
  const labels: Record<BootMode, string> = {
    sd: 'SD card boot',
    firmware_update: 'Firmware update',
    service: 'Service mode',
  };
  return modes.map((mode): PowerCombo => {
    const combination = bootSequence(document, [mode]);
    const inputs: PowerCombo['inputs'] = combination.hold.map(({ id, stage }) =>
      stage == null ? { id } : { id, stage },
    );
    if (combination.usb != null) {
      inputs.unshift({ id: 'usb', position: combination.usb ? 'on' : 'off' });
    }
    return {
      label: labels[mode],
      inputs,
      modes: [mode],
    };
  });
}
