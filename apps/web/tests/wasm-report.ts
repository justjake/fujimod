/** Read the first store-only report ZIP member produced by Engine.errorReport. */
export async function engineReport(blob: Blob): Promise<Record<string, unknown>> {
  const buffer = await blob.arrayBuffer(),
    view = new DataView(buffer);
  if (view.getUint32(0, true) !== 0x04034b50 || view.getUint16(8, true) !== 0)
    throw Error('Unexpected report ZIP format.');
  const length = view.getUint32(18, true),
    nameLength = view.getUint16(26, true),
    extra = view.getUint16(28, true);
  const name = new TextDecoder().decode(new Uint8Array(buffer, 30, nameLength));
  if (name !== 'report.json') throw Error('Missing report.json.');
  return JSON.parse(
    new TextDecoder().decode(new Uint8Array(buffer, 30 + nameLength + extra, length)),
  ) as Record<string, unknown>;
}
