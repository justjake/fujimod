import type { Firmware } from '../catalog';
import { sha256 } from '../patch/patcher';
import { firmwareMirrors } from './firmware-mirrors';
import { sharingUrl } from '../sharing-server';

/** Read the mirror once and validate before importing into browser storage. */
export async function fetchFirmware(
  firmware: Firmware,
  progress?: (received: number) => void,
): Promise<File> {
  const source = firmwareMirrors[firmware.id];
  if (!source) throw Error('No download mirror is available for this firmware.');
  const response = await fetch(sharingUrl(`/proxy?url=${encodeURIComponent(source)}`));
  if (!response.ok || !response.body) throw Error(`Firmware download failed (${response.status}).`);
  const bytes = new Uint8Array(firmware.bytes);
  let received = 0;
  for await (const chunk of response.body) {
    if (received + chunk.byteLength > bytes.byteLength)
      throw Error('The downloaded firmware failed its integrity check.');
    bytes.set(chunk, received);
    received += chunk.byteLength;
    progress?.(received);
  }
  if (received !== firmware.bytes || (await sha256(bytes)) !== firmware.sha256)
    throw Error('The downloaded firmware failed its integrity check.');
  return new File([bytes], new URL(source).pathname.split('/').pop()!, {
    type: 'application/octet-stream',
  });
}
