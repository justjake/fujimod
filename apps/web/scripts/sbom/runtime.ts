import { cargoComponents } from './cargo.ts';

export function emulatorComponents(root: string, profiling: boolean, webgpu: boolean) {
  return cargoComponents(root, [
    {
      package: 'fujimod-emulator-javascript',
      target: 'wasm32-unknown-emscripten',
      product: 'emulator',
      features: profiling ? ['perf'] : [],
    },
    ...(webgpu
      ? [
          {
            package: 'fujimod-emulator-ip-dmp-webgpu',
            target: 'wasm32-unknown-unknown',
            product: 'emulator' as const,
          },
        ]
      : []),
  ]);
}
