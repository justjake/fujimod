import type {
  BootInput,
  Camera,
  CardFile,
  EmulatorModule,
  SessionState,
  TouchPhase,
  UsbStatus,
} from 'fujimod/emulator';

/** Generated-module double for transport/lifetime tests. Never loaded by an engine worker. */
export function testCameraModule(shared = false) {
  const memory = new WebAssembly.Memory({ initial: 2, maximum: 4, shared });
  let ticks = 0n,
    failed = false,
    createError = '',
    commandError = '',
    frameEnabled = false,
    prepared: (request: string, camera?: Uint8Array) => void = () => {};
  const calls: { action: string; options: Record<string, unknown>; bytes: number[] }[] = [];
  const state: SessionState = {
    inputs: [{ id: 'menu', label: 'Menu', kind: 'button', supported: true }],
    positions: { power: 'on' },
    inputQueue: null,
    poweredOn: true,
    card: {
      supported: true,
      present: true,
      protected: false,
      busy: false,
      revision: 1,
      capacity_bytes: 1000,
      data_faults: [],
      fault_history: [],
    },
  };
  const record = (
    action: string,
    options: Record<string, unknown> = {},
    bytes: Uint8Array = new Uint8Array(),
  ) => {
    calls.push({ action, options, bytes: [...bytes] });
    if (commandError) throw Error(commandError);
  };
  const point = () => ({ tick: String(ticks), instructions: [String(ticks * 2n)] });
  const created = () => {
    if (createError) throw Error(createError);
    return new TestCamera() as unknown as Camera;
  };
  const disposable = { free() {}, [Symbol.dispose]() {} };
  class TestCamera implements Partial<Camera> {
    static fromGraph(_config: string, _dat: Uint8Array) {
      return created();
    }
    static fromResolved(_config: string, _firmware: Uint8Array) {
      return created();
    }
    static fromPrepared(
      request: string,
      _update: Uint8Array,
      _bootRom: Uint8Array,
      _nand?: Uint8Array,
      _settings?: Uint8Array,
      camera?: Uint8Array,
    ) {
      prepared(request, camera);
      return created();
    }
    static fromSnapshot(bytes: Uint8Array) {
      calls.push({ action: 'load_snapshot', options: {}, bytes: [...bytes] });
      ticks = 17n;
      return created();
    }
    free() {}
    [Symbol.dispose]() {}
    close() {}
    runUntil(until: bigint, progress?: (instructions: number) => void) {
      if (failed) {
        const error = Error('unmapped guest load');
        error.name = 'GuestExecutionError';
        throw error;
      }
      ticks = until;
      progress?.(Number(ticks * 2n));
    }
    input(key: string, pressed: boolean, at: bigint) {
      calls.push({ action: 'input', options: { key, pressed, at }, bytes: [] });
    }
    status() {
      return { ticks, instructions: ticks * 2n, failed, ...disposable };
    }
    frame(output: number) {
      if (!frameEnabled) return undefined;
      const rgba = new Uint8Array(memory.buffer, 70000, 4);
      rgba.set([10, 20, 30, 255]);
      return { width: 1, height: 1, output, sequence: ticks, rgba, ...disposable };
    }
    setScene(width: number, height: number, rgb: Uint8Array) {
      record('scene', { width, height }, rgb);
    }
    clearScene() {
      record('scene');
    }
    sceneRaf(raf: Uint8Array) {
      record('scene_raf', {}, raf);
    }
    state() {
      record('state');
      return state;
    }
    usbStatus(node: string): UsbStatus {
      record('usb_status', { node });
      return {
        connected: true,
        endpoint: {
          options: {
            transfer_ticks: 4,
            packet_bytes: 64,
            dma_limit: 1024,
            out_endpoints: [],
            in_endpoints: [],
          },
          wiring: { irq: { Shared: 0 }, dma_irq: { Shared: 1 }, ram_bytes: 0 },
          registers: [],
          rx_length: [0, 0],
          rx_position: [0, 0],
          setup_length: 0,
          setup_position: 0,
          tx_packets: [],
          tx_bytes: [],
          active_dma: false,
        },
      };
    }
    usbReset(node: string) {
      record('usb_reset', { node });
    }
    usbSetup(node: string, packet: Uint8Array) {
      record('usb_setup', { node }, packet);
    }
    usbOut(node: string, endpoint: number, packet: Uint8Array) {
      record('usb_out', { node, endpoint }, packet);
    }
    usbInToken(node: string, endpoint: number) {
      record('usb_in_token', { node, endpoint });
    }
    usbIn(node: string, endpoint: number, size: number) {
      record('usb_in', { node, endpoint, size });
      return new Uint8Array();
    }
    press(id: string, stage?: string | null) {
      record('press', { id, stage });
      return state;
    }
    releaseBootInputs(inputs: BootInput[]) {
      record('release_boot_inputs', { inputs });
      return state;
    }
    release(id: string) {
      record('release', { id });
      return state;
    }
    rotate(id: string, steps: number) {
      record('rotate', { id, steps });
      return state;
    }
    touch(id: string, phase: TouchPhase, x: number, y: number) {
      record('touch', { id, phase, x, y });
      return state;
    }
    setPosition(id: string, position: string) {
      record('set_position', { id, position });
      return state;
    }
    power(on: boolean) {
      record('power', { on });
      state.poweredOn = on;
      ticks = 0n;
      return state;
    }
    powerCycle(inputs: BootInput[]) {
      record('power_cycle', { inputs });
      state.poweredOn = true;
      ticks = 0n;
      return state;
    }
    replaceCard(files: CardFile[], upload: Uint8Array, inputs: BootInput[]) {
      record('restart_card', { files, inputs }, upload);
      state.poweredOn = true;
      ticks = 0n;
      return state;
    }
    cardState() {
      record('card_state');
      return state.card;
    }
    cardTree(revision?: number | null) {
      record('card_tree', { revision });
      return { ...state.card, entries: [] };
    }
    cardRead(path: string, revision?: number | null, maxBytes?: number | null) {
      record('card_read', { path, revision, maxBytes });
      return new Uint8Array([4, 5, 6]);
    }
    cardInsert(present: boolean) {
      record('card_insert', { present });
      return state;
    }
    cardFormat() {
      record('card_format');
      return state;
    }
    bootToRec(limit: bigint, progress?: (instructions: number) => void) {
      progress?.(Number(ticks * 2n));
      record('boot_to_rec', { limit });
      return {
        status: 'rec',
        instructions: 0,
        virtual_ticks: 0,
        ui_mode: 0,
        system_mode: 3,
        menu_active: 0,
        menu_node: 0,
        pending_mode: 0,
        pending: false,
        setup_presses: 0,
        translation_cache_size_bytes: 0,
        scanout_instance: null,
        failure: null,
      };
    }
    preview() {
      record('preview');
      return { ticksPerSecond: '12000000', frames: Number(ticks / 1200000n), state: {} };
    }
    previewState() {
      record('preview_state');
      return { ticksPerSecond: '12000000', frames: Number(ticks / 1200000n), state: {} };
    }
    checkpoint() {
      record('checkpoint');
      return { id: String(ticks), ...point(), retainedIds: [String(ticks)] };
    }
    restore(id: string) {
      record('restore', { id });
      ticks = BigInt(id);
      failed = false;
      return point();
    }
    undo() {
      record('undo');
      return point();
    }
    rollback(tick: bigint) {
      record('rollback', { tick });
      failed = false;
      ticks = 10n;
      return point();
    }
    history() {
      record('history');
      return { log: new Uint8Array([0x81, 0x01]), entries: 1 };
    }
    replay(log: Uint8Array) {
      record('replay', {}, log);
      return point();
    }
    fingerprint() {
      record('fingerprint');
      return { sha256: 'a'.repeat(64) };
    }
    profile(limit?: number | null) {
      record('profile', { limit });
      return { ticks: Number(ticks), instructions: [Number(ticks * 2n)], pcEntries: [], failed };
    }
    memoryRead(address: bigint, size: number) {
      record('memory_read', { address, size });
      return new Uint8Array(size);
    }
    errorReport() {
      record('error_report');
      return {
        report: {
          bus: {
            devices: [
              { kind: 'external-image-frames', state: { frames: Number(ticks / 400000n) } },
            ],
          },
        },
        snapshot: new Uint8Array([0xa1, 0x01]),
      };
    }
    exportSnapshot() {
      record('export_snapshot');
      return { snapshot: new Uint8Array([0xa1, 0x01]), ...point() };
    }
    save() {
      record('save');
      return { durable: new Uint8Array([1, 2]), positions: state.positions, poweredOn: true };
    }
    restoreDurable(durable: Uint8Array) {
      record('restore_durable', {}, durable);
      return state;
    }
  }
  const module = {
    configure() {},
    jitStatistics: () => ({ enabled: false }),
    memoryBytes: () => memory.buffer.byteLength,
    Camera: TestCamera,
  } as unknown as EmulatorModule;
  return {
    module,
    memory,
    calls,
    state,
    ticks: () => ticks,
    createError(value: string) {
      createError = value;
    },
    commandError(value: string) {
      commandError = value;
    },
    /** Observe the prepared construction request text. */
    prepared(hook: (request: string, camera?: Uint8Array) => void) {
      prepared = hook;
    },
    fail() {
      failed = true;
    },
    frame() {
      frameEnabled = true;
    },
  };
}
