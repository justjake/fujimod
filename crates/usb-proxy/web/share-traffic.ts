import type { Operation, Completion } from './peer.ts';
export interface RemoteRequest {
  id: number;
  device?: string;
  operation: Operation;
  timeout_ms: number;
}
interface Identity {
  controller: string;
  id: number;
  device?: string;
  operation: Readonly<Operation>;
  timeoutMs: number;
}
export type ShareTrafficEvent = Identity &
  (
    | { phase: 'submitted' }
    | {
        phase: 'completed';
        elapsedMs: number;
        completion: Readonly<
          Pick<Completion, 'data' | 'dataLength' | 'transferred' | 'error'> & {
            deviceCount: number;
          }
        >;
      }
  );
// Passive, optional observation of owner-side requests. Observers cannot change
// the request, completion, USB ownership or whether a reply reaches the peer.
function notify(
  callback: ((event: ShareTrafficEvent) => void) | undefined,
  event: ShareTrafficEvent,
) {
  if (!callback) return;
  try {
    callback(Object.freeze(event));
  } catch {
    // Diagnostics must never change transport behavior.
  }
}
// Diagnostic observers receive private bounded prefixes, never live payload buffers.
const CAPTURE = 4096;
const snapshot = (data: Uint8Array | undefined) =>
  data instanceof Uint8Array ? data.slice(0, CAPTURE) : new Uint8Array();
export async function observeRequest(
  controller: string,
  request: RemoteRequest,
  execute: () => Promise<Completion>,
  callback?: (event: ShareTrafficEvent) => void,
) {
  const operation = {
    ...request.operation,
    data: callback ? snapshot(request.operation.data) : undefined,
    dataLength: request.operation.data?.byteLength ?? 0,
  };
  if (operation.setup) operation.setup = Object.freeze({ ...operation.setup });
  const identity = {
    controller,
    id: request.id,
    device: request.device,
    operation: Object.freeze(operation),
    timeoutMs: request.timeout_ms,
  };
  const started = performance.now();
  notify(callback, { ...identity, phase: 'submitted' });
  const completion: Completion = {
    devices: [],
    data: new Uint8Array(),
    transferred: 0,
    error: null,
  };
  try {
    Object.assign(completion, await execute());
  } catch (error) {
    completion.error = error instanceof Error ? error.message : String(error);
  }
  notify(callback, {
    ...identity,
    phase: 'completed',
    elapsedMs: performance.now() - started,
    completion: Object.freeze({
      data: callback ? snapshot(completion.data) : undefined,
      dataLength: completion.data?.byteLength ?? 0,
      transferred: completion.transferred,
      error: completion.error,
      deviceCount: completion.devices?.length ?? 0,
    }),
  });
  return completion;
}
