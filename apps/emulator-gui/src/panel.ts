import type { PhysicalControls } from 'fujimod';
import type { ControlInput } from 'fujimod/emulator';
import type { DevicePanel } from '@fujimod/emulator-ui/ui/layout';
import { defaultLayout } from '@fujimod/emulator-ui/ui/default-layout';

/** Build the shared panel from the native session's own input catalog. */
export function nativePanel(
  body: string,
  inputs: ControlInput[],
  controls: PhysicalControls,
): DevicePanel {
  const layout = defaultLayout(inputs, controls);
  return { ...layout, body };
}
