/**
 * The Dev Tools camera model (see DESIGN.md). One entry per camera the page
 * knows about; everything the page shows is derived from these values.
 * Implementations: the USB model over the Rust device owner, and test models.
 */
import type { Session } from 'fujimod';
import type { Layer, Level, Direction } from './log';

/** Who holds a camera's session. */
export type SessionHolder =
  | { kind: 'local'; tool: string }
  | { kind: 'remote'; controller: string; reason: string };

export interface CameraIdentity {
  vendor?: string;
  manufacturer?: string;
  product?: string;
  internalVersion?: string;
  deviceType?: string;
  serial?: string;
  framework?: string;
}

export interface CameraSharing {
  link: string;
  /** Connected remote controllers, by id, with the reason each gave. */
  peers: ReadonlyMap<string, string>;
}

export interface Camera {
  /** Stable for the page's lifetime; the same camera plugged back in keeps it. */
  id: string;
  /**
   * How the camera is reached: this browser's USB, another host's share, or a
   * simulated camera on a saved firmware over simulated USB.
   */
  transport: 'usb' | 'remote' | 'simulated';
  name: string;
  present: boolean;
  /** Time the camera went away, while not present. */
  goneAt?: number;
  /** Filled by the automatic identity read when the camera appears. */
  identity?: CameraIdentity;
  /**
   * The debug-mode flag (RAM configuration byte 0xF7), read with the identity
   * and after each debug-mode change. 'unknown' until observed or if the read
   * is unavailable or failed.
   */
  debugMode?: 'on' | 'off' | 'unknown';
  /**
   * True once the camera is known to be in service mode: the diagnostic link
   * with debug mode off is only available in service mode. Service mode cannot
   * change without a restart, so this holds until the camera goes away.
   */
  serviceMode?: boolean;
  session?: SessionHolder;
  /** A session ended without a known completion; reopen before new sessions. */
  uncertain: boolean;
  /** Absent while sharing is off. Remote cameras cannot be shared again. */
  sharing?: CameraSharing;
  /** What the remote tool holding the session reports it is doing. */
  remoteActivity?: {
    controller: string;
    title?: string;
    progress?: { label: string; done: number; total: number };
  };
  /** The camera's latest unresolved problem, until it is dismissed or resolved. */
  problem?: string;
}

/** One console log entry. Every entry names its camera. */
export interface CameraLogEntry {
  id: number;
  time: number;
  camera: string;
  holder?: SessionHolder;
  layer: Layer;
  level: Level;
  direction: Direction;
  message: string;
  /** Request and response byte counts, for FF80 exchanges. */
  sizes?: { sent: number; received: number };
  data?: Uint8Array;
  detail?: string;
}

/** An open ff80 session. Close it when the tool's operation is done. */
export interface ToolSession {
  deviceInfo(selector: string): Promise<{ text?: string; data: Uint8Array }>;
  configurationGeometry(): Promise<{ capacity: number; pageBytes: number }>;
  readConfiguration(offset: number, length: number): Promise<Uint8Array>;
  /**
   * Qualify the firmware's stock bulk path and read a range through it. The
   * result names diagnostic scratch the stock setup overwrote.
   */
  readBulk(
    firmwareId: string,
    address: number,
    length: number,
  ): Promise<{ data: Uint8Array; overwritten: { address: number; size: number } | null }>;
  submitConsole(data: Uint8Array): Promise<void>;
  pollConsole(): Promise<{ complete: boolean; status: number; output: Uint8Array }>;
  /** The debug-mode workflows, run by the Enter and Leave debug mode tools. */
  debug: Pick<Session, 'armResetGuard' | 'enterDebugMode' | 'leaveDebugMode'>;
  /**
   * Confirm service mode, arm the RAM reset guard and enable debug access in
   * RAM until the camera restarts. Nothing is stored.
   */
  prepareServiceSession(firmwareId: string): Promise<ServicePreparation>;
  /**
   * Copy the whole internal flash through the firmware's own reader, calling
   * `chunk` with each piece in read order. Restores everything it borrowed.
   * Progress counts delivered flash bytes, excluding the control read.
   */
  backupFlash(
    firmwareId: string,
    chunk: (piece: { offset: number; data: Uint8Array }) => void,
    progress?: (done: number, total: number) => void,
  ): Promise<FlashBackup>;
  /**
   * Read every region of the firmware's camera state (DRAM, boot ROM, other
   * readable device state), calling `region` as each completes.
   * Progress counts read bytes, including both reads of constant regions.
   */
  dumpState(
    firmwareId: string,
    region: (piece: StateRegion & { data: Uint8Array }) => void,
    progress?: (done: number, total: number) => void,
  ): Promise<void>;
}

export interface ServicePreparation {
  guardBefore: number;
  debugBefore: number;
  progress: string[];
}
export interface FlashBackup {
  /** Device kind and part, from the firmware's flash fact. */
  device: { kind: string; part?: string };
  size: number;
  sha256: string;
}
export interface StateRegion {
  name: string;
  address: number;
  size: number;
  /** Constant regions (boot ROM) are read twice and must match. */
  constant: boolean;
  sha256: string;
}

export interface CameraModel {
  /** Every camera the page knows about, in the order they appeared. */
  cameras(): readonly Camera[];
  /** The console log, oldest first. */
  log(): readonly CameraLogEntry[];
  /** Show the browser's USB picker and add the chosen camera. */
  connectUsb(): Promise<void>;
  /** Add the camera another host shares at `link`. */
  connectRemote(link: string): Promise<void>;
  /**
   * Start the saved camera `savedCamera` (an id from the emulator's camera
   * store) as a simulated camera over simulated USB. Absent while this build
   * cannot run a simulated camera.
   */
  connectSimulated?(savedCamera: string): Promise<void>;
  /** Forget a camera: closes its owner and removes it from the list. */
  remove(camera: string): Promise<void>;
  /**
   * Run `operation` in a session on `camera`, held by the local `tool`.
   * Refuses without USB traffic while the camera is not present, uncertain,
   * or held by anyone else. The session closes when `operation` settles.
   */
  withSession<T>(
    camera: string,
    tool: string,
    operation: (session: ToolSession) => Promise<T>,
  ): Promise<T>;
  /** Turn sharing on or off. Never affects sessions or the connection. */
  setSharing(camera: string, on: boolean): Promise<void>;
  /**
   * Called when a remote tool asks for the owner's attention. The page runs the
   * spoken countdown and resolves when it finishes; the remote tool waits.
   */
  onOwnerCountdown(run: (camera: string, message: string) => Promise<void>): () => void;
  dismissProblem(camera: string): void;
  clearLog(): void;
  dispose(): void;
}
