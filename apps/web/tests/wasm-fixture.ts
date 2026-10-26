import type { EmulatorManifest } from 'fujimod/emulator';

/** Generated ARM instructions program LCDC, poll an ADC ladder and store pixels. */
export async function wasmFixture(fault = false) {
  const LCD = 0xff990000,
    ADC = 0xff906000,
    RAM = 0x10000,
    PIXELS = 0x12000;
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
  ])
    write(LCD + offset!, value!);
  constant(1, ADC + 0x10 + 11 * 4);
  constant(3, PIXELS);
  words.push(0xe5912000, 0xe38224ff, 0xe5832000, 0xe5832004, 0xe5832008, 0xe583200c, 0xeafffff8);
  if (fault) words.splice(0, words.length, 0xe5910000); // ldr r0,[r1], r1 initially zero
  const dat = new Uint8Array(words.length * 4);
  words.forEach((word, i) => new DataView(dat.buffer).setUint32(i * 4, word, true));
  for (let i = 0; i < dat.length; i++) dat[i] ^= 255; // DAT raw-stream inversion
  const sha256 = [...new Uint8Array(await crypto.subtle.digest('SHA-256', dat))]
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('');
  const manifest: EmulatorManifest = {
    schema: 1,
    abi: 1,
    executionPolicy: 'exact',
    module: 'camera.mjs',
    wasm: 'camera.wasm',
    construction: 'resolved',
    ticksPerSecond: 1_000_000_000,
    bootTicks: 0,
    output: 0,
    session: {
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
        peripherals: fault
          ? []
          : [LCD, ADC].map((address) => ({ core: 0, address, physical: address, size: 0x1000 })),
        dma_mappings: [{ address: RAM, physical: RAM, size: 0x4000, permissions: 3 }],
        instructions_per_tick: [1, 1],
      },
      body: fault
        ? { kind: 'bare' }
        : {
            kind: 'xp5',
            windows: [
              { base: LCD, size: 0x1000, kind: 'display' },
              { base: ADC, size: 0x1000, kind: 'adc', options: { samples: Array(15).fill(0x456) } },
            ],
            controls: {
              kind: 'x100vi',
              resistor_keys: {
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
                detent_return_ns: 60000000,
                shutter_half_delay_ns: 30000000,
              },
            },
            displays: [
              { base: LCD, period_ns: 3_000_000, raw_formats: ['bgra', null, null, null] },
            ],
          },
      scenario: {
        quantum: 100,
        firmware: {
          size: dat.length,
          sha256,
          format: 'dat',
          mappings: [
            {
              name: 'generated-arm',
              offset: 0,
              size: dat.length,
              base: RAM,
              encoding: 'raw',
              executable: true,
            },
          ],
        },
      },
    },
  };
  return {
    dat,
    manifest,
    inputs: { inputs: [{ id: 'MENU', label: 'Menu', kind: 'button', supported: true }] },
  };
}
