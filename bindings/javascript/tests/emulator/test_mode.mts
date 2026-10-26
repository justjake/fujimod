// Runtime tests select an explicit Emscripten module creation option.
import assert from 'node:assert/strict';
import type {
  EmulatorModule,
  JitOptions,
  JitStatistics,
  ModuleOptions,
} from '../../dist/emulator/index.js';
export function moduleOptions(options: boolean | JitOptions = true): ModuleOptions {
  const policy = process.env.FUJIMOD_EXECUTION_POLICY ?? 'exact';
  assert(policy === 'interactive' || policy === 'exact');
  return {
    executionPolicy: policy,
    ...(process.env.FUJIMOD_TCG_JIT === '0'
      ? { tcgJit: false }
      : process.env.FUJIMOD_TCG_JIT === '1'
        ? { tcgJit: options }
        : {}),
  };
}
export function verifyJitStats(stats: JitStatistics) {
  if (process.env.FUJIMOD_TCG_JIT === '0') {
    assert.equal(stats.enabled, false);
    return stats;
  }
  assert(stats.enabled, 'the generated-code tier was not enabled');
  assert.equal(stats.failures, 0, stats.lastError ?? 'unexpected JIT failure');
  if (process.env.FUJIMOD_TCG_JIT === '1') {
    assert(BigInt(stats.compiledExecutions) > 0n, 'no generated translation block executed');
  }
  return stats;
}
export function finishJit(module: EmulatorModule) {
  console.error(JSON.stringify({ jit: verifyJitStats(module.jitStatistics()) }));
}
