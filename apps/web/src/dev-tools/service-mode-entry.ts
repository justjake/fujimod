import { catalog, initialize, type PhysicalControls } from 'fujimod';

/** A camera model and its catalog controls, when the catalog has them. */
export interface ServiceModeCamera {
  model: string;
  controls?: PhysicalControls;
}

/** Every catalog camera model, by name, with its physical controls when known. */
export async function serviceModeCameras(): Promise<ServiceModeCamera[]> {
  await initialize();
  const controls = new Map<string, PhysicalControls>();
  for (const selection of catalog.selections())
    if (selection.physical_controls)
      controls.set(selection.firmware.id, selection.physical_controls);
  const cameras = new Map<string, ServiceModeCamera>();
  for (const release of catalog.firmwareVersions())
    if (!cameras.get(release.product)?.controls)
      cameras.set(release.product, {
        model: release.product,
        controls: controls.get(release.firmware_id),
      });
  return [...cameras.values()].sort((a, b) =>
    a.model.localeCompare(b.model, 'en', { numeric: true }),
  );
}

/** Service-mode entry, in order, around the shared control sequence. */
export const serviceModeEntry = [
  'Switch the camera OFF, then remove and reinsert the battery.',
  'Switch the camera ON while holding the buttons, then release them.',
];
