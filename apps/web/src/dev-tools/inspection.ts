/** Read-only ff80 operations, shared by USB and the simulator worker. */
export const infoSelectors = [
  'Vendor',
  'Manufacturer',
  'Product',
  'InternalVersion',
  'DeviceType',
  'Serial',
  'Framework',
] as const;
export type InfoSelector = (typeof infoSelectors)[number];
export type InspectionRequest =
  | { kind: 'identity'; selector: InfoSelector }
  | { kind: 'geometry' }
  | { kind: 'configuration'; offset: number; length: number };
export interface InspectionReply {
  data: Uint8Array;
  text?: string;
  capacity?: number;
  pageBytes?: number;
}
export type InspectorRead = (request: InspectionRequest) => Promise<InspectionReply>;
