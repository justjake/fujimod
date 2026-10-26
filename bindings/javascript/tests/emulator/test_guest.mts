// Generated ARM programs cover code translation and retirement behavior.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { loadNodeModule } from '../../dist/emulator/node.mjs';
interface RunOptions {
  ratio: number;
  quantum: number;
}
interface BuildOptions extends RunOptions {
  protectedCode: boolean;
  protectStack: boolean;
}
interface Node {
  name: string;
  boot_entries: string[];
  dependencies: string[];
  windows: { base: number; size: number }[];
  model: Record<string, unknown>;
}
const modulePath =
  process.argv[2] ??
  fileURLToPath(new URL('../../../../target/emulator/camera.mjs', import.meta.url));
const hash = (b: Uint8Array) => createHash('sha256').update(b).digest('hex');
function assembler(base: number) {
  const words: number[] = [],
    literals: { at: number; r: number; value: number | string }[] = [],
    labels = new Map<string, number>(),
    branches: { at: number; label: string; condition: number }[] = [];
  const address = (name: string) => {
    const target = labels.get(name);
    assert(target !== undefined, `Missing label ${name}`);
    return target;
  };
  return {
    address,
    emit(...w: number[]) {
      words.push(...w);
    },
    label(name: string) {
      labels.set(name, base + words.length * 4);
    },
    literal(r: number, value: number | string) {
      literals.push({ at: words.length, r, value });
      words.push(0);
    },
    branch(label: string, condition = 0xe) {
      branches.push({ at: words.length, label, condition });
      words.push(0);
    },
    finish() {
      for (const { at, label, condition } of branches) {
        assert(labels.has(label));
        words[at] =
          (condition << 28) | 0x0a000000 | (((address(label) - base) / 4 - at - 2) & 0xffffff);
      }
      const pool = words.length;
      for (let n = 0; n < literals.length; n++) {
        const { at, r, value } = literals[n],
          offset = (pool + n - at - 2) * 4;
        assert(offset >= 0 && offset < 4096);
        words[at] = 0xe59f0000 | (r << 12) | offset;
        words.push(typeof value === 'string' ? address(value) : value);
      }
      return words;
    },
  };
}
function program(scenario: string) {
  const a = assembler(0x10000),
    b = assembler(0x20000);
  if (scenario.startsWith('bulk-')) {
    // ARM STMDB r13!, {r0-r7}: one guest instruction with eight stores.
    // Warm its body before a late partial abort, or straddle two RAM pages.
    const stack = scenario === 'bulk-boundary' ? 0x19010 : 0x19020;
    a.literal(13, stack);
    a.literal(11, 0x18000);
    a.emit(0xe3a0a000);
    for (let r = 0; r < 8; r++) a.literal(r, 0x01020300 + r * 0x01010101);
    a.label('loop');
    if (scenario === 'bulk-unmapped' || scenario === 'bulk-protected') {
      a.literal(12, scenario === 'bulk-unmapped' ? 0x40010 : 0x30010);
      a.emit(0xe35a0040, 0x01a0d00c); // move the stack only on iteration 64
    }
    a.emit(0xe92d00ff, 0xe28dd020, 0xe28aa001, 0xe35a0080);
    a.branch('loop', 1);
    a.emit(0xe58b0000, 0xe58ba004);
    a.label('done');
    a.branch('done');
    const image = new Uint8Array(0x30000),
      view = new DataView(image.buffer);
    a.finish().forEach((word, n) => view.setUint32(n * 4, word, true));
    return image;
  }
  if (scenario.startsWith('late-')) {
    a.literal(1, 0x19000);
    a.emit(0xe3a03000);
    a.label('loop');
    a.emit(0xe2833001, 0xe3530040);
    if (scenario === 'late-unmapped') a.emit(0x03a01000, 0xe5910000);
    else if (scenario === 'late-protected') a.emit(0x05810000);
    else a.emit(0x07f000f0); // conditional architectural UDF after warming
    a.branch('loop');
    const image = new Uint8Array(0x30000),
      view = new DataView(image.buffer);
    a.finish().forEach((word, n) => view.setUint32(n * 4, word, true));
    return image;
  }
  a.literal(4, 0x18000);
  a.emit(0xe3a03000, 0xe3a00000); // output pointer, iteration/result registers
  if (scenario === 'active-smc') {
    a.literal(1, 'add');
    a.literal(2, 0xe2800002);
  }
  a.label('loop');
  a.emit(0xe2833001);
  if (scenario === 'active-smc') a.emit(0xe3530040, 0x05812000); // patch next instruction on iteration 64
  if (scenario === 'due-dma') {
    a.emit(0xe3530040);
    a.branch('transfer', 0);
  }
  a.label('add');
  a.emit(0xe2800001);
  if (scenario === 'active-smc') {
    a.emit(0xe3530080);
    a.branch('loop', 1);
    a.emit(0xe5840000, 0xe5843004);
    a.label('done');
    a.branch('done');
  } else {
    a.emit(0xe5840000, 0xe5843004);
    a.branch('loop');
    if (scenario === 'peer-alias') {
      b.literal(8, 2048);
      b.label('wait');
      b.emit(0xe2588001);
      b.branch('wait', 1);
      b.literal(1, 0x40000000 + a.address('add'));
      b.literal(2, 0xe2800002);
      b.emit(0xe5812000);
      b.label('done');
      b.branch('done');
    } else {
      a.label('transfer');
      for (const [offset, value] of [
        [0, 0x19000],
        [4, a.address('add')],
        [8, 4],
        [0x2c, 0x1555400],
        [0x28, 5],
      ]) {
        a.literal(1, 0xfff02000 + offset);
        a.literal(2, value);
        a.emit(0xe5812000);
      }
      a.branch('add');
    }
  }
  const image = new Uint8Array(0x30000),
    view = new DataView(image.buffer);
  a.finish().forEach((word, n) => view.setUint32(n * 4, word, true));
  if (scenario === 'peer-alias')
    b.finish().forEach((word, n) => view.setUint32(0x10000 + n * 4, word, true));
  if (scenario === 'due-dma')
    for (let n = 0; n < 8; n++) view.setUint32(0x9000 + n * 4, 0xe2800002, true);
  return image;
}
function configuration(
  firmware: Uint8Array,
  cores: number,
  dma: boolean,
  { ratio, quantum, protectedCode, protectStack }: BuildOptions,
) {
  const mapping = {
    address: 0x10000,
    physical: 0x10000,
    size: 0x30000,
    permissions: protectedCode ? 5 : 7,
  };
  const spans = protectStack
    ? [
        { address: 0x10000, physical: 0x10000, size: 0x20000, permissions: 7 },
        { address: 0x30000, physical: 0x30000, size: 0x1000, permissions: 5 },
        { address: 0x31000, physical: 0x31000, size: 0xf000, permissions: 7 },
      ]
    : [mapping];
  const processors = Array.from({ length: cores }, (_, n) => ({
    model: 'cortex-r5f',
    released: true,
    registers: [['Pc', n ? 0x20000 : 0x10000]],
    mprofile: null,
    rprofile: {
      cp14_identity: 0x4003,
      tcm: { atcm: { bytes: 0, reset: 0 }, btcm: { bytes: 0, reset: 0 } },
    },
  }));
  const config = {
    schema: 1,
    firmwareId: 'generated',
    bootSource: 'internal',
    initialPositions: {},
    graph: {
      schema: 1,
      firmwareId: 'generated',
      bootSource: 'internal',
      quantum,
      execution: {
        processors,
        ram: [{ base: 0x10000, size: 0x30000 }],
        mappings: processors.flatMap((_, core) =>
          spans.flatMap((span) => [
            { core, ...span },
            { core, ...span, address: 0x40000000 + span.address },
          ]),
        ),
        dma_mappings: [{ ...mapping, permissions: 3 }],
        decoders: [],
        instructions_per_tick: [ratio, 1],
        peripherals: [{ core: 0, address: 0xfffb3000, physical: 0xfffb3000, size: 4096 }],
      },
      devices: {
        schema: 1,
        boot_entry: 'loader',
        wires: {} as Record<string, string>,
        nodes: [
          {
            name: 'irq',
            boot_entries: ['loader'],
            dependencies: [],
            windows: [{ base: 0xfffb3000, size: 4096 }],
            model: {
              kind: 'exr2-interrupt-controller',
              options: { arbitration: 'unique' },
              core: 0,
              base: 0xfffb3000,
              routes: null,
              software: [],
            },
          },
        ] as Node[],
      },
      scene: {},
      assets: {},
      firmware: {
        size: firmware.length,
        sha256: hash(firmware),
        format: 'raw',
        mappings: [
          {
            name: 'program',
            base: 0x10000,
            offset: 0,
            size: firmware.length,
            encoding: 'raw',
            executable: true,
          },
        ],
      },
    },
  };
  if (dma) {
    config.graph.devices.wires.routing = 'irq-routes';
    config.graph.devices.nodes[0].model.routes = 'routing';
    config.graph.execution.peripherals.push({
      core: 0,
      address: 0xfff02000,
      physical: 0xfff02000,
      size: 4096,
    });
    config.graph.devices.nodes.push({
      name: 'dma',
      boot_entries: ['loader'],
      dependencies: [],
      windows: [
        { base: 0xfff02000, size: 64 },
        { base: 0xfff02300, size: 4 },
      ],
      model: { kind: 'memory-dma', options: {}, base: 0xfff02000, routes: 'routing' },
    });
  }
  return config;
}
async function run(scenario: string, jit: boolean, options: RunOptions) {
  const fault =
    scenario.startsWith('late-') || scenario === 'bulk-unmapped' || scenario === 'bulk-protected';
  const module = await loadNodeModule(modulePath, {
    executionPolicy: 'exact',
    tcgJit: jit ? { threshold: 2, batchSize: 2 } : false,
  });
  const image = program(scenario),
    json = JSON.stringify(
      configuration(image, scenario === 'peer-alias' ? 2 : 1, scenario === 'due-dma', {
        ...options,
        protectedCode: scenario === 'late-protected',
        protectStack: scenario === 'bulk-protected',
      }),
    );
  const camera = module.Camera.fromGraph(json, image);
  let executionError: string | null = null;
  const advance = (tick: number, fails = false) => {
    try {
      camera.runUntil(BigInt(tick));
      assert(!fails, 'the guest did not fault');
    } catch (error) {
      if (!fails) throw error;
      executionError = (error as Error).message;
    }
  };
  const word = (address: number) =>
    new DataView(camera.memoryRead(BigInt(address), 4).buffer).getUint32(0, true);
  try {
    advance(options.ratio === 1 ? 100 : 2);
    const saved = camera.checkpoint();
    advance(12000 / options.ratio, fault);
    const result = {
      error: executionError,
      fingerprint: camera.fingerprint(),
      report: camera.errorReport().report,
      output: word(0x18000),
      iterations: word(0x18004),
    };
    if (scenario === 'active-smc') {
      assert.equal(result.output, 193);
      assert.equal(result.iterations, 128);
    } else if (scenario.startsWith('bulk-')) {
      if (!fault) {
        assert.equal(result.output, 0x01020300);
        assert.equal(result.iterations, 128);
      }
      const first =
        scenario === 'bulk-unmapped'
          ? 0x3fff0
          : scenario === 'bulk-protected'
            ? 0x2fff0
            : scenario === 'bulk-boundary'
              ? 0x18ff0
              : 0x19000;
      for (let r = 0; r < (fault ? 4 : 8); r++)
        assert.equal(
          word(first + r * 4),
          0x01020300 + r * 0x01010101,
          'STM store order / partial abort',
        );
    } else if (!fault)
      assert(result.output > result.iterations, 'the peer code write did not replace the add');
    camera.restore(saved.id);
    advance(12000 / options.ratio, fault);
    assert.deepEqual(camera.fingerprint(), result.fingerprint);
    const stats = module.jitStatistics();
    if (jit) {
      assert(stats.enabled);
      assert.equal(stats.failures, 0, stats.lastError ?? 'JIT failed');
      assert(BigInt(stats.compiledExecutions) > 0n);
      if (!fault && !scenario.startsWith('bulk-')) assert(stats.invalidations > 0);
      assert(stats.flushes > 0);
      assert(BigInt(stats.chainedExecutions) > 0n);
    }
    console.log(
      JSON.stringify({
        scenario,
        jit,
        ...options,
        output: result.output,
        iterations: result.iterations,
        stats,
      }),
    );
    return result;
  } finally {
    camera.close();
  }
}
for (const options of [
  { ratio: 1, quantum: 100 },
  { ratio: 24, quantum: 17 },
])
  for (const scenario of [
    'active-smc',
    'peer-alias',
    'due-dma',
    'late-unmapped',
    'late-protected',
    'late-undefined',
    'bulk-stores',
    'bulk-boundary',
    'bulk-unmapped',
    'bulk-protected',
  ])
    assert.deepEqual(await run(scenario, false, options), await run(scenario, true, options));
console.log(
  JSON.stringify({
    passed: true,
    activeSelfModifyingCode: true,
    peerAliasCodeWrite: true,
    dueDmaCodeWrite: true,
    restoreCodePages: true,
    compiledFaults: true,
    bulkStores: true,
    partialStoreAborts: true,
  }),
);
