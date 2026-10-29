/** Read the first report ZIP member produced by Engine.errorReport. */
export async function engineReport(blob: Blob): Promise<Record<string, unknown>> {
  const buffer = await blob.arrayBuffer(),
    view = new DataView(buffer);
  if (view.getUint32(0, true) !== 0x04034b50)
    throw Error('Unexpected report ZIP format.');
  const method = view.getUint16(8, true),
    compressedLength = view.getUint32(18, true),
    length = view.getUint32(22, true),
    nameLength = view.getUint16(26, true),
    extra = view.getUint16(28, true);
  const name = new TextDecoder().decode(new Uint8Array(buffer, 30, nameLength));
  if (name !== 'report.json') throw Error('Missing report.json.');
  const compressed = new Uint8Array(buffer, 30 + nameLength + extra, compressedLength);
  const bytes =
    method === 0
      ? compressed
      : method === 8
        ? new Uint8Array(
            await new Response(
              new Blob([compressed]).stream().pipeThrough(new DecompressionStream('deflate-raw')),
            ).arrayBuffer(),
          )
        : undefined;
  if (!bytes || bytes.byteLength !== length) throw Error('Unsupported report ZIP compression.');
  return JSON.parse(
    new TextDecoder().decode(bytes),
  ) as Record<string, unknown>;
}
