import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import { patch } from '../src/patch/patcher.ts';
const [base, compiled, expected, updateFilename] = process.argv.slice(2);
const outputs = await patch(await readFile(base), await readFile(compiled));
for (const name of ['ffboot', 'ffsys'] as const)
  assert.deepEqual(Buffer.from(outputs[name]), await readFile(`${expected}/${name}.bin`));
if (outputs.fpupdate) {
  assert.ok(updateFilename, 'An installable patch needs the update file name.');
  assert.deepEqual(Buffer.from(outputs.fpupdate), await readFile(`${expected}/${updateFilename}`));
}
console.log(`JavaScript/Python byte-for-byte match: ${outputs.metadata.firmware}`);
