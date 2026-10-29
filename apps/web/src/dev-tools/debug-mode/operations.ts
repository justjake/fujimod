import { serviceWorkflow } from '../service-artifacts';
import type { ToolSession } from '../camera-model';

export type DebugModeSession = ToolSession['debug'];

/** Persist debug access on or off through the connected USB session. */
export interface DebugModeOperations {
  enter(firmwareId: string, progress: (line: string) => void): Promise<void>;
  leave(firmwareId: string, progress: (line: string) => void): Promise<void>;
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
    const args = await serviceWorkflow(firmwareId, 'persistence');
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
