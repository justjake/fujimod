// Network tests substitute the Rust exporter boundary; device ownership is
// exercised against the real Wasm/nusb backend in bindings/javascript/tests/ownership.test.ts.
import type { Completion, Descriptor, Operation, UsbExporter } from './types.ts';
import { decodeMessage, encodeMessage } from './wire.ts';
export function exporterFixture(
  descriptor: Descriptor,
  run: (op: Operation) => Promise<Completion> = async () => ({}),
  free: () => void = () => {},
): UsbExporter {
  let owner: string | undefined;
  let fenced = false;
  return {
    inventoryJson: JSON.stringify([descriptor]),
    busy: () => owner !== undefined,
    owns: (controller) => owner === controller,
    fenced: () => fenced,
    abandon(controller) {
      if (owner !== controller) return false;
      owner = undefined;
      fenced = true;
      return true;
    },
    async execute(controller, bytes) {
      const message = decodeMessage(bytes);
      if (message.type !== 'execute') throw Error('Expected execute');
      const { seq, op } = message;
      let completion: Completion;
      if (op.op === 'open' && owner !== undefined) completion = { error: 'Device in use' };
      else if (op.op !== 'open' && owner !== controller)
        completion = { error: 'Device not claimed' };
      else {
        if (op.op === 'open') owner = controller;
        completion = await run(op);
        if (op.op === 'close') owner = undefined;
      }
      return encodeMessage({ type: 'result', seq, completion });
    },
    stopSharing: () => {},
    shutdown: async () => {},
    free,
  };
}
