import type { device, devicePermission } from '../db/device.ts';

type DeviceRow = typeof device.$inferInsert;
type PermissionRow = typeof devicePermission.$inferInsert;

export type DeviceKind = NonNullable<DeviceRow['kind']>;
export type DevicePermission = NonNullable<PermissionRow['permission']>;

export interface DevicePolicy {
  kind: DeviceKind;
  name: string;
  permissions: DevicePermission[];
}

/** The clients this Worker authorizes, and what each one may ask for afterwards.
 * A client's permissions come from here, never from the scopes it requests. */
const clients: Record<string, DevicePolicy> = {
  'x70-camera': {
    kind: 'camera',
    name: 'Fujifilm camera',
    permissions: ['google.photos.append'],
  },
};

export function devicePolicy(clientId: string | null | undefined): DevicePolicy | undefined {
  return clientId ? clients[clientId] : undefined;
}

export function isDeviceClient(clientId: string | null | undefined): boolean {
  return devicePolicy(clientId) !== undefined;
}
