/** FJMODP01 interpreter. No camera/model maps, network access or dependencies. */
export const MAGIC = 'FJMODP01';
export const names = ['ffboot', 'ffsys', 'fpupdate'] as const;
export type OutputName = (typeof names)[number];
export type BinaryInput = Uint8Array | ArrayBuffer | { arrayBuffer(): Promise<ArrayBuffer> };
export interface FileIdentity {
  bytes: number;
  sha256: string;
}
export interface PatchMetadata {
  schema: number;
  source: FileIdentity;
  outputs: Record<'ffboot' | 'ffsys', FileIdentity> & { fpupdate?: FileIdentity };
  commands: number;
  program_sha256: string;
  firmware?: string;
  release?: string;
  mods?: string[];
}
export interface PatchFiles {
  ffboot: Uint8Array<ArrayBuffer>;
  ffsys: Uint8Array<ArrayBuffer>;
}
export interface PatchResult extends PatchFiles {
  fpupdate?: Uint8Array<ArrayBuffer>;
  metadata: PatchMetadata;
}
const fail = (message: string): never => {
  throw new Error(message);
};
const hashPattern = /^[0-9a-f]{64}$/;
const integer = (value: unknown, max: number) =>
  typeof value === 'number' && Number.isInteger(value) && value > 0 && value <= max;
export async function bytes(value: BinaryInput): Promise<Uint8Array<ArrayBuffer>> {
  if (value instanceof Uint8Array)
    return value.buffer instanceof ArrayBuffer
      ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength)
      : Uint8Array.from(value);
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if ('arrayBuffer' in value && typeof value.arrayBuffer === 'function')
    return new Uint8Array(await value.arrayBuffer());
  return fail('Expected a File, Blob, ArrayBuffer or Uint8Array.');
}
export async function sha256(value: BinaryInput) {
  const digest = await crypto.subtle.digest('SHA-256', await bytes(value));
  return Array.from(new Uint8Array(digest), (n) => n.toString(16).padStart(2, '0')).join('');
}
export function inspectPatch(input: Uint8Array) {
  const data = input;
  if (
    !(data instanceof Uint8Array) ||
    data.length < 12 ||
    new TextDecoder().decode(data.subarray(0, 8)) !== MAGIC
  )
    return fail('This is not a supported Fujimod patch.');
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const length = view.getUint32(8, true);
  if (!integer(length, 1048576) || length + 12 > data.length)
    return fail('Invalid patch metadata length.');
  const meta: PatchMetadata = JSON.parse(
    new TextDecoder('utf-8', { fatal: true }).decode(data.subarray(12, 12 + length)),
  );
  if (
    meta.schema !== 1 ||
    !integer(meta.source?.bytes, 256 * 1024 * 1024) ||
    !hashPattern.test(meta.source.sha256) ||
    !integer(meta.commands, 1000000) ||
    !hashPattern.test(meta.program_sha256)
  )
    return fail('Invalid patch metadata.');
  const outputNames = names.filter(
    (name) => name !== 'fpupdate' || meta.outputs?.fpupdate !== undefined,
  );
  for (const name of outputNames)
    if (
      !integer(meta.outputs?.[name]?.bytes, 128 * 1024 * 1024) ||
      !hashPattern.test(meta.outputs[name]!.sha256)
    )
      return fail('Invalid patch output metadata.');
  return { meta, outputNames, program: data.subarray(12 + length) };
}
function decodeLZ(data: Uint8Array, length: number): Uint8Array<ArrayBuffer> {
  if (data.length < 20) return fail('Truncated LZ header.');
  const header = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const packed = header.getUint32(4, true);
  if (
    header.getUint32(0, true) !== length ||
    packed !== data.length ||
    packed < 20 ||
    header.getUint32(12, true) !== 0x4000 ||
    header.getUint32(16, true) !== 4 ||
    header.getUint32(8, true) !== Math.ceil(packed / 0x4000)
  )
    return fail('Invalid LZ stream header.');
  const output = new Uint8Array(length);
  let at = 20,
    written = 0;
  while (at < packed) {
    const command = data[at++];
    if (command < 128) {
      if (!command || at + command > packed || written + command > length)
        return fail('Invalid LZ literal.');
      output.set(data.subarray(at, at + command), written);
      at += command;
      written += command;
    } else {
      if (at >= packed) return fail('Truncated LZ match.');
      const second = data[at++],
        distance = ((command & 127) << 4) | (second >> 4),
        count = second & 15;
      if (!distance || distance > written || !count || written + count > length)
        return fail('Invalid LZ match.');
      for (let n = 0; n < count; n++) {
        output[written] = output[written - distance];
        written++;
      }
    }
  }
  if (written !== length) return fail('LZ output length mismatch.');
  return output;
}
/** async patch(baseStockDatFile, compiledPatch) -> {ffboot, ffsys, fpupdate?, metadata}. */
export async function patch(
  baseStockDatFile: BinaryInput,
  compiledPatch: BinaryInput,
): Promise<PatchResult> {
  const [base, data] = await Promise.all([bytes(baseStockDatFile), bytes(compiledPatch)]);
  const { meta, outputNames, program } = inspectPatch(data);
  if (base.length !== meta.source.bytes || (await sha256(base)) !== meta.source.sha256)
    return fail('Stock DAT SHA-256 does not match this patch.');
  if ((await sha256(program)) !== meta.program_sha256) return fail('Patch program hash mismatch.');
  const outputs = outputNames.map((name) => new Uint8Array(meta.outputs[name]!.bytes));
  const view = new DataView(program.buffer, program.byteOffset, program.byteLength);
  let cursor = 0;
  for (let n = 0; n < meta.commands; n++) {
    if (cursor + 20 > program.length) return fail('Truncated patch command.');
    const op = program[cursor],
      target = program[cursor + 1],
      flags = program[cursor + 2],
      reserved = program[cursor + 3];
    const dest = view.getUint32(cursor + 4, true),
      length = view.getUint32(cursor + 8, true);
    const source = view.getUint32(cursor + 12, true),
      packed = view.getUint32(cursor + 16, true);
    cursor += 20;
    if (
      target >= outputs.length ||
      flags > 1 ||
      reserved ||
      !length ||
      dest + length > outputs[target].length
    )
      return fail('Invalid patch command bounds or flags.');
    let chunk;
    if (op === 1 || op === 2) {
      if (!packed || source + packed > base.length || (op === 1 && packed !== length))
        return fail('Invalid patch source bounds.');
      chunk = base.subarray(source, source + packed);
      if (flags) chunk = chunk.map((value) => value ^ 255);
      if (op === 2) chunk = decodeLZ(chunk, length);
    } else if (op === 3) {
      if (flags || source || packed || cursor + length > program.length)
        return fail('Invalid patch literal.');
      chunk = program.subarray(cursor, cursor + length);
      cursor += length;
    } else return fail('Unknown patch opcode.');
    outputs[target].set(chunk, dest);
  }
  if (cursor !== program.length) return fail('Trailing patch bytes.');
  for (let n = 0; n < outputNames.length; n++)
    if ((await sha256(outputs[n])) !== meta.outputs[outputNames[n]]!.sha256)
      return fail('Patch output hash mismatch.');
  return { ffboot: outputs[0], ffsys: outputs[1], fpupdate: outputs[2], metadata: meta };
}
