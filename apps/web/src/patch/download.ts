import type { PatchFiles } from './patcher.ts';
import { zipBlob } from '@fujimod/emulator-ui/archive';

export function cardZip({ ffboot, ffsys }: PatchFiles): Promise<Blob> {
  return zipBlob([
    { path: 'ffboot.bin', data: new Blob([ffboot]) },
    { path: 'ffsys.bin', data: new Blob([ffsys]) },
  ]);
}
