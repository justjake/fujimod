import { expect, test } from 'vite-plus/test';
import { bitmap, composeScreen } from './screen';

test('the saved planes composite as the display stacks them', () => {
  // Two pixels of video: black, and a saturated red pair member sharing chroma.
  const files: Record<string, Uint8Array> = {
    'screen/video.yuv': Uint8Array.of(16, 81, 90, 240),
    // A B G R: opaque white over the second pixel at full global alpha.
    'screen/layer0.abgr': Uint8Array.of(255, 255, 255, 255),
  };
  const image = composeScreen(
    {
      video: 'screen/video.yuv',
      width: 2,
      height: 1,
      layers: [{ path: 'screen/layer0.abgr', x: 1, width: 1, height: 1, alpha: 128 }],
      rectangles: [{ right: 1, thickness: 1, luma: 0, opacity: 64 }],
    },
    (path) => files[path],
  )!;
  // Pixel 0: Y16 with Cb -38, Cr +112, then half-covered by a black rectangle.
  expect([...image.subarray(0, 3)]).toEqual([87, 0, 0]);
  // Pixel 1: the opaque white layer, then the same rectangle.
  expect([...image.subarray(3, 6)]).toEqual([128, 128, 128]);
});

test('a plane of the wrong size shows no screen', () => {
  expect(
    composeScreen({ video: 'v', width: 2, height: 1 }, () => Uint8Array.of(1, 2, 3)),
  ).toBeUndefined();
});

test('bitmaps are top-down 24-bit BGR', () => {
  const out = bitmap(Uint8Array.of(1, 2, 3, 4, 5, 6), 2, 1);
  expect([...out.subarray(0, 2)]).toEqual([0x42, 0x4d]);
  expect(new DataView(out.buffer).getInt32(22, true)).toBe(-1);
  expect([...out.subarray(54, 60)]).toEqual([3, 2, 1, 6, 5, 4]);
});
