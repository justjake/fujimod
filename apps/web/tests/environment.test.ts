interface Gltf {
  meshes: { primitives: { attributes: Record<string, number>; indices: number }[] }[];
  accessors: { count: number }[];
  images: { mimeType: string; bufferView: number }[];
  bufferViews: { byteOffset?: number; byteLength: number }[];
  nodes: { name: string }[];
}
import { test } from 'vite-plus/test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';

test('the installed station has matching provenance and bounded geometry and texture costs', async () => {
  const root = new URL(
    '../../../crates/simulation/environments/crouch-end-station/',
    import.meta.url,
  );
  const manifest = JSON.parse(await readFile(new URL('environment.json', root), 'utf8'));
  const bytes = await readFile(new URL('assets/scene.glb', root));
  assert.equal(bytes.readUInt32LE(0), 0x46546c67, 'Pull the curated GLB through Git LFS');
  assert.ok(bytes.length < 20 * 1024 * 1024);
  assert.equal(createHash('sha256').update(bytes).digest('hex'), manifest.sha256);
  const jsonSize = bytes.readUInt32LE(12);
  const gltf: Gltf = JSON.parse(bytes.subarray(20, 20 + jsonSize).toString());
  assert.ok(
    gltf.meshes.flatMap((m) => m.primitives).every((p) => p.attributes.NORMAL !== undefined),
    'Retain normals to avoid loader-side vertex expansion',
  );
  const triangles = gltf.meshes
    .flatMap((m) => m.primitives)
    .reduce((n, p) => n + gltf.accessors[p.indices].count / 3, 0);
  assert.equal(triangles, manifest.preparation.runtime_faces);
  assert.ok(triangles < 400000);
  for (const image of gltf.images) {
    assert.equal(image.mimeType, 'image/jpeg');
    const view = gltf.bufferViews[image.bufferView];
    const jpeg = bytes.subarray(
      28 + jsonSize + (view.byteOffset ?? 0),
      28 + jsonSize + (view.byteOffset ?? 0) + view.byteLength,
    );
    let dimensions;
    for (let offset = 2; offset < jpeg.length;) {
      assert.equal(jpeg[offset], 0xff);
      const marker = jpeg[offset + 1];
      if ([0xc0, 0xc1, 0xc2].includes(marker)) {
        dimensions = [jpeg.readUInt16BE(offset + 5), jpeg.readUInt16BE(offset + 7)];
        break;
      }
      offset += 2 + jpeg.readUInt16BE(offset + 2);
    }
    assert.ok(dimensions, 'JPEG must declare its dimensions');
    assert.ok(dimensions.every((d) => d <= 2048));
  }
});

test('the camera fits its geometry budget and retains every authored control and dial face', async () => {
  const bytes = await readFile(
    new URL('../../../crates/simulation/assets/x70.glb', import.meta.url),
  );
  assert.ok(bytes.length < 5 * 1024 * 1024);
  const gltf: Gltf = JSON.parse(bytes.subarray(20, 20 + bytes.readUInt32LE(12)).toString());
  const parts = JSON.parse(
    await readFile(new URL('../devices/x70/parts.json', import.meta.url), 'utf8'),
  );
  const names = new Set(gltf.nodes.map((node) => node.name));
  const required = [
    parts.screen,
    ...Object.values(parts.inputs).flatMap((input) =>
      Array.isArray(input) ? input : (input as { nodes: string[] }).nodes,
    ),
    ...Object.values(parts.rotaries).flat(),
  ];
  for (const name of required) assert.ok(names.has(name), `Missing camera node: ${name}`);
  const vertices = gltf.meshes
    .flatMap((mesh) => mesh.primitives)
    .reduce((count, primitive) => count + gltf.accessors[primitive.attributes.POSITION].count, 0);
  assert.ok(vertices < 130000);
});
