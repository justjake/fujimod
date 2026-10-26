/** Reduce authored camera geometry without changing node names, pivots or materials. */
import { NodeIO } from '@gltf-transform/core';
import { ALL_EXTENSIONS } from '@gltf-transform/extensions';
import { compactPrimitive, prune } from '@gltf-transform/functions';
import { MeshoptSimplifier } from 'meshoptimizer';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';

const [source, destination] = process.argv.slice(2);
if (!source || !destination || existsSync(destination))
  throw Error('Expected input GLB and a fresh output GLB');
await MeshoptSimplifier.ready;
const io = new NodeIO().registerExtensions(ALL_EXTENSIONS);
const document = await io.read(source);
for (const mesh of document.getRoot().listMeshes()) {
  for (const primitive of mesh.listPrimitives()) {
    const indices = primitive.getIndices();
    const positions = primitive.getAttribute('POSITION');
    if (!indices || indices.getCount() < 300) continue;
    const indexArray = indices.getArray(),
      positionArray = positions?.getArray();
    assert.ok(indexArray && positionArray, 'Indexed camera geometry requires position data');
    // A tight geometric error preserves the small controls and silhouette.
    // Simplification stays within each primitive: labels never merge into dials.
    const [simplified] = MeshoptSimplifier.simplify(
      new Uint32Array(indexArray),
      new Float32Array(positionArray),
      3,
      Math.floor((indices.getCount() * 0.15) / 3) * 3,
      0.001,
      ['Permissive'],
    );
    primitive.setIndices(
      document
        .createAccessor()
        .setType('SCALAR')
        .setArray(Uint32Array.from(simplified))
        .setBuffer(indices.getBuffer()),
    );
    compactPrimitive(primitive);
  }
}
await document.transform(prune({ keepAttributes: true, keepLeaves: true }));
await io.write(destination, document);
