import { readFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import { patch } from '../src/patch/patcher.ts';
const [base, compiled, expected] = process.argv.slice(2);
const outputs = await patch(await readFile(base), await readFile(compiled));
for (const name of ['ffboot', 'ffsys'] as const)
  assert.deepEqual(Buffer.from(outputs[name]), await readFile(`${expected}/${name}.bin`));
console.log(`JavaScript/Python byte-for-byte match: ${outputs.metadata.firmware}`);
