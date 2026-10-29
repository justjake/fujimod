import type { WorkerReply } from './dmp-gpu-worker';

const output = document.createElement('pre');
document.body.append(output);
const worker = new Worker('/tests/dmp-gpu-worker.ts', { type: 'module' });
worker.addEventListener('message', (event: MessageEvent<WorkerReply>) => {
  if ('stage' in event.data) {
    output.textContent = event.data.stage;
    document.body.dataset.stage = event.data.stage;
    return;
  }
  output.textContent = JSON.stringify(event.data, null, 2);
  document.body.dataset.proof = event.data.ok ? 'passed' : 'failed';
  worker.terminate();
});
worker.addEventListener('error', (event) => {
  output.textContent = event.message;
  document.body.dataset.proof = 'failed';
  worker.terminate();
});
worker.postMessage(null);
