export const FAULT_UNDO_SECONDS = 10;
export interface EngineFault {
  kind: 'unsupported-device' | 'guest-fault' | 'wasm-trap' | 'exception';
  message: string;
  details: Record<string, unknown>;
  undoSeconds?: number;
}
export class EngineError extends Error {
  readonly fault: EngineFault;
  constructor(fault: EngineFault) {
    super(fault.message);
    this.fault = fault;
    this.name = 'EngineError';
  }
}
export function faultFrom(value: unknown): EngineFault {
  if (value instanceof EngineError) return value.fault;
  const details: Record<string, unknown> =
    value && typeof value === 'object'
      ? value instanceof Error
        ? { type: value.name, error: value.message, stack: value.stack }
        : { ...value }
      : { error: String(value) };
  const text = [details.type, details.error_type, details.error, details.message, details.exception]
    .filter(Boolean)
    .join(' ');
  const kind =
    /Unsupported(?:Device|Scanout)|unsupported device|requires unavailable adapter/i.test(text)
      ? 'unsupported-device'
      : value instanceof WebAssembly.RuntimeError ||
          /RuntimeError:.*(?:unreachable|out of bounds)|wasm trap/i.test(text)
        ? 'wasm-trap'
        : 'cpu' in details ||
            'pc' in details ||
            /guest|mmio|unmapped|instruction|cpu|abort|fault/i.test(text)
          ? 'guest-fault'
          : 'exception';
  if (kind === 'unsupported-device') {
    for (const name of ['device', 'operation', 'address', 'size', 'value', 'cpu', 'pc'])
      details[name] ??= null;
  }
  const message =
    kind === 'unsupported-device'
      ? "We don't know how to simulate this hardware feature yet; it works on a real camera."
      : kind === 'wasm-trap'
        ? 'The emulator runtime stopped unexpectedly.'
        : kind === 'guest-fault'
          ? 'The camera stopped after an execution fault.'
          : 'The emulator stopped unexpectedly.';
  return { kind, message, details };
}
export function stateFault(state: { failure?: unknown; execution_error?: unknown }) {
  if (!state.execution_error && !state.failure) return undefined;
  const primary = state.execution_error || state.failure;
  if (typeof primary !== 'object' || primary === null) return faultFrom(primary);
  const details: Record<string, unknown> = {
    ...(state.failure && typeof state.failure === 'object' ? state.failure : {}),
    ...(state.execution_error && typeof state.execution_error === 'object'
      ? state.execution_error
      : {}),
    failure: state.failure,
    execution_error: state.execution_error,
  };
  if (state.failure && typeof state.failure === 'object') {
    for (const name of ['device', 'operation', 'address', 'size', 'value', 'cpu', 'pc']) {
      details[name] ??= (state.failure as Record<string, unknown>)[name];
    }
  }
  return faultFrom(details);
}
