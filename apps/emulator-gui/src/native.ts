import type { NativeCatalog, NativeSessionState } from '@fujimod/emulator-ui/engine/remote';

export interface NativeCamera {
  id: string;
  body: string;
  model: string;
  firmware: string;
  endpoint: string;
}

export type { NativeCatalog, NativeSessionState };
