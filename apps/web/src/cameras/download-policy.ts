import type { PatchFirmware } from '../patch/catalog';

/** Firmware artifacts on an emulated card follow the selected firmware's release switches. */
export function cardDownloadAllowed(
  name: string,
  firmware?: Pick<PatchFirmware, 'downloads' | 'update_filename'>,
) {
  if (/^ff(?:boot|sys)\.bin$/i.test(name)) return firmware?.downloads.sd_card ?? false;
  const update = firmware?.update_filename;
  if (update ? name.toUpperCase() === update : /\.dat$/i.test(name))
    return firmware?.downloads.installable ?? false;
  return true;
}
