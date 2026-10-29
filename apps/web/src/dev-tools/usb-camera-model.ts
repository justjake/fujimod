import { createSignal, runWithOwner } from 'solid-js';
import { shownError } from '@fujimod/emulator-ui/ui/shown-error';
import type { DevShareOptions } from './dev-share';
import {
  initialize,
  usb,
  type Device,
  type DeviceState,
  type Ff80Exchange,
  type Session,
} from 'fujimod';
import type { Callbacks, UsbExporter } from '../../../../crates/usb-proxy/web/types.ts';
import type {
  Camera,
  CameraIdentity,
  CameraLogEntry,
  CameraModel,
  SessionHolder,
  ToolMemory,
  ToolSession,
} from './camera-model';
import { serviceWorkflow, flashReader } from './service-artifacts';
import { Direction, Layer, Level, hex as dumpBytes } from './log';
import { ShareHost, newTicket, shareUrl, signalUrl } from './sharing';

export interface ModelStream<T> {
  next(): Promise<T | undefined>;
  close(): void;
  free(): void;
}
type Selection = Awaited<ReturnType<Device['interfaces']>>[number];
interface Reply {
  raw: Uint8Array;
  text?: string;
  free(): void;
}
export interface ModelSession extends Pick<
  Session,
  | 'close'
  | 'free'
  | 'abandon'
  | 'state'
  | 'backupFlash'
  | 'dumpState'
  | 'readConfiguration'
  | 'submitConsole'
  | 'prepareServiceSession'
  | 'armResetGuard'
  | 'enterDebugMode'
  | 'leaveDebugMode'
> {
  deviceInfo(selector: Parameters<Session['deviceInfo']>[0]): Promise<Reply>;
  memory(firmwareId: string): Promise<ToolMemory & { free(): void }>;
  configurationGeometry(
    view: Parameters<Session['configurationGeometry']>[0],
  ): Promise<{ capacity: number; pageBytes: number; free(): void }>;
  pollConsole(
    maximum: number,
  ): Promise<{ progress: number; status: number; output: Uint8Array; free(): void }>;
}
type Holder = { kind: 'local'; tool?: string } | { kind: 'remote'; controller: string };
type Exchange = Omit<Ff80Exchange, 'holder'> & { holder: Holder };
export interface ModelDevice extends Pick<
  Device,
  | 'id'
  | 'productName'
  | 'manufacturer'
  | 'serialNumber'
  | 'open'
  | 'close'
  | 'free'
  | 'interfaces'
  | 'stopSharing'
> {
  share(): UsbExporter;
  readonly status: DeviceState;
  watch(): ModelStream<DeviceState>;
  watchLog(): ModelStream<Exchange>;
  acquireSession(
    selection: Selection,
    limits: InstanceType<typeof usb.FirmwareLimits>,
    options: InstanceType<typeof usb.TransferOptions>,
    tool?: string,
  ): Promise<ModelSession>;
}
export interface ModelShare {
  stop(): void;
  cameraLost(reason: string): void;
  cameraReturned(exporter: UsbExporter): void;
}
declare const __FUJIMOD_SIMULATED_USB__: boolean;
export interface SimulatedCamera {
  device: ModelDevice;
  dispose(): void;
}
export interface UsbCameraServices {
  simulate?(savedCamera: string): Promise<SimulatedCamera>;
  initialize(): Promise<unknown>;
  discover(): Promise<readonly ModelDevice[]>;
  requestDevice(): Promise<ModelDevice | undefined>;
  discoverProxy(link: string): Promise<readonly ModelDevice[]>;
  share(device: ModelDevice, callbacks: Callbacks): Promise<{ host: ModelShare; link: string }>;
  now(): number;
  pause(milliseconds: number): Promise<void>;
}
const defaults: UsbCameraServices = {
  initialize,
  discover: () => usb.discover(usb.DeviceFilter.Diagnostic),
  requestDevice: () => usb.requestDevice(usb.DeviceFilter.Diagnostic),
  discoverProxy: (link) => usb.discoverProxy(link, '', usb.DeviceFilter.Diagnostic),
  async share(device, callbacks) {
    const ticket = newTicket(signalUrl(location.href));
    return {
      host: await ShareHost.create(ticket, device.share(), callbacks),
      link: shareUrl(location.href, ticket),
    };
  },
  simulate:
    typeof __FUJIMOD_SIMULATED_USB__ !== 'undefined' && __FUJIMOD_SIMULATED_USB__
      ? async (id) => (await import('./simulated-camera')).startSimulatedCamera(id)
      : undefined,
  now: () => Date.now(),
  pause: (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds)),
};
const selectors = [
  ['Vendor', 'vendor', 0x01],
  ['Manufacturer', 'manufacturer', 0x02],
  ['Product', 'product', 0x04],
  ['InternalVersion', 'internalVersion', 0x08],
  ['DeviceType', 'deviceType', 0x10],
  ['Serial', 'serial', 0x20],
  ['Framework', 'framework', 0x40],
] as const;
interface Entry {
  device: ModelDevice;
  camera: Camera;
  selection: Promise<Selection>;
  selections: Selection[];
  states: ModelStream<DeviceState>;
  logs: ModelStream<Exchange>;
  holder?: SessionHolder;
  session?: ModelSession;
  removed: boolean;
  disposeSimulation?: () => void;
  identityNeeded: boolean;
  identifying: boolean;
  debugModeNeeded: boolean;
  readingDebugMode: boolean;
  presenceGeneration: number;
  host?: ModelShare;
  sharing?: Promise<void>;
  shareAttempt: number;
  peers: Map<string, string>;
  activities: Map<
    string,
    { title?: string; progress?: { label: string; done: number; total: number } }
  >;
  problemKind?: 'presence' | 'session' | 'sharing' | 'attention';
  shareStatus?: ReturnType<NonNullable<DevShareOptions['status']>>;
}
const hex = (value: number) => `0x${value.toString(16)}`;
/** Format only complete, successful identity replies; execution validates them in Rust. */
function identityText(data: Uint8Array | undefined): string | undefined {
  if (!data || data.length < 16) return;
  const header = new DataView(data.buffer, data.byteOffset, data.length);
  if (
    header.getUint32(0, true) !== 0x150000 ||
    [4, 8, 12].some((offset) => header.getUint32(offset, true) !== 0)
  )
    return;
  for (let end = 16; end + 1 < data.length; end += 2) {
    if (data[end] !== 0 || data[end + 1] !== 0) continue;
    try {
      return new TextDecoder('utf-16le', { fatal: true, ignoreBOM: true }).decode(
        data.subarray(16, end),
      );
    } catch {
      return;
    }
  }
}

function toolSession(
  session: ModelSession,
  pause: UsbCameraServices['pause'],
  servicePrepared: () => void,
): ToolSession {
  return {
    async deviceInfo(selector) {
      const selected = selectors.find(([name]) => name === selector);
      if (!selected) throw Error(`Unknown identity selector: ${selector}`);
      const reply = await session.deviceInfo(usb.InfoSelector[selected[0]]);
      try {
        return { text: reply.text, data: reply.raw.slice() };
      } finally {
        reply.free();
      }
    },
    async configurationGeometry() {
      const reply = await session.configurationGeometry(usb.ConfigurationView.Ram);
      try {
        return { capacity: reply.capacity, pageBytes: reply.pageBytes };
      } finally {
        reply.free();
      }
    },
    readConfiguration: (offset, length) =>
      session.readConfiguration(usb.ConfigurationView.Ram, 0, offset, length),
    async memory(firmwareId) {
      const memory = await session.memory(firmwareId);
      return {
        read: (address, length) => memory.read(address, length),
        write: (address, data) => memory.write(address, data),
        get overwritten() {
          return memory.overwritten;
        },
        async close() {
          try {
            await memory.close();
          } finally {
            memory.free();
          }
        },
      };
    },
    async submitConsole(data) {
      await session.submitConsole(data);
    },
    async pollConsole() {
      const reply = await session.pollConsole(4096);
      try {
        if (reply.progress !== usb.ConsoleProgress.Complete && reply.output.length === 0)
          await pause(50);
        return {
          complete: reply.progress === usb.ConsoleProgress.Complete,
          status: reply.status,
          output: reply.output.slice(),
        };
      } finally {
        reply.free();
      }
    },
    debug: {
      armResetGuard: (...args) => session.armResetGuard(...args),
      enterDebugMode: (...args) => session.enterDebugMode(...args),
      leaveDebugMode: (...args) => session.leaveDebugMode(...args),
    },
    prepareServiceSession: async (firmwareId) => {
      const result = await session.prepareServiceSession(...(await serviceWorkflow(firmwareId)));
      servicePrepared();
      return result;
    },
    backupFlash: async (firmwareId, chunk, progress) => {
      const [metadata, image] = await flashReader(firmwareId);
      return session.backupFlash(firmwareId, metadata, image, chunk, progress);
    },
    dumpState: (firmwareId, region, progress) => session.dumpState(firmwareId, region, progress),
  };
}

const runtimes = new WeakMap<CameraModel, DevShareOptions & { stop?: () => void }>();

/** One reactive collection; Rust retains USB ownership and produces every FF80 log. */
export function createUsbCameraModel(overrides: Partial<UsbCameraServices> = {}): CameraModel {
  const services = { ...defaults, ...overrides };
  const entries = new Map<string, Entry>();
  let cameraValues: readonly Camera[] = [],
    logValues: readonly CameraLogEntry[] = [];
  const [cameraVersion, setCameraVersion] = createSignal(0);
  const [logVersion, setLogVersion] = createSignal(0);
  let version = 0,
    logRevision = 0,
    nextLog = 0,
    logBytes = 0,
    disposed = false;
  let countdown: ((camera: string, message: string) => Promise<void>) | undefined;
  const publish = () => {
    if (disposed) return;
    cameraValues = [...entries.values()].map(({ camera }) => ({
      ...camera,
      sharing: camera.sharing && { ...camera.sharing, peers: new Map(camera.sharing.peers) },
    }));
    setCameraVersion(++version);
  };
  const append = (
    entry: Entry,
    value: Omit<CameraLogEntry, 'id' | 'camera' | 'time'> & { time?: number },
  ) => {
    if (disposed || entry.removed) return;
    const row = {
      ...value,
      id: ++nextLog,
      camera: entry.camera.id,
      time: value.time ?? services.now(),
    };
    const rows = [...logValues, row];
    logBytes += row.data?.length ?? 0;
    while (rows.length > 2000 || (rows.length > 1 && logBytes > 16 * 1024 * 1024))
      logBytes -= rows.shift()!.data?.length ?? 0;
    logValues = rows;
    setLogVersion(++logRevision);
  };
  const connection = (entry: Entry, message: string, level = Level.Info, holder?: SessionHolder) =>
    append(entry, { layer: Layer.Connection, direction: Direction.Host, level, message, holder });
  const problem = (entry: Entry, message: string, kind: Entry['problemKind']) => {
    entry.camera.problem = message;
    entry.problemKind = kind;
    connection(entry, message, Level.Error);
    publish();
  };
  const sessionHolder = (entry: Entry, holder: Holder | null): SessionHolder | undefined =>
    holder?.kind === 'remote'
      ? { ...holder, reason: entry.peers.get(holder.controller) ?? '' }
      : holder
        ? {
            kind: 'local',
            tool: holder.tool ?? (entry.holder?.kind === 'local' ? entry.holder.tool : 'local'),
          }
        : undefined;
  const refreshSession = (entry: Entry) => {
    const state = entry.device.status;
    entry.camera.session = entry.holder ?? sessionHolder(entry, state.sessionOwner);
    entry.camera.uncertain = state.fenced;
    const holder = entry.camera.session;
    const activity =
      holder?.kind === 'remote' ? entry.activities.get(holder.controller) : undefined;
    entry.camera.remoteActivity =
      activity && holder?.kind === 'remote'
        ? { controller: holder.controller, ...activity }
        : undefined;
  };
  const requireEntry = (id: string) => {
    const entry = entries.get(id);
    if (!entry || entry.removed || disposed) throw Error('Camera is no longer available');
    return entry;
  };
  async function withSession<T>(
    id: string,
    tool: string,
    operation: (session: ToolSession) => Promise<T>,
  ): Promise<T> {
    const entry = requireEntry(id);
    refreshSession(entry);
    if (!entry.camera.present) throw Error('Camera is disconnected');
    if (entry.camera.uncertain) throw Error('USB completion unknown. Reconnect the camera.');
    if (entry.camera.session)
      throw Error('Camera session is held by another tool or remote controller');
    entry.holder = { kind: 'local', tool };
    refreshSession(entry);
    publish();
    let session: ModelSession | undefined;
    let outcome: { value: T } | { error: unknown } | undefined;
    try {
      const selected = await entry.selection;
      if (entry.removed || disposed || !entry.device.status.present)
        throw Error('Camera is disconnected');
      const options = new usb.TransferOptions(5000, 65536),
        limits = new usb.FirmwareLimits(16384, 16384, 256, 4096);
      try {
        session = await entry.device.acquireSession(selected, limits, options, tool);
      } finally {
        options.free();
        limits.free();
      }
      entry.session = session;
      if (entry.removed || disposed) throw Error('Camera is no longer available');
      outcome = {
        value: await operation(
          toolSession(session, services.pause, () => {
            if (entry.removed || disposed) return;
            entry.camera.serviceMode = true;
            entry.camera.debugMode = 'on';
            publish();
          }),
        ),
      };
    } catch (error) {
      outcome = { error };
      if (!entry.removed) problem(entry, shownError(error), 'session');
    } finally {
      if (session) {
        try {
          await session.close();
        } catch (error) {
          if (!outcome || 'value' in outcome) {
            outcome = { error };
            if (!entry.removed) problem(entry, shownError(error), 'session');
          }
        } finally {
          session.free();
        }
      }
      entry.session = undefined;
      entry.holder = undefined;
      if (session && (tool === 'Enable debug mode' || tool === 'Disable debug mode')) {
        entry.debugModeNeeded = true;
        entry.camera.debugMode = 'unknown';
      }
      if (!entry.removed && !disposed) {
        refreshSession(entry);
        publish();
        if (entry.identityNeeded)
          queueMicrotask(() => {
            void identity(entry);
          });
        await refreshDebugMode(entry);
      }
    }
    if (!outcome) throw Error('Camera session did not finish');
    if ('error' in outcome) throw outcome.error;
    if (entry.problemKind === 'session' && !entry.camera.uncertain) {
      entry.camera.problem = undefined;
      entry.problemKind = undefined;
      publish();
    }
    return outcome.value;
  }
  const readDebugMode = async (entry: Entry, session: ToolSession) => {
    const generation = entry.presenceGeneration;
    const current = () =>
      !entry.removed &&
      !disposed &&
      entry.camera.present &&
      entry.device.status.present &&
      generation === entry.presenceGeneration;
    try {
      const data = await session.readConfiguration(0xf7, 1);
      if (data.length !== 1) throw Error(`Expected one debug-mode byte, received ${data.length}`);
      if (!current()) return;
      entry.camera.debugMode = data[0] === 0 ? 'off' : 'on';
      if (entry.camera.transport === 'usb' && data[0] === 0) entry.camera.serviceMode = true;
    } catch (error) {
      if (!current()) return;
      entry.camera.debugMode = 'unknown';
      connection(entry, `Debug mode read failed: ${shownError(error)}`, Level.Error);
    }
    publish();
  };
  const refreshDebugMode = async (entry: Entry) => {
    if (
      !entry.debugModeNeeded ||
      entry.readingDebugMode ||
      entry.identifying ||
      entry.identityNeeded ||
      entry.removed ||
      disposed
    )
      return;
    refreshSession(entry);
    if (!entry.camera.present || entry.camera.uncertain || entry.camera.session) return;
    entry.readingDebugMode = true;
    entry.debugModeNeeded = false;
    try {
      await withSession(entry.camera.id, 'Debug mode', (session) => readDebugMode(entry, session));
    } catch {
      if (!entry.removed && entry.camera.present) {
        refreshSession(entry);
        // A peer can acquire after the idle check and before Rust grants this lease.
        entry.debugModeNeeded = Boolean(entry.camera.session) && !entry.camera.uncertain;
        entry.camera.debugMode = 'unknown';
        publish();
      }
    } finally {
      entry.readingDebugMode = false;
    }
  };
  const identity = async (entry: Entry) => {
    if (entry.identifying || !entry.identityNeeded || entry.removed || disposed) return;
    refreshSession(entry);
    if (!entry.camera.present || entry.camera.uncertain || entry.camera.session) return;
    const generation = entry.presenceGeneration;
    entry.identifying = true;
    entry.identityNeeded = false;
    entry.debugModeNeeded = false;
    try {
      const result = await withSession(entry.camera.id, 'identity', async (session) => {
        const result: CameraIdentity = {};
        try {
          for (const [selector, field] of selectors)
            result[field] = (await session.deviceInfo(selector)).text;
          return result;
        } finally {
          await readDebugMode(entry, session);
        }
      });
      if (!entry.removed) {
        entry.camera.identity = result;
        entry.camera.name = result.product || entry.device.productName || entry.camera.name;
        publish();
      }
    } catch {
      /* withSession records the connection error and Rust reports fencing. */
    } finally {
      entry.identifying = false;
      // Selection/open failures never enter the session callback that reads 0xF7.
      if (
        !entry.removed &&
        !disposed &&
        generation === entry.presenceGeneration &&
        entry.camera.debugMode === undefined
      ) {
        entry.camera.debugMode = 'unknown';
        publish();
      }
      if (entry.identityNeeded) void identity(entry);
      else void refreshDebugMode(entry);
    }
  };
  async function watchState(entry: Entry) {
    try {
      for (;;) {
        const state = await entry.states.next();
        if (!state || entry.removed || disposed) return;
        const wasPresent = entry.camera.present,
          wasUncertain = entry.camera.uncertain;
        entry.camera.present = state.present;
        if (wasPresent !== state.present) {
          entry.presenceGeneration++;
          if (state.present) {
            entry.camera.goneAt = undefined;
            entry.identityNeeded = true;
            connection(entry, 'Camera connected');
            if (entry.problemKind === 'presence' || entry.problemKind === 'session') {
              entry.camera.problem = undefined;
              entry.problemKind = undefined;
            }
            try {
              await entry.device.open();
              if (entry.host) entry.host.cameraReturned(entry.device.share());
              entry.shareStatus = { ...entry.shareStatus, online: true };
            } catch (error) {
              problem(entry, shownError(error), 'session');
            }
          } else {
            entry.camera.goneAt = services.now();
            entry.camera.debugMode = 'unknown';
            entry.camera.serviceMode = undefined;
            entry.debugModeNeeded = false;
            entry.shareStatus = { ...entry.shareStatus, online: false, reason: 'unplugged' };
            connection(entry, 'Camera disconnected');
            if (entry.host) {
              entry.host.cameraLost('unplugged');
              problem(entry, 'Camera disconnected while sharing', 'presence');
            }
          }
        }
        if (!entry.device.status.sharing && entry.host) {
          entry.host.stop();
          entry.host = undefined;
          entry.camera.sharing = undefined;
          entry.peers.clear();
          entry.activities.clear();
        }
        refreshSession(entry);
        if (!wasUncertain && entry.camera.uncertain)
          problem(entry, 'USB completion unknown. Reconnect the camera.', 'session');
        publish();
        void identity(entry);
        void refreshDebugMode(entry);
      }
    } catch (error) {
      if (!entry.removed && !disposed) problem(entry, shownError(error), 'session');
    }
  }
  async function watchLog(entry: Entry) {
    let command: Exchange | undefined;
    const exchange = (request: Exchange, reply?: Exchange, missingReply?: string) => {
      const input = ![0xd0, 0xe0].includes(request.kind);
      const error = [request.error, reply?.error, missingReply].filter(Boolean).join('; ');
      const selectorName =
        request.command === 0x150000
          ? selectors.find(([, , wire]) => wire === request.selector)?.[0]
          : undefined;
      const value =
        selectorName && !error
          ? identityText(reply?.data ?? (input ? request.data : undefined))
          : undefined;
      const description = [
        request.operation,
        request.address === null ? '' : hex(request.address),
        selectorName
          ? `${selectorName}${value === undefined ? '' : ` → ${value}`}`
          : request.selector === null
            ? ''
            : `selector ${hex(request.selector)}`,
        request.length === null ? '' : `${request.length} B`,
      ]
        .filter(Boolean)
        .join(' · ');
      const detail = [
        `Request envelope: ${dumpBytes(request.envelope)}`,
        !input && `Request payload: ${dumpBytes(request.data)}`,
        reply && `Reply envelope: ${dumpBytes(reply.envelope)}`,
        error,
        request.truncated && `Request capture: ${request.data.length} of ${request.transferred} B`,
        reply?.truncated && `Reply capture: ${reply.data.length} of ${reply.transferred} B`,
        !reply &&
          input &&
          request.truncated &&
          `Reply capture: ${request.data.length} of ${request.transferred} B`,
      ]
        .filter(Boolean)
        .join('\n');
      append(entry, {
        layer: Layer.Ff80,
        level: error ? Level.Error : Level.Debug,
        direction: reply ? Direction.In : input ? Direction.In : Direction.Out,
        time: request.timestampMs,
        holder: sessionHolder(entry, request.holder),
        message: description,
        sizes: {
          sent:
            request.envelope.length +
            (input ? 0 : request.transferred) +
            (reply?.envelope.length ?? 0),
          received: (input ? request.transferred : 0) + (reply?.transferred ?? 0),
        },
        data: (reply?.data ?? (request.kind === 0xd0 ? new Uint8Array() : request.data)).slice(),
        detail,
      });
    };
    const flush = (reason: string) => {
      if (command) exchange(command, undefined, reason);
      command = undefined;
    };
    try {
      for (;;) {
        const event = await entry.logs.next();
        if (entry.removed || disposed) return;
        if (!event) {
          flush('Command reply was not observed');
          return;
        }
        const matches =
          command &&
          event.kind === 0xd1 &&
          command.session === event.session &&
          JSON.stringify(command.holder) === JSON.stringify(event.holder);
        if (matches) {
          exchange(command!, event);
          command = undefined;
        } else {
          flush('Command reply was not observed');
          if (event.kind === 0xd0 && !event.error) command = event;
          else exchange(event);
        }
        if (event.consoleData?.length)
          append(entry, {
            layer: Layer.Console,
            level: Level.Info,
            direction: event.kind === 0xd0 ? Direction.Out : Direction.In,
            time: event.timestampMs,
            holder: sessionHolder(entry, event.holder),
            message: new TextDecoder().decode(event.consoleData),
            data: event.consoleData.slice(),
          });
      }
    } catch (error) {
      const message = shownError(error);
      flush(message);
      if (!entry.removed && !disposed) problem(entry, message, 'session');
    }
  }
  async function add(device: ModelDevice, transport: Camera['transport']) {
    if (disposed) {
      device.free();
      return;
    }
    const known = entries.get(device.id);
    if (known) {
      device.free();
      return;
    }
    const state = device.status;
    const entry: Entry = {
      device,
      camera: {
        id: device.id,
        transport,
        name: device.productName || (transport === 'usb' ? 'USB camera' : 'Shared camera'),
        present: state.present,
        uncertain: state.fenced,
        debugMode: 'unknown',
        goneAt: state.present ? undefined : services.now(),
      },
      selection: Promise.resolve().then(async () => {
        const selections = await device.interfaces();
        entry.selections = selections;
        if (entry.removed) {
          for (const selection of selections) selection.free();
          throw Error('Camera was removed');
        }
        if (selections.length !== 1) throw Error('Camera must have exactly one bulk USB interface');
        return selections[0];
      }),
      selections: [],
      states: device.watch(),
      logs: device.watchLog(),
      removed: false,
      identityNeeded: state.present,
      identifying: false,
      debugModeNeeded: false,
      readingDebugMode: false,
      presenceGeneration: 0,
      shareAttempt: 0,
      peers: new Map(),
      activities: new Map(),
    };
    entries.set(device.id, entry);
    refreshSession(entry);
    publish();
    connection(entry, state.present ? 'Camera connected' : 'Camera disconnected');
    void entry.selection.catch((error: unknown) => {
      if (!entry.removed) problem(entry, shownError(error), 'session');
    });
    void watchState(entry);
    void watchLog(entry);
    void identity(entry);
  }
  async function setSharing(id: string, on: boolean) {
    const entry = requireEntry(id);
    if (entry.camera.transport === 'remote') {
      if (on) throw Error('A remote camera cannot be shared again');
      return;
    }
    if (!on) {
      ++entry.shareAttempt;
      entry.host?.stop();
      entry.host = undefined;
      entry.device.stopSharing();
      entry.peers.clear();
      entry.activities.clear();
      entry.camera.remoteActivity = undefined;
      if (entry.camera.sharing) connection(entry, 'Camera sharing stopped');
      entry.camera.sharing = undefined;
      entry.shareStatus = undefined;
      refreshSession(entry);
      publish();
      return;
    }
    if (entry.host) return;
    if (entry.sharing) {
      const pending = entry.sharing;
      await pending;
      if (!entry.host && !entry.removed && !disposed) return setSharing(id, true);
      return;
    }
    if (!entry.camera.present) throw Error('Camera is disconnected');
    const attempt = ++entry.shareAttempt;
    const active = () => !entry.removed && !disposed && attempt === entry.shareAttempt;
    const activity = (
      controller: string,
      update: (value: {
        title?: string;
        progress?: { label: string; done: number; total: number };
      }) => void,
    ) => {
      if (!active()) return;
      const value = { ...entry.activities.get(controller) };
      update(value);
      if (value.title || value.progress) entry.activities.set(controller, value);
      else entry.activities.delete(controller);
      refreshSession(entry);
      publish();
    };
    entry.sharing = (async () => {
      const shared = await services.share(entry.device, {
        connected(controller, reason) {
          if (!active()) return;
          entry.peers.set(controller, reason);
          if (entry.camera.sharing)
            entry.camera.sharing = { ...entry.camera.sharing, peers: new Map(entry.peers) };
          refreshSession(entry);
          connection(
            entry,
            `Remote controller connected${reason ? `: ${reason}` : ''}`,
            Level.Info,
            { kind: 'remote', controller, reason },
          );
          publish();
        },
        disconnected(controller, reason) {
          if (!active()) return;
          const holder: SessionHolder = {
            kind: 'remote',
            controller,
            reason: entry.peers.get(controller) ?? '',
          };
          entry.peers.delete(controller);
          entry.activities.delete(controller);
          if (entry.camera.sharing)
            entry.camera.sharing = { ...entry.camera.sharing, peers: new Map(entry.peers) };
          refreshSession(entry);
          connection(entry, `Remote controller disconnected: ${reason}`, Level.Info, holder);
          publish();
        },
        toolStatus(controller, message) {
          if (active())
            connection(entry, message, Level.Info, {
              kind: 'remote',
              controller,
              reason: entry.peers.get(controller) ?? '',
            });
        },
        activity: (controller, title) =>
          activity(controller, (value) => {
            value.title = title || undefined;
          }),
        progress: (controller, progress) =>
          activity(controller, (value) => {
            value.progress = progress;
          }),
        countdown: async (_controller, message) => {
          if (!active()) throw Error('Camera sharing stopped');
          if (!countdown) throw Error('Owner countdown is unavailable');
          connection(entry, message, Level.Info, {
            kind: 'remote',
            controller: _controller,
            reason: entry.peers.get(_controller) ?? '',
          });
          await countdown(id, message);
        },
        attention: (_controller, message) => {
          if (active()) problem(entry, message, 'attention');
        },
        error: (error) => {
          if (active()) problem(entry, shownError(error), 'sharing');
        },
        state(status) {
          if (!active()) return;
          entry.shareStatus = {
            online: status.online,
            reason: status.reason,
            battery: status.battery,
            battery_at_ms:
              status.battery_age_ms === undefined
                ? undefined
                : services.now() - status.battery_age_ms,
          };
          publish();
          if (!status.online)
            problem(entry, `Camera offline: ${status.reason ?? 'unavailable'}`, 'sharing');
          else if (entry.problemKind === 'sharing') {
            entry.camera.problem = undefined;
            entry.problemKind = undefined;
            publish();
          }
        },
      });
      if (entry.removed || disposed || attempt !== entry.shareAttempt) {
        shared.host.stop();
        return;
      }
      entry.host = shared.host;
      if (!entry.camera.present) {
        entry.host.cameraLost('unplugged');
        problem(entry, 'Camera disconnected while sharing', 'presence');
      }
      entry.camera.sharing = { link: shared.link, peers: new Map(entry.peers) };
      connection(entry, 'Camera sharing started');
      publish();
    })();
    try {
      await entry.sharing;
    } catch (error) {
      if (active()) {
        entry.device.stopSharing();
        problem(entry, shownError(error), 'sharing');
      }
      throw error;
    } finally {
      entry.sharing = undefined;
    }
  }
  async function remove(id: string) {
    const entry = requireEntry(id);
    entry.shareAttempt++;
    entry.host?.stop();
    if (entry.camera.transport !== 'remote') entry.device.stopSharing();
    entry.host = undefined;
    entry.removed = true;
    entry.states.close();
    entry.logs.close();
    entry.session?.abandon();
    try {
      await entry.device.close();
    } finally {
      entry.states.free();
      entry.logs.free();
      for (const selection of entry.selections) selection.free();
      entry.device.free();
      entry.disposeSimulation?.();
      entries.delete(id);
      publish();
    }
  }
  const initialized = Promise.resolve().then(() => services.initialize());
  void initialized
    .then(() => services.discover())
    .then(async (devices) => {
      for (const device of devices) await add(device, 'usb');
    })
    .catch((error: unknown) => {
      // A picker can still add a camera if permitted-device discovery failed.
      const message = shownError(error);
      for (const entry of entries.values()) problem(entry, message, 'session');
    });
  const simulations = new Map<string, Promise<void>>();
  const model: CameraModel = {
    cameras() {
      cameraVersion();
      return cameraValues;
    },
    log() {
      logVersion();
      return logValues;
    },
    async connectUsb() {
      await initialized;
      const device = await services.requestDevice();
      if (device) await add(device, 'usb');
    },
    async connectRemote(link) {
      await initialized;
      const devices = await services.discoverProxy(link);
      for (const device of devices) await add(device, 'remote');
    },
    ...(services.simulate
      ? {
          connectSimulated(savedCamera: string): Promise<void> {
            if (disposed) return Promise.reject(Error('Camera model is disposed'));
            if (entries.has(`simulated-${savedCamera}`)) return Promise.resolve();
            const pending = simulations.get(savedCamera);
            if (pending) return pending;
            const start = (async () => {
              await initialized;
              const simulation = await services.simulate!(savedCamera);
              try {
                if (disposed) throw Error('Camera model is disposed');
                await add(simulation.device, 'simulated');
                const entry = requireEntry(simulation.device.id);
                entry.disposeSimulation = () => simulation.dispose();
              } catch (error) {
                try {
                  await simulation.device.close();
                } finally {
                  simulation.device.free();
                  simulation.dispose();
                }
                throw error;
              }
            })().finally(() => simulations.delete(savedCamera));
            simulations.set(savedCamera, start);
            return start;
          },
        }
      : {}),
    remove,
    withSession,
    setSharing,
    onOwnerCountdown(run) {
      countdown = run;
      return () => {
        if (countdown === run) countdown = undefined;
      };
    },
    dismissProblem(id) {
      const entry = requireEntry(id);
      entry.camera.problem = undefined;
      entry.problemKind = undefined;
      publish();
    },
    clearLog() {
      logValues = [];
      logBytes = 0;
      setLogVersion(++logRevision);
    },
    dispose() {
      if (disposed) return;
      runtimes.get(model)?.stop?.();
      for (const id of entries.keys()) void remove(id).catch((error) => console.error(error));
      disposed = true;
      countdown = undefined;
    },
  };
  runtimes.set(model, {
    report(id, message, error) {
      const entry = entries.get(id);
      if (entry) connection(entry, message, error ? Level.Error : Level.Info);
    },
    status: (id) => entries.get(id)?.shareStatus,
  });
  return model;
}

const page =
  typeof window === 'undefined'
    ? undefined
    : (window as Window & { __fujimodCameraModel?: CameraModel });

/** The browser page owns this instance across component and module hot swaps. */
export function pageCameraModel(): CameraModel {
  if (!page) return runWithOwner(null, () => createUsbCameraModel());
  if (page.__fujimodCameraModel) return page.__fujimodCameraModel;
  const model = runWithOwner(null, () => createUsbCameraModel());
  page.__fujimodCameraModel = model;
  // Loaded only in development and local-preview builds; production bundles omit it.
  if (import.meta.env.DEV || import.meta.env.MODE === 'local-preview') {
    const runtime = runtimes.get(model)!;
    void import('./dev-share').then(({ startDevShare }) => {
      runtime.stop = runWithOwner(null, () => startDevShare(model, runtime));
    });
  }
  return model;
}
