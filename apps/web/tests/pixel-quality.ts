export function pixelQuality(
  expected: Uint8ClampedArray,
  actual: Uint8ClampedArray,
  width: number,
  area: { left: number; top: number; right: number; bottom: number },
) {
  if (
    !Number.isInteger(width) ||
    width <= 0 ||
    expected.length !== actual.length ||
    expected.length % (width * 4) !== 0 ||
    !Object.values(area).every(Number.isInteger) ||
    area.left < 0 ||
    area.top < 0 ||
    area.right > width ||
    area.bottom > expected.length / (width * 4) ||
    area.right <= area.left ||
    area.bottom <= area.top
  )
    throw Error('Pixel comparison requires matching RGBA planes and a nonempty bounded area.');
  const sums = Array.from({ length: 3 }, () => [0, 0, 0, 0, 0, 0]);
  const count = (area.right - area.left) * (area.bottom - area.top);
  for (let y = area.top; y < area.bottom; y++)
    for (let x = area.left; x < area.right; x++)
      for (let c = 0; c < 3; c++) {
        const at = (y * width + x) * 4 + c,
          a = expected[at]!,
          b = actual[at]!,
          v = sums[c]!;
        v[0] += Math.abs(a - b);
        v[1] += a;
        v[2] += b;
        v[3] += a * a;
        v[4] += b * b;
        v[5] += a * b;
      }
  const expectedVariance = sums.map((v) => Math.max(0, v[3]! / count - (v[1]! / count) ** 2));
  const actualVariance = sums.map((v) => Math.max(0, v[4]! / count - (v[2]! / count) ** 2));
  return {
    mae: sums.map((v) => v[0]! / count),
    expectedVariance,
    actualVariance,
    correlation: sums.map((v, c) => {
      const variance = expectedVariance[c]! * actualVariance[c]!;
      // A flat photo cannot qualify scene content, even when both planes agree.
      if (variance === 0) return 0;
      return (v[5]! / count - (v[1]! / count) * (v[2]! / count)) / Math.sqrt(variance);
    }),
  };
}
