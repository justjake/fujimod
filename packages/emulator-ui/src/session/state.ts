import type { AssetProgress } from '../engine/download-progress';
import type { EngineFault } from '../engine/fault';
import type { FaultContext } from '../ui/fault-text';
import type { Connection } from '../engine/api';

export function sessionState() {
  return {
    message: '',
    error: false,
    live: '',
    loading: false,
    loadingAssets: [] as AssetProgress[],
    webcamActive: false,
    webcamBusy: false,
    webcamError: '',
    progress: undefined as number | undefined,
    speed: undefined as number | undefined,
    fault: undefined as EngineFault | undefined,
    faultContext: { build: 'Development' } as FaultContext,
    canUndo: false,
    undoBusy: false,
    canRestart: false,
    idle: true,
    panelVisible: false,
    toolbarVisible: false,
    promptVisible: true,
    camerasVisible: true,
    connections: [] as Connection[],
  };
}
export type SessionState = ReturnType<typeof sessionState>;
export type SessionSetter = (write: (state: SessionState) => void) => void;
