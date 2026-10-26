/** CDP sample weights are microseconds; inclusive totals count each function once per stack. */
export interface CpuProfile {
  nodes: { id: number; callFrame: { functionName: string; url: string }; children?: number[] }[];
  samples: number[];
  timeDeltas: number[];
}
export function summarizeProfile(profile: CpuProfile) {
  if (profile.samples.length !== profile.timeDeltas.length)
    throw Error('CPU profile sample and time-delta lengths differ.');
  const nodes = new Map(profile.nodes.map((node) => [node.id, node]));
  const parents = new Map(
    profile.nodes.flatMap((node) =>
      (node.children ?? []).map((child) => [child, node.id] as const),
    ),
  );
  const self = new Map<string, number>();
  const inclusive = new Map<string, number>();
  const categories = new Map<string, number>();
  let total = 0;
  for (let i = 0; i < profile.samples.length; i++) {
    const id = profile.samples[i]!;
    const delta = profile.timeDeltas[i]!;
    const node = nodes.get(id);
    if (!node) throw Error(`CPU profile refers to absent node ${id}.`);
    total += delta;
    const { functionName: name, url } = node.callFrame;
    self.set(name, (self.get(name) ?? 0) + delta);
    const category =
      name === '(idle)'
        ? 'idle'
        : url.startsWith('wasm://')
          ? 'generated Wasm'
          : url.includes('camera.wasm')
            ? 'emulator Wasm'
            : ['(program)', '(garbage collector)'].includes(name)
              ? name
              : 'JavaScript and glue';
    categories.set(category, (categories.get(category) ?? 0) + delta);
    const stack = new Set<string>();
    let current: number | undefined = id;
    while (current !== undefined) {
      const frame = nodes.get(current);
      if (!frame) throw Error(`CPU profile refers to absent ancestor ${current}.`);
      stack.add(frame.callFrame.functionName);
      current = parents.get(current);
    }
    for (const functionName of stack)
      inclusive.set(functionName, (inclusive.get(functionName) ?? 0) + delta);
  }
  const rows = (counts: Map<string, number>) =>
    [...counts]
      .sort((a, b) => b[1] - a[1])
      .map(([name, microseconds]) => ({
        name,
        seconds: microseconds / 1e6,
        percent: (100 * microseconds) / total,
      }));
  return {
    sampledSeconds: total / 1e6,
    samples: profile.samples.length,
    categories: rows(categories),
    self: rows(self).slice(0, 50),
    inclusive: rows(inclusive).slice(0, 50),
  };
}
