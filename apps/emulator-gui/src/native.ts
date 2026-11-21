import type { Engine } from '@fujimod/emulator-ui/engine/api';
import {
  RemoteEngine,
  type NativeCatalog,
  type NativeSessionState,
} from '@fujimod/emulator-ui/engine/remote';

export interface NativeCamera {
  id: string;
  body: string;
  model: string;
  firmware: string;
  endpoint: string;
}

export type { NativeCatalog };
export type NativeState = NativeSessionState;
export type StateListener = (state: NativeState) => void;

/** The native session's device catalog arrives before the engine's first RPC. */
export class NativeEngine implements Engine {
  readonly remote: RemoteEngine;

  constructor(
    endpoint: string,
    private readonly onError: (error: Error) => void,
  ) {
    this.remote = new RemoteEngine(endpoint, onError);
  }

  state(listener: StateListener) {
    return this.remote.sessionState(listener);
  }

  catalog(listener: (catalog: NativeCatalog) => void) {
    return this.remote.catalog(listener);
  }

  exportSnapshot = () => this.remote.exportSnapshot();
  loadSnapshot: Engine['loadSnapshot'] = (bytes, config, progress) =>
    this.remote.loadSnapshot(bytes, config, progress);
  checkpoint = () => this.remote.checkpoint();
  restoreCheckpoint = (id: string) => this.remote.restoreCheckpoint(id);
  undo = () => this.remote.undo();
  history = () => this.remote.history();
  replayHistory = (bytes: Uint8Array) => this.remote.replayHistory(bytes);
  completeState = () => this.remote.completeState();
  prepare: Engine['prepare'] = (config) => this.remote.prepare(config);
  saveCamera = () => this.remote.saveCamera();
  cameraSaves: Engine['cameraSaves'] = (listener) => this.remote.cameraSaves(listener);
  replay: Engine['replay'] = (steps) => this.remote.replay(steps);
  metrics: Engine['metrics'] = (listener) => this.remote.metrics(listener);
  profile = (limit?: number) => this.remote.profile(limit);
  deviceStates = (kind: string) => this.remote.deviceStates(kind);
  errors: Engine['errors'] = (listener) => this.remote.errors(listener);
  rollback = (seconds: number) => this.remote.rollback(seconds);
  errorReport: Engine['errorReport'] = (submission) => this.remote.errorReport(submission);
  boot: Engine['boot'] = (dat, config, progress, media) =>
    this.remote.boot(dat, config, progress, media);
  restartWithCardFiles: Engine['restartWithCardFiles'] = (files, progress) =>
    this.remote.restartWithCardFiles(files, progress);
  formatCard: Engine['formatCard'] = (progress) => this.remote.formatCard(progress);
  insertCard = (present: boolean) => this.remote.insertCard(present);
  powerCycle: Engine['powerCycle'] = (inputs, progress) =>
    this.remote.powerCycle(inputs, progress);
  cardState = () => this.remote.cardState();
  cardTree = (revision?: number) => this.remote.cardTree(revision);
  readCardFile: Engine['readCardFile'] = (path, options) =>
    this.remote.readCardFile(path, options);
  connections: Engine['connections'] = (listener) => this.remote.connections(listener);
  cardChanges: Engine['cardChanges'] = (listener) => this.remote.cardChanges(listener);
  powerChanges: Engine['powerChanges'] = (listener) => this.remote.powerChanges(listener);
  frames: Engine['frames'] = (listener) => this.remote.frames(listener);
  press = (id: string, stage?: string) => this.remote.press(id, stage);
  release = (id: string) => this.remote.release(id);
  rotate = (id: string, steps: number) => this.remote.rotate(id, steps);
  touch: Engine['touch'] = (id, phase, x, y) => this.remote.touch(id, phase, x, y);
  setPosition = (id: string, position: string) => this.remote.setPosition(id, position);
  setScene: Engine['setScene'] = (scene) => this.remote.setScene(scene);
  power = (on: boolean) => this.remote.power(on);
  async dispose() {
    await this.remote.dispose();
  }
}

export function bootInputs(state: NativeState) {
  return state.inputs
    .filter((input) => !input.unavailable && input.positions?.length)
    .flatMap((input) => {
      const position = state.positions[input.id] ?? input.initial ?? input.positions![0]?.id;
      return position ? [{ id: input.id, position }] : [];
    });
}
