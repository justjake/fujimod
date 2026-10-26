/** One shared runtime asset: preserve photographic lighting, bound GPU geometry/texture cost. */
import { NodeIO } from '@gltf-transform/core';
import { KHRMaterialsUnlit } from '@gltf-transform/extensions';
import { compactPrimitive, prune, textureCompress, unlit } from '@gltf-transform/functions';
import { MeshoptSimplifier } from 'meshoptimizer';
import sharp from 'sharp';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
const [source, destination] = process.argv.slice(2);
if (!source || !destination || existsSync(destination))
  throw Error('Expected input glTF and a fresh output GLB');
await MeshoptSimplifier.ready;
const io = new NodeIO().registerExtensions([KHRMaterialsUnlit]);
const document = await io.read(source);
// Preserve texture seams explicitly: position-only permissive collapse tears
// atlas islands and destroys photographed detail even when geometry looks intact.
for (const mesh of document.getRoot().listMeshes()) {
  for (const primitive of mesh.listPrimitives()) {
    const indices = primitive.getIndices();
    const positions = primitive.getAttribute('POSITION');
    const uv = primitive.getAttribute('TEXCOORD_0');
    const indexArray = indices?.getArray(),
      positionArray = positions?.getArray(),
      uvArray = uv?.getArray();
    assert.ok(
      indices && indexArray && positionArray && uvArray,
      'Photogrammetry requires indexed positions and texture coordinates',
    );
    const count = Math.floor((indices.getCount() * 0.18) / 3) * 3;
    const [simplified] = MeshoptSimplifier.simplifyWithAttributes(
      new Uint32Array(indexArray),
      new Float32Array(positionArray),
      3,
      new Float32Array(uvArray),
      2,
      [1, 1],
      null,
      count,
      0.003,
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
await document.transform(
  textureCompress({ encoder: sharp, targetFormat: 'jpeg', resize: [2048, 2048], quality: 85 }),
  unlit(),
  prune({ keepAttributes: true }),
);
await io.write(destination, document);
