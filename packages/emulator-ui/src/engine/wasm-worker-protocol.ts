import type { CardFile, CardReadOptions, EngineConfig, ReplayStep, Scene } from './api';
import type { BootInput, TouchPhase } from 'fujimod/emulator';
import type { AssetProgress } from './download-progress';
import type { EngineFault } from './fault';
import type { CameraPerformance } from './performance';
import type { SessionState } from 'fujimod/emulator';

export type WasmRequest =
  | { action: 'prepare'; bundle: string }
  | { action: 'load_snapshot'; bytes: Uint8Array; config: EngineConfig }
  | { action: 'export_snapshot' }
  | { action: 'usb_status' | 'usb_reset' | 'usb_step'; node: string }
  | { action: 'usb_setup'; node: string; bytes: Uint8Array }
  | { action: 'usb_out'; node: string; endpoint: number; bytes: Uint8Array }
  | { action: 'usb_in'; node: string; endpoint: number; size: number }
  | { action: 'usb_in_token'; node: string; endpoint: number }
  | {
      action: 'boot';
      dat: ArrayBuffer;
      config: EngineConfig;
      files: CardFile[];
      scene?: Scene;
      seedIdentity: { path: string; sha256: string }[];
    }
  | { action: 'press'; id: string; stage?: string }
  | { action: 'release'; id: string }
  | { action: 'rotate'; id: string; steps: number }
  | { action: 'touch'; id: string; phase: TouchPhase; x: number; y: number }
  | { action: 'set_position'; id: string; position: string }
  | { action: 'scene'; scene: Scene | null }
  | { action: 'power'; on: boolean }
  | { action: 'power_cycle'; inputs: BootInput[] }
  | { action: 'card_state' | 'card_format' | 'save' | 'error_report' }
  | { action: 'card_tree'; revision?: number }
  | { action: 'card_read'; path: string; options?: CardReadOptions }
  | { action: 'card_insert'; present: boolean }
  | { action: 'restart_card'; files: CardFile[] }
  | { action: 'rollback'; seconds: number }
  | { action: 'checkpoint' | 'undo' | 'history' | 'fingerprint' }
  | { action: 'restore_checkpoint'; id: string }
  | { action: 'replay_history'; bytes: Uint8Array }
  | { action: 'profile'; limit?: number }
  | { action: 'replay'; steps: readonly ReplayStep[] };
export type WasmWorkerRequest =
  | { type: 'rpc'; serial: number; request: WasmRequest }
  | { type: 'frame_release'; lease: number };
export type WasmWorkerEvent =
  | { type: 'reply'; serial: number; result?: unknown }
  | { type: 'error'; serial?: number; error: string; fault?: EngineFault }
  | { type: 'progress'; message: string; percent?: number; assets?: AssetProgress[] }
  | { type: 'state'; state: SessionState }
  | { type: 'metrics'; metrics: CameraPerformance }
  | { type: 'saved'; savedAt?: number; revision?: number; error?: string }
  | { type: 'frame'; bitmap: ImageBitmap; width: number; height: number; sequence: number }
  | {
      type: 'frame_view';
      buffer: SharedArrayBuffer;
      offset: number;
      length: number;
      width: number;
      height: number;
      sequence: number;
      lease: number;
    };
