import { build } from 'vite';
import solid from '@solidjs/vite-plugin';
import { fileURLToPath } from 'node:url';

// Isolate Vite from Vitest's NODE_ENV=test while bundling the module that starts automatic sharing.
const result = await build({
  configFile: false,
  mode: process.argv[2],
  logLevel: 'silent',
  plugins: [solid({ hot: false })],
  build: {
    write: false,
    minify: true,
    rollupOptions: {
      input: fileURLToPath(new URL('./usb-camera-model.ts', import.meta.url)),
      external: (id) =>
        !id.endsWith('/usb-camera-model.ts') &&
        !id.endsWith('/dev-share.ts') &&
        id !== './dev-share',
      preserveEntrySignatures: 'strict',
    },
  },
});
if (Array.isArray(result) || !('output' in result)) throw Error('Expected one build output');
const chunks = result.output.filter((output) => output.type === 'chunk');
console.log(
  JSON.stringify({
    code: chunks.map((chunk) => chunk.code).join('\n'),
    modules: chunks.flatMap((chunk) => Object.keys(chunk.modules)),
  }),
);
