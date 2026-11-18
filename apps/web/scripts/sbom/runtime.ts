import { cargoComponents } from './cargo.ts';
import type { Component } from './bom.ts';

export interface RuntimeSbom {
  schema: 2;
  components: Component[];
}

export interface EmulatorLinks {
  camera: string[];
  webgpu: string[];
}

export function emulatorComponents(
  root: string,
  links: EmulatorLinks,
  perf: boolean,
  webgpu: boolean,
) {
  return cargoComponents(root, [
    {
      package: 'fujimod-emulator-javascript',
      target: 'wasm32-unknown-emscripten',
      product: 'emulator',
      features: perf ? ['perf'] : [],
      linkedDirectories: links.camera,
    },
    ...(webgpu
      ? [
          {
            package: 'fujimod-emulator-ip-dmp-webgpu',
            target: 'wasm32-unknown-unknown',
            product: 'emulator' as const,
            linkedDirectories: links.webgpu,
          },
        ]
      : []),
  ]);
}
