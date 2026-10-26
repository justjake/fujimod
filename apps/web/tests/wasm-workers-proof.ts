import type { WorkerMessage, WorkerReceipt } from './wasm-workers-worker';
const query = new URL(location.href).searchParams;
const requested = query.get('workers');
const plan = query.get('plan') || '/__wasm-workers/plan.json';
if (requested !== '0' && requested !== '2') throw Error('Select ?workers=0 or ?workers=2');
const canvas = document.createElement('canvas'),
  output = document.createElement('pre');
canvas.style.cssText = 'width:720px;height:480px;display:block;image-rendering:pixelated';
document.body.append(canvas, output);
const photos: { path: string; url: string }[] = [];
const worker = new Worker(new URL('./wasm-workers-worker.ts', import.meta.url), { type: 'module' });
worker.onmessage = (event: MessageEvent<WorkerMessage>) => {
  const message = event.data;
  if (message.type === 'frame') {
    canvas.width = message.width;
    canvas.height = message.height;
    canvas
      .getContext('2d')!
      .putImageData(
        new ImageData(new Uint8ClampedArray(message.rgba), message.width, message.height),
        0,
        0,
      );
  } else if (message.type === 'photo') {
    photos.push({
      path: message.path,
      url: URL.createObjectURL(new Blob([message.bytes], { type: 'image/jpeg' })),
    });
  } else if (message.type === 'phase') {
    output.textContent = JSON.stringify(message.phase, null, 2);
    console.log(JSON.stringify(message.phase));
  } else {
    const result: WorkerReceipt & { downloads: typeof photos } = {
      ...message.result,
      downloads: photos,
    };
    output.textContent = JSON.stringify(result, null, 2);
    document.body.dataset.proof = 'complete';
    worker.terminate();
  }
};
worker.onerror = (error) => {
  output.textContent = JSON.stringify({ complete: false, error: error.message });
  document.body.dataset.proof = 'complete';
  worker.terminate();
};
worker.postMessage({ workers: Number(requested), plan });
