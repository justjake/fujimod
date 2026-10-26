import type { ShareTrafficEvent } from './share-traffic.ts';
export interface Descriptor {
  id: string;
  vendor_id: number;
  product_id: number;
  manufacturer: string | null;
  product: string | null;
  serial: string | null;
  interfaces: {
    configuration: number;
    interface: number;
    alternate: number;
    endpoints: { address: number; kind: string; packet_size: number }[];
  }[];
  status?: ShareStatus;
}
export interface Operation {
  op: string;
  data?: Uint8Array;
  configuration?: number;
  interface?: number;
  alternate?: number;
  endpoint?: number;
  length?: number;
  message?: string;
  setup?: {
    request_type?: string;
    recipient?: string;
    request: number;
    value?: number;
    index?: number;
  };
  /** Full byte count when this is a bounded diagnostic snapshot. */
  dataLength?: number;
  [key: string]: unknown;
}
export interface Completion {
  devices?: Descriptor[];
  data?: Uint8Array;
  transferred?: number;
  dataLength?: number;
  error?: string | null;
}
/** The Rust/Wasm exporter. JavaScript only routes encoded proxy messages. */
export interface UsbExporter {
  readonly inventoryJson: string;
  execute(controller: string, message: Uint8Array): Promise<Uint8Array>;
  busy(device: string): boolean;
  owns(controller: string, device: string): boolean;
  fenced(device: string): boolean;
  abandon(controller: string): boolean;
  stopSharing(): void;
  shutdown(): Promise<void>;
  free(): void;
}
export interface Ticket {
  v: 1;
  signal: string;
  room: string;
  key: string;
  stun: string[];
}
export interface Callbacks {
  /** `reason`: why the controller connected (native tools send their command line), or ''. */
  connected?(id: string, reason: string): void;
  /** `reason`: the controller's goodbye ("done" or its error), or 'connection lost without goodbye'. */
  disconnected?(id: string, reason: string): void;
  /** Passive owner-side request and completion observation, including failed operations. */
  traffic?(this: void, event: ShareTrafficEvent): void;
  /** A controller reported the camera's battery level (firmware level name). */
  battery?(id: string, level: string): void;
  /** A controller asks the owner's user to fix something only it can see. */
  attention?(id: string, message: string): void;
  /** Neutral tool progress, including an expected refusal to continue. */
  toolStatus?(id: string, message: string): void;
  /** What a controller is doing now; an empty title clears it. */
  activity?(id: string, title: string): void;
  /** A controller's progress bar, or undefined when it is done. */
  progress?(id: string, progress: { label: string; done: number; total: number } | undefined): void;
  /** Resolves only after the owner's spoken ten-second countdown. */
  countdown?(this: void, id: string, message: string): Promise<void>;
  /** Online state or battery changed. */
  state?(status: ShareStatus): void;
  error?(error: Error): void;
}
export interface ShareStatus {
  online: boolean;
  /** Why the camera is offline: 'battery', 'unplugged' or 'not responding'. */
  reason?: string;
  battery?: string;
  battery_age_ms?: number;
}

/** Run `op` after every earlier operation, unless the pipeline has failed. */
export interface ExecuteMessage {
  type: 'execute';
  seq: number;
  device: string;
  timeout_ms: number;
  op: Operation;
}
/** Fail the pipeline unless IN result `of` holds `bytes` at `offset` (under `mask`). */
export interface ExpectMessage {
  type: 'expect';
  seq: number;
  of: number;
  offset: number;
  bytes: Uint8Array;
  mask?: Uint8Array | null;
  length?: number | null;
  alternatives?: Uint8Array[];
}
export interface ResultMessage {
  type: 'result';
  seq: number;
  completion: Completion;
}
/** One postcard value on the data path; see wire.ts and PIPELINE.md. */
export type Message =
  | { type: 'hello'; version: number; role: string; token: string; reason?: string | null }
  | { type: 'ready'; version: number; id: string }
  | { type: 'goodbye'; reason: string }
  | { type: 'status'; battery: string }
  | { type: 'attention'; message: string }
  | { type: 'activity'; title: string }
  | { type: 'progress_start'; label: string; total: number }
  | { type: 'progress_update'; done: number }
  | { type: 'progress_done' }
  | { type: 'inventory'; devices: Descriptor[] }
  | ExecuteMessage
  | ExpectMessage
  | { type: 'sync'; seq: number }
  | ResultMessage
  | { type: 'expect_failed'; seq: number; actual: Uint8Array }
  | { type: 'synced'; seq: number; discarded: number }
  | { type: 'routed'; stream: number; message: Message };
export interface SignalFrame {
  type: string;
  from: string;
  peer: string;
  data: string;
  message?: string;
  ice_servers?: RTCIceServer[];
}
