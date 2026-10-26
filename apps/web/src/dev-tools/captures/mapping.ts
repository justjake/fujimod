/** Words that follow one held control across released, held, released, held captures. */
export interface ControlDiff {
  /** Addresses equal in both released and both held captures, but different between them. */
  candidates: Uint32Array;
  /** Every word that differs between any two captures: address, then its four values. */
  changed: Uint32Array;
  /** Changed words beyond `limit` that were counted but not kept. */
  omitted: number;
}

export function controlDiff(
  base: number,
  captures: readonly [Uint8Array, Uint8Array, Uint8Array, Uint8Array],
  limit = 1 << 20,
): ControlDiff {
  const length = captures[0].length;
  if (length % 4 || captures.some((capture) => capture.length !== length))
    throw Error('Mapping captures must cover the same word-aligned range');
  const [r1, h1, r2, h2] = captures.map(
    (capture) => new Uint32Array(capture.buffer, capture.byteOffset, length / 4),
  );
  const candidates: number[] = [];
  const changed: number[] = [];
  let omitted = 0;
  for (let index = 0; index < r1.length; index++) {
    const a = r1[index],
      b = h1[index],
      c = r2[index],
      d = h2[index];
    if (a === b && b === c && c === d) continue;
    const address = base + index * 4;
    if (a === c && b === d) candidates.push(address);
    if (changed.length / 5 < limit) changed.push(address, a, b, c, d);
    else omitted++;
  }
  return {
    candidates: Uint32Array.from(candidates),
    changed: Uint32Array.from(changed),
    omitted,
  };
}
