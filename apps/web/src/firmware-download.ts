export type FirmwareDownloadKind = 'recovery' | 'green-led' | 'modified';
export type FirmwareDownloadFormat = 'sd-boot-v1' | 'sd-card-recovery' | 'green-led' | 'installable';
export interface FirmwareDownloadIdentity {
  model: string;
  version: string;
}

/** The archive identifies its contents, boot format, camera, firmware and build. */
export function firmwareDownloadName(
  kind: FirmwareDownloadKind,
  format: FirmwareDownloadFormat,
  firmware: FirmwareDownloadIdentity,
  build: string,
): string {
  const model = firmware.model
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-|-$/g, '');
  const version = firmware.version
    .toLowerCase()
    .replace(/[^a-z0-9.]+/g, '-')
    .replace(/^[.-]+|[.-]+$/g, '');
  if (!model || !version || !/^[a-f0-9]{8}$/.test(build))
    throw Error('Firmware download identity is invalid.');
  return `fujimod-${kind === 'modified' ? `modified-${format}` : format}-${model}-${version}-build-${build}.zip`;
}
