import { serviceValues, definitionValues, valueSchemas } from '../pkg/service-values.js';
export type * from '../pkg/fujimod.js';
import * as native from '../pkg/fujimod.js';
export { default as initialize } from '../pkg/fujimod.js';
export type {
  Session,
  ResetGuardResult,
  DebugModeResult,
  Device,
  Exporter,
  DeviceEvents,
  DeviceState,
  LogEvents,
  Ff80Exchange,
  InfoReply,
  ConfigurationGeometry,
  DiagnosticReply,
  ConsoleReply,
  LogReply,
  CapabilitiesReply,
  FirmwareDownloadReply,
  DataWindow,
  UploadReply,
  ResearchRequest,
  ResearchReply,
  App80Request,
  Mailbox,
} from '../pkg/fujimod.js';
export type {
  FirmwareIdentity,
  ServiceModes,
  ServiceDefinition,
  ServiceEntry,
  ServiceMailbox,
  ModeAvailability,
  ModeTransition,
  DefinitionFailure,
  CommandFailure,
  App80Invocation,
  App80CommandReply,
} from '../pkg/fujimod.js';

class ChunkIterator implements AsyncIterableIterator<Uint8Array> {
  constructor(private readonly reader: native.RangeReader) {}
  get bytesCompleted(): number {
    return this.reader.bytesCompleted;
  }
  close(): void {
    this.reader.close();
  }
  cancelPending(): void {
    this.reader.cancelPending();
  }
  async next(): Promise<IteratorResult<Uint8Array>> {
    const value = await this.reader.nextChunk();
    return value === undefined ? { done: true, value: undefined } : { done: false, value };
  }
  async return(): Promise<IteratorResult<Uint8Array>> {
    this.close();
    return { done: true, value: undefined };
  }
  [Symbol.asyncIterator](): AsyncIterableIterator<Uint8Array> {
    return this;
  }
}

/** One cohesive namespace; protocol operations execute in the Rust core. */
export interface UsbError extends Error {
  readonly kind: native.ErrorKind;
  readonly stage?: native.TraceStage;
  readonly requestKind?: native.TraceRequestKind;
  readonly transferKind?: native.TransferFailureKind;
  readonly protocolKind?: native.ProtocolFailureKind;
  readonly command?: number;
  readonly primary?: number;
  readonly secondary?: number;
  readonly transferred?: number;
  readonly expected?: number;
  readonly actual?: number;
  readonly available?: number;
  readonly declared?: number;
  readonly requested?: number;
  readonly limit?: number;
  readonly reserved?: number;
  readonly backendCode?: number;
  readonly partial?: Uint8Array;
  readonly cause?: UsbError;
  readonly reason?: native.DefinitionFailure | native.CommandFailure;
}
function isUsbError(error: unknown): error is UsbError {
  return (
    error instanceof Error &&
    'kind' in error &&
    typeof error.kind === 'number' &&
    Object.values(native.ErrorKind).includes(error.kind)
  );
}
export interface DefinitionError extends UsbError {
  readonly kind: native.ErrorKind.Definition;
  readonly reason: native.DefinitionFailure;
}
export interface ServiceCommandError extends UsbError {
  readonly kind: native.ErrorKind.ServiceCommand;
  readonly reason: native.CommandFailure;
}
function isDefinitionError(error: unknown): error is DefinitionError {
  return isUsbError(error) && error.kind === native.ErrorKind.Definition;
}
function isServiceCommandError(error: unknown): error is ServiceCommandError {
  return isUsbError(error) && error.kind === native.ErrorKind.ServiceCommand;
}
/** Caller-selected data shared by USB and future emulator/SDK consumers. */
export const catalog = Object.freeze({
  /** Every bundled catalog firmware with its body, SoC, parts and generated facts. */
  selections: native.catalogSelections,
  /** Public/internal release identities, including bodies without emulator support. */
  firmwareVersions: native.catalogFirmwareVersions,
  FirmwareIdentity: native.FirmwareIdentity,
  DefinitionFailureKind: native.DefinitionFailureKind,
  isError: isDefinitionError,
  service: Object.freeze({
    Definition: Object.freeze({
      fromJson: native.ServiceDefinition.fromJson.bind(native.ServiceDefinition),
    }),
    ServiceModes: native.ServiceModes,
    ...definitionValues,
    ModeAvailabilityKind: native.ModeAvailabilityKind,
    ModeTransitionKind: native.ModeTransitionKind,
  }),
});
/** Construct an SD recovery card from an identity-checked official update. */
export const recovery = Object.freeze({
  availability: native.recoveryAvailability,
  build: native.buildRecoveryCard,
});
export const usb = Object.freeze({
  Device: native.Device,
  Exporter: native.Exporter,
  Session: native.Session,
  SessionState: native.SessionState,
  ErrorKind: native.ErrorKind,
  ProtocolFailureKind: native.ProtocolFailureKind,
  TransferFailureKind: native.TransferFailureKind,
  TransferStage: native.TraceStage,
  RequestKind: native.TraceRequestKind,
  isError: isUsbError,
  DeviceFilter: native.DeviceFilter,
  discover: native.discover,
  discoverProxy: native.discoverProxy,
  requestDevice: native.requestDevice,
  attachWebUsbDevice: native.attachWebUsbDevice,
  detachWebUsbDevice: native.detachWebUsbDevice,
  FirmwareLimits: native.FirmwareLimits,
  InterfaceSelection: native.InterfaceSelection,
  TransferOptions: native.TransferOptions,
  InfoSelector: native.InfoSelector,
  InfoReply: native.InfoReply,
  ConfigurationView: native.ConfigurationView,
  ConfigurationGeometry: native.ConfigurationGeometry,
  DiagnosticTag: native.DiagnosticTag,
  DiagnosticReply: native.DiagnosticReply,
  CommandStatus: native.CommandStatus,
  KeyState: native.KeyState,
  CameraMode: native.CameraMode,
  DataReadMode: native.DataReadMode,
  DataWriteMode: native.DataWriteMode,
  ConsoleProgress: native.ConsoleProgress,
  ConsoleReply: native.ConsoleReply,
  LogReply: native.LogReply,
  CapabilitiesReply: native.CapabilitiesReply,
  FirmwareDownloadState: native.FirmwareDownloadState,
  FirmwareDownloadReply: native.FirmwareDownloadReply,
  DataWindow: native.DataWindow,
  CommandSequence: native.CommandSequence,
  InstallQueries: native.InstallQueries,
  OwnedUpload: native.OwnedUpload,
  OffsetMode: native.OffsetMode,
  memory: Object.freeze({
    readRange: native.readMemoryRange,
    writeRange: native.writeMemoryRange,
    readChunks(session: native.Session, address: number, length: number): ChunkIterator {
      return new ChunkIterator(native.memoryChunks(session, address, length));
    },
  }),
  data: Object.freeze({
    upload: native.uploadData,
    readChunks(
      session: native.Session,
      window: native.DataWindow,
      address: number,
      length: number,
    ): ChunkIterator {
      return new ChunkIterator(native.dataChunks(session, window, address, length));
    },
  }),
  app80: Object.freeze({
    ...serviceValues,
    CommandCode: native.App80CommandCode,
    Request: native.App80Request,
    Mailbox: native.Mailbox,
    exchange: native.exchangeApp80,
    Service: native.App80Service,
    CommandFailureKind: native.CommandFailureKind,
    isError: isServiceCommandError,
    execute: native.executeApp80,
  }),
  research: Object.freeze({
    CommandCode: native.ResearchCommandCode,
    Request: native.ResearchRequest,
    Reply: native.ResearchReply,
  }),
  tracing: Object.freeze({
    Layer: native.TraceLayer,
    Trace: native.Trace,
    Stage: native.TraceStage,
    Phase: native.TracePhase,
    RequestKind: native.TraceRequestKind,
    FailureKind: native.TraceFailureKind,
  }),
});

/** Make this initialized Rust module available to the FujiMod Python package. */
export function registerPyodide(runtime: {
  registerJsModule(name: string, module: object): void;
}): void {
  runtime.registerJsModule('_fujimod_js', { usb, catalog, valueSchemas });
}
