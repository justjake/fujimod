import { patch, sha256 } from './patcher.ts';
import { validateIndex, verifyPatch, type PatchIndex } from './catalog.ts';
const worker = self as unknown as DedicatedWorkerGlobalScope;
worker.onmessage = async ({
  data: { file, index, artifactBase },
}: MessageEvent<{ file: Blob; index: PatchIndex; artifactBase: string }>) => {
  try {
    validateIndex(index);
    worker.postMessage({ stage: 'Checking firmware…' });
    const base = new Uint8Array(await file.arrayBuffer());
    const digest = await sha256(base);
    const firmware = index.firmwares.find((entry) => entry.sha256 === digest);
    if (!firmware) {
      worker.postMessage({ unsupported: true, digest });
      return;
    }
    if (base.length !== firmware.bytes)
      throw new Error('Firmware size does not match its release entry.');
    worker.postMessage({ stage: `Building ${firmware.model} ${firmware.version} SD boot files…` });
    const response = await fetch(new URL(index.patches[digest], artifactBase), {
      cache: 'no-cache',
    });
    if (!response.ok)
      throw new Error('The release patch could not be downloaded. Please try again.');
    const compiled = new Uint8Array(await response.arrayBuffer());
    await verifyPatch(index, firmware, compiled);
    const result = await patch(base, compiled);
    worker.postMessage({ result, firmware }, [
      result.ffboot.buffer,
      result.ffsys.buffer,
      ...(result.fpupdate ? [result.fpupdate.buffer] : []),
    ]);
  } catch (error) {
    worker.postMessage({ error: error instanceof Error ? error.message : String(error) });
  }
};
