import { validateCatalog, type Catalog } from '../../catalog';
import type { ToolSession } from '../camera-model';

export type DebugModeSession = ToolSession['debug'];

/** Persist debug access on or off through the connected USB session. */
export interface DebugModeOperations {
  enter(firmwareId: string, progress: (line: string) => void): Promise<void>;
  leave(firmwareId: string, progress: (line: string) => void): Promise<void>;
}

declare const __FUJIMOD_CATALOG__: string;
async function definition(firmwareId: string) {
  const response = await fetch(new URL(__FUJIMOD_CATALOG__, document.baseURI));
  if (!response.ok) throw Error('Could not load the firmware catalog.');
  const catalog = validateCatalog((await response.json()) as Catalog);
  const firmware = catalog.firmwares.find((entry) => entry.id === firmwareId);
  if (!firmware?.camera_table) throw Error('The selected firmware has no camera table.');
  const table = await fetch(new URL(firmware.camera_table, document.baseURI));
  if (!table.ok) throw Error('Could not load the camera table.');
  return [await table.text(), firmware.id, firmware.sha256] as const;
}

/** Run each debug-mode step in its own session on the selected camera. */
export function debugModeOperations(
  run: <T>(tool: string, operation: (debug: DebugModeSession) => Promise<T>) => Promise<T>,
): DebugModeOperations {
  async function step(
    tool: string,
    method: 'enterDebugMode' | 'leaveDebugMode',
    firmwareId: string,
    progress: (line: string) => void,
  ) {
    const args = await definition(firmwareId);
    const result = await run(tool, async (debug) =>
      method === 'enterDebugMode' ? debug.enterDebugMode(...args) : debug.leaveDebugMode(...args),
    );
    for (const line of result.progress) progress(line);
  }
  return {
    enter: (firmwareId, progress) =>
      step('Enable debug mode', 'enterDebugMode', firmwareId, progress),
    leave: (firmwareId, progress) =>
      step('Disable debug mode', 'leaveDebugMode', firmwareId, progress),
  };
}
