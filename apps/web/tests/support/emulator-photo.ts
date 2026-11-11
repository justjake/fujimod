import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { deflateSync } from 'node:zlib';
import { loadNodeModule } from '../../../../bindings/javascript/emulator/node.mts';

/** The generated emulator module the web app ships; an explicit path wins. */
export const emulatorModule = fileURLToPath(
  new URL('../../../../bindings/javascript/dist/emulator/camera.mjs', import.meta.url),
);

export function emulatorAvailable(): boolean {
  const path = process.env.FUJIMOD_EMULATOR_MODULE ?? emulatorModule;
  return existsSync(path);
}

const LCD = 0xff990000;
const ADC = 0xff906000;
const RAM = 0x10000;
const PIXELS = 0x12000;

/** A two-pixel-wide program that drives the display and stores the ADC sample. */
function syntheticFirmware(): { bytes: Uint8Array; sha256: string } {
  const words: number[] = [];
  const constant = (register: number, value: number) =>
    words.push((0xe59f0000 | (register << 12)) >>> 0, 0xea000000, value >>> 0);
  const write = (address: number, value: number) => {
    constant(0, address);
    constant(1, value);
    words.push(0xe5801000);
  };
  for (const [offset, value] of [
    [8, 1 << 20],
    [0x50, 0],
    [0x54, PIXELS],
    [0x64, 8],
    [0x68, 0],
    [0x6c, 0],
    [0x70, 0x20002],
    [0x74, 0x20002],
    [4, 1],
  ] as const)
    write(LCD + offset, value);
  constant(1, ADC + 0x10 + 11 * 4);
  constant(3, PIXELS);
  words.push(0xe5912000, 0xe38224ff, 0xe5832000, 0xe5832004, 0xe5832008, 0xe583200c, 0xeafffff8);
  const bytes = new Uint8Array(words.length * 4);
  words.forEach((word, index) => new DataView(bytes.buffer).setUint32(index * 4, word, true));
  const digest = createHash('sha256').update(bytes).digest('hex');
  return { bytes, sha256: digest };
}

function cameraConfig(sha256: string, size: number): Record<string, unknown> {
  const keys = {
    layout: {
      shutter_half: { port: 0, pin: 0 },
      shutter_full: { port: 0, pin: 1 },
      rear_dial: [
        { port: 5, pin: 7 },
        { port: 5, pin: 9 },
      ],
      front_dial: [
        { port: 5, pin: 8 },
        { port: 13, pin: 5 },
      ],
    },
    initial_levels: [
      { pin: { port: 0, pin: 0 }, high: true },
      { pin: { port: 0, pin: 1 }, high: true },
      { pin: { port: 5, pin: 7 }, high: false },
      { pin: { port: 5, pin: 9 }, high: false },
      { pin: { port: 5, pin: 8 }, high: false },
      { pin: { port: 13, pin: 5 }, high: false },
    ],
    samples: { menu: 0x123, back: 2, menu_back: 3, up: 4, down: 5, left: 6, right: 7 },
    detent_return_ns: 60_000_000,
    shutter_half_delay_ns: 30_000_000,
  };
  return {
    schema: 1,
    processor: {
      cores: [
        {
          model: 'cortex-r5f',
          released: true,
          pc: RAM,
          registers: {},
          rprofile: {
            cp14_identity: 0x4003,
            tcm: { atcm: { bytes: 0, reset: 0 }, btcm: { bytes: 0, reset: 0 } },
          },
        },
      ],
      interrupts: {
        affinities: [0],
        lines: 256,
        distributor: 0x8000000,
        redistributor: 0x8020000,
        stride: 0x20000,
      },
      ram: [{ base: RAM, size: 0x4000 }],
      mappings: [{ core: 0, address: RAM, physical: RAM, size: 0x4000, permissions: 7 }],
      peripherals: [LCD, ADC].map((address) => ({
        core: 0,
        address,
        physical: address,
        size: 0x1000,
      })),
      dma_mappings: [{ address: RAM, physical: RAM, size: 0x4000, permissions: 3 }],
      instructions_per_tick: [1, 1],
    },
    body: {
      kind: 'xp5',
      windows: [
        { base: LCD, size: 0x1000, kind: 'display' },
        { base: ADC, size: 0x1000, kind: 'adc', options: { samples: Array(15).fill(0x456) } },
      ],
      controls: { kind: 'resistor-keys', ...keys },
      displays: [{ base: LCD, period_ns: 3_000_000, raw_formats: ['bgra', null, null, null] }],
    },
    scenario: {
      quantum: 100,
      firmware: {
        size,
        sha256,
        format: 'raw',
        mappings: [
          {
            name: 'synthetic-arm',
            offset: 0,
            size,
            base: RAM,
            encoding: 'raw',
            executable: true,
          },
        ],
      },
    },
  };
}

/** Older generated modules name the key model and nest its options. */
function legacyConfig(config: Record<string, unknown>): Record<string, unknown> {
  const body = config.body as Record<string, unknown>;
  const controls = body.controls as Record<string, unknown>;
  const { kind: _kind, ...options } = controls;
  return {
    ...config,
    body: { ...body, controls: { kind: 'x100vi', resistor_keys: options } },
  };
}

const crcTable = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let value = n;
    for (let bit = 0; bit < 8; bit++) value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
    table[n] = value >>> 0;
  }
  return table;
})();

function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) crc = crcTable[(crc ^ byte) & 0xff]! ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type: string, data: Uint8Array): Uint8Array {
  const chunk = new Uint8Array(12 + data.length);
  const view = new DataView(chunk.buffer);
  view.setUint32(0, data.length);
  chunk.set(new TextEncoder().encode(type), 4);
  chunk.set(data, 8);
  view.setUint32(8 + data.length, crc32(chunk.subarray(4, 8 + data.length)));
  return chunk;
}

export function encodePng(width: number, height: number, rgba: Uint8Array): Uint8Array {
  const stride = width * 4;
  const scanlines = new Uint8Array((stride + 1) * height);
  for (let y = 0; y < height; y++)
    scanlines.set(rgba.subarray(y * stride, (y + 1) * stride), y * (stride + 1) + 1);
  const header = new Uint8Array(13);
  const view = new DataView(header.buffer);
  view.setUint32(0, width);
  view.setUint32(4, height);
  header.set([8, 6, 0, 0, 0], 8);
  const parts = [
    new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', deflateSync(scanlines)),
    chunk('IEND', new Uint8Array()),
  ];
  const png = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    png.set(part, offset);
    offset += part.length;
  }
  return png;
}

export interface EmulatorPhoto {
  width: number;
  height: number;
  png: Uint8Array;
}

/** Boots the generated module, runs the synthetic program to a frame, and
 * returns that frame as PNG bytes. */
export async function renderEmulatorPhoto(
  modulePath = process.env.FUJIMOD_EMULATOR_MODULE ?? emulatorModule,
): Promise<EmulatorPhoto> {
  const module = await loadNodeModule(modulePath);
  const firmware = syntheticFirmware();
  const config = cameraConfig(firmware.sha256, firmware.bytes.length);
  let camera: ReturnType<typeof module.Camera.fromResolved>;
  try {
    camera = module.Camera.fromResolved(JSON.stringify(config), firmware.bytes);
  } catch (failure) {
    if (!/unknown (variant|field)/.test(String(failure))) throw failure;
    camera = module.Camera.fromResolved(JSON.stringify(legacyConfig(config)), firmware.bytes);
  }
  try {
    camera.runUntil(4_500_000n);
    const frame = camera.frame(0);
    if (!frame) throw new Error('The emulator produced no frame.');
    try {
      return {
        width: frame.width,
        height: frame.height,
        png: encodePng(frame.width, frame.height, frame.rgba),
      };
    } finally {
      frame.free();
    }
  } finally {
    camera.close();
  }
}
