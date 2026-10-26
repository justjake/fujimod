import { test } from 'vite-plus/test';
import assert from 'node:assert/strict';
import { featureMediaFiles } from '../scripts/stage-site.ts';
import { featureMedia, validateFeatureMedia } from '../src/home/feature-media.ts';

const manifest = validateFeatureMedia({
  schema: 1,
  cards: {
    shape_aspect_item: {
      video: '1'.repeat(24) + '.mp4',
      still: '2'.repeat(24) + '.webp',
      clips: { '65:24': [1, 2.5] },
    },
  },
});

test('cards show their newest recording', () => {
  assert.equal(
    featureMedia(manifest)('missing', 'shape_aspect_item')?.still,
    '2'.repeat(24) + '.webp',
  );
  assert.equal(featureMedia(null)('shape_aspect_item'), null);
});

test('deployment stages media for the cards the homepage shows', () => {
  assert.deepEqual(
    [...featureMediaFiles(manifest, ['shape_aspect_item', 'image_size'])],
    ['1'.repeat(24) + '.mp4', '2'.repeat(24) + '.webp'],
  );
  assert.deepEqual([...featureMediaFiles(manifest, ['image_size'])], []);
  assert.throws(
    () =>
      validateFeatureMedia({
        ...manifest,
        cards: { x: { video: '../x.mp4', still: '', clips: {} } },
      }),
    /Invalid feature media: x/,
  );
});
