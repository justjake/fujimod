import type { EmulatorManifest } from 'fujimod/emulator';

/** A generated ARM loop, real SD host/FAT media and a clocked scene source. */
export async function wasmSessionFixture() {
  const dat = new Uint8Array([0x01, 0x00, 0x00, 0x15]); // inverted b .
  const sha256 = [...new Uint8Array(await crypto.subtle.digest('SHA-256', dat))]
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('');
  const format = {
    sectors: 0x800000,
    max_dirty_sectors: 131072,
    partition_start: 2048,
    sectors_per_cluster: 64,
    label: 'TEST',
  };
  const node = (
    name: string,
    model: Record<string, unknown>,
    windows: { base: number; size: number }[] = [],
  ) => ({ name, boot_entries: ['loader'], windows, dependencies: [], model });
  const mapping = { address: 0x10000, physical: 0x10000, size: 0x4000, permissions: 7 };
  const manifest: EmulatorManifest = {
    schema: 1,
    abi: 1,
    executionPolicy: 'exact',
    module: '../wasm-proof/camera.mjs',
    wasm: '../wasm-proof/camera.wasm',
    construction: 'session',
    ticksPerSecond: 12000000,
    bootTicks: 0,
    output: 0,
    session: {
      schema: 1,
      firmwareId: 'generated-session',
      bootSource: 'internal',
      quantum: 100,
      execution: {
        processors: [
          {
            model: 'cortex-r5f',
            released: true,
            registers: [['Pc', 0x10000]],
            mprofile: null,
            rprofile: {
              cp14_identity: 0x4003,
              tcm: { atcm: { bytes: 0, reset: 0 }, btcm: { bytes: 0, reset: 0 } },
            },
          },
        ],
        ram: [{ base: 0x10000, size: 0x4000 }],
        mappings: [{ core: 0, ...mapping }],
        dma_mappings: [{ ...mapping, permissions: 3 }],
        decoders: [],
        instructions_per_tick: [1, 1],
        peripherals: [0xfffb3000, 0xfff70000].map((address) => ({
          core: 0,
          address,
          physical: address,
          size: 4096,
        })),
      },
      devices: {
        schema: 1,
        boot_entry: 'loader',
        wires: {},
        nodes: [
          node(
            'irq',
            {
              kind: 'exr2-interrupt-controller',
              options: { arbitration: 'unique' },
              core: 0,
              base: 0xfffb3000,
              routes: null,
              software: [],
            },
            [{ base: 0xfffb3000, size: 4096 }],
          ),
          node(
            'sd',
            {
              kind: 'sd-host',
              options: {
                base: 0xfff70000,
                protocol: 'sdhc',
                transfer_ticks: 2,
                clock_ticks: 2,
                rca: 4660,
                ram_bytes: 0x14000,
              },
              card: { kind: 'formatted', options: format, files: 'sd_files' },
              present: true,
              protected: false,
              irq: { Core: { core: 0, line: 80 } },
            },
            [{ base: 0xfff70000, size: 0x108 }],
          ),
          node('frames', {
            kind: 'external-frames',
            options: { period_ticks: 10 },
            images: 'image',
            statistics: 'statistics',
          }),
        ],
      },
      scene: {
        files: { sd_files: { '/DCIM/100_FUJI/A.JPG': 'photo' } },
        images: { image: { width: 2, height: 2, stride: 4, asset: 'pixels' } },
        statistics: { statistics: [{ destination: 0, offset: 0, asset: 'stats' }] },
      },
      assets: {
        photo: { kind: 'bytes', bytes: [...new TextEncoder().encode('camera bytes')] },
        pixels: { kind: 'repeat', pattern: [64, 64, 128, 128], size: 8 },
        stats: { kind: 'repeat', pattern: [0], size: 1024 },
      },
      firmware: {
        size: dat.length,
        sha256,
        format: 'dat',
        mappings: [
          { name: 'loop', offset: 0, size: 4, base: 0x10000, encoding: 'raw', executable: true },
        ],
      },
      card: { node: 'sd', format },
      sceneInput: { images: 'image', sensor: null, node: 'frames' },
    },
  };
  return { dat, manifest };
}
