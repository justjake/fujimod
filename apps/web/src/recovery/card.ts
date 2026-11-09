import { sdCardTools } from 'fujimod';

/** One file for an SD card utility. */
export interface SdCardUtilityFile {
  name: string;
  data: Uint8Array<ArrayBuffer>;
}

export type SdCardUtilityDownload = 'system' | 'green-led';

/** Build SD card recovery or green LED files from an official update. */
export function buildSdCardUtility(
  firmwareId: string,
  dat: Uint8Array,
  kind: SdCardUtilityDownload,
): SdCardUtilityFile[] {
  const card =
    kind === 'green-led'
      ? sdCardTools.buildGreenLed(firmwareId, dat)
      : sdCardTools.build(firmwareId, dat);
  try {
    return card.takeFiles().map((file) => {
      try {
        return { name: file.name, data: new Uint8Array(file.bytes) };
      } finally {
        file.free();
      }
    });
  } finally {
    card.free();
  }
}
