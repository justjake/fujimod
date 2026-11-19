import type { Firmware } from '../catalog';
import { sha256 } from '../patch/patcher';
import { firmwareMirrors } from './firmware-mirrors';

/** Read the mirror once and validate before importing into browser storage. */
export async function fetchFirmware(firmware: Firmware): Promise<File> {
  const source = firmwareMirrors[firmware.id];
  if (!source) throw Error('No download mirror is available for this firmware.');
  const response = await fetch(`https://share.fujimod.com/proxy?url=${encodeURIComponent(source)}`);
  if (!response.ok) throw Error(`Firmware download failed (${response.status}).`);
  const bytes = await response.arrayBuffer();
  if (bytes.byteLength !== firmware.bytes || (await sha256(bytes)) !== firmware.sha256)
    throw Error('The downloaded firmware failed its integrity check.');
  return new File([bytes], new URL(source).pathname.split('/').pop()!, {
    type: 'application/octet-stream',
  });
}
