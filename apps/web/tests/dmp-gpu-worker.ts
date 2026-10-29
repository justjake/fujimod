type RendererModule = {
  default(input?: URL): Promise<unknown>;
  render_capture(capture: Uint8Array): Promise<unknown>;
};

export type WorkerReply =
  | { ok: true; stage: string }
  | { ok: true; proof: unknown }
  | { ok: false; error: string };

const worker = self as unknown as DedicatedWorkerGlobalScope;

worker.addEventListener('message', async () => {
  try {
    worker.postMessage({ ok: true, stage: 'loading renderer module' } satisfies WorkerReply);
    const moduleUrl = new URL('/dmp_webgpu.js', worker.location.href),
      wasmUrl = new URL('/dmp_webgpu_bg.wasm', worker.location.href);
    const loaded: unknown = await import(moduleUrl.href);
    const renderer = loaded as RendererModule;
    worker.postMessage({ ok: true, stage: 'initializing WebGPU' } satisfies WorkerReply);
    await renderer.default(wasmUrl);
    worker.postMessage({ ok: true, stage: 'loading saved capture' } satisfies WorkerReply);
    const response = await fetch('/capture.dmpcap');
    if (!response.ok) throw Error('Saved DMP capture is unavailable.');
    const capture = new Uint8Array(await response.arrayBuffer());
    worker.postMessage({ ok: true, stage: 'rendering saved capture' } satisfies WorkerReply);
    const proof = await renderer.render_capture(capture);
    worker.postMessage({ ok: true, proof } satisfies WorkerReply);
  } catch (error) {
    worker.postMessage({
      ok: false,
      error: error instanceof Error ? error.message : String(error),
    } satisfies WorkerReply);
  }
});
