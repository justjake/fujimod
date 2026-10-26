import { usb, type UsbError, type Session, type Device, type InfoReply } from '../src/index.js';
import type { LensClockMode, PartAttachment, StabilizerCommand, BodyProfile } from '../pkg/fujimod.js';
declare const lensClock: LensClockMode;
if (lensClock.kind === 'divided') {
  const periods: number[] = lensClock.period_picoseconds;
  // @ts-expect-error Divided clocks have no stage quantum.
  void lensClock.ticks_per_quantum;
  void periods;
}
declare const attachment: PartAttachment;
const homePin: number | undefined = attachment.motor_home_pins?.[0]?.pin;
declare const bodyProfile: BodyProfile;
const constructionSchema: number | undefined = bodyProfile.construction?.schema;
const stabilizer: StabilizerCommand = { code: 1, bits: 8, operation: { kind: 'ram_write', bank: 2 } };
// @ts-expect-error Stabilizer command codes are numeric hardware values.
const invalidStabilizer: StabilizerCommand = { code: '1', bits: 8, operation: { kind: 'write' } };
void homePin;
void constructionSchema;
void stabilizer;
void invalidStabilizer;
declare const session: Session;
declare const device: Device;
const bytes: Promise<Uint8Array> = session.readMemory(0x1000, 4);
const info: Promise<InfoReply> = session.deviceInfo(usb.InfoSelector.Product);
const opened: Promise<Session> = usb.Session.open(
  device,
  new usb.InterfaceSelection(1, 0, 0),
  new usb.FirmwareLimits(256, 256, 256, 256),
  new usb.TransferOptions(1000, 1024),
);
type NotAny<T> = 0 extends 1 & T ? never : T;
async function inspect(): Promise<void> {
  const reply = await info;
  const raw: NotAny<typeof reply.raw> = reply.raw;
  const array: Uint8Array = raw;
  for await (const part of usb.memory.readChunks(session, 0x1000, 8)) {
    const output: Uint8Array = part;
    void output;
  }
  void array;
}
function errors(value: unknown): void {
  if (usb.isError(value)) {
    const error: UsbError = value;
    const kind = error.kind;
    void kind;
  }
}
// @ts-expect-error Each choice group has its own enum type.
void session.deviceInfo(usb.DataReadMode.Echo);
// @ts-expect-error Protocol choices are enums, not strings.
void session.readConfiguration('Ram', 0, 0, 1);
// @ts-expect-error Sessions come from explicit opens.
new usb.Session();
// @ts-expect-error Observational state cannot be assigned.
session.state = usb.SessionState.Ready;
void bytes;
void opened;
void inspect;
void errors;

import { catalog, type App80CommandReply, type App80ReplyValue } from '../src/index.js';
const selected = catalog.service.Definition.fromJson(
  '{}',
  new catalog.FirmwareIdentity('body-v1', 'ab'.repeat(32)),
);
const service = new usb.app80.Service(selected);
const invocation = service.prepare(
  new catalog.service.ServiceModes(4),
  usb.app80.Command.readPins(new usb.app80.PinBank(0)),
);
const typedReply: Promise<App80CommandReply> = usb.app80.execute(session, invocation);
async function serviceValues(): Promise<void> {
  const reply = await typedReply;
  const value: NotAny<App80ReplyValue> = reply.value;
  if (value.kind === usb.app80.ReplyKind.Pins) {
    const levels: number = value.levels;
    // @ts-expect-error Pin replies do not contain save status.
    void value.status;
    // @ts-expect-error Result variants are immutable.
    value.levels = 3;
    void levels;
  }
  if (
    value.kind === usb.app80.ReplyKind.ConfigurationSaved &&
    value.status.kind === usb.app80.SaveStatusKind.Unknown
  ) {
    const raw: number = value.status.rawStatus;
    void raw;
  }
}
function serviceErrors(error: unknown): void {
  if (usb.app80.isError(error) && error.reason.kind === usb.app80.CommandFailureKind.Unsupported) {
    const codec = error.reason.codec;
    void codec;
    // @ts-expect-error Unsupported has a codec, not an argument.
    void error.reason.argument;
  }
  if (catalog.isError(error) && error.reason.kind === catalog.DefinitionFailureKind.InvalidLayout) {
    const code: number = error.reason.code;
    void code;
  }
}
// @ts-expect-error Checked pin-bank objects are required.
usb.app80.Command.readPins(0);
// @ts-expect-error Modes are a distinct checked quantity.
service.prepare(4, usb.app80.Command.flushConfiguration());
// @ts-expect-error Command variants have named factories, not a raw constructor.
new usb.app80.Command();
// @ts-expect-error Profile choices require the right enum group.
usb.app80.ProfileAction.read(usb.DataReadMode.Echo, new usb.app80.ProfileIndex(0));
void serviceValues;
void serviceErrors;

const peripheral = usb.app80.Command.i2c(
  usb.app80.I2cAction.read(usb.app80.I2cPort.Port0, 0xa5, new usb.app80.I2cLength(3)),
);
// @ts-expect-error The write-only single-byte route is not a read port.
void usb.app80.I2cPort.Port3;
// @ts-expect-error The length is a checked value, not an unchecked number.
usb.app80.I2cAction.read(usb.app80.I2cPort.Port0, 1, 3);
// @ts-expect-error Port enum groups remain distinct from other choices.
usb.app80.I2cAction.read(usb.app80.ProfileBank.First, 1, new usb.app80.I2cLength(1));
void peripheral;

// @ts-expect-error HDMI quantities must be checked value objects.
usb.app80.HdmiAction.triplet(1, 2, 3);
// @ts-expect-error Toggle choices are enums, not booleans.
usb.app80.ChargeAction.control(true);
// @ts-expect-error Sound formats cannot be substituted with a display mode enum.
usb.app80.SoundAction.clip(new usb.app80.SoundClip(0), usb.app80.DisplayMode.Option0);
async function chargeValues(): Promise<void> {
  const value = (await typedReply).value;
  if (value.kind === usb.app80.ReplyKind.ChargePresence) {
    if (value.status.kind === usb.app80.ChargePresenceKind.Unknown) {
      const raw: number = value.status.rawStatus;
      void raw;
    }
    // @ts-expect-error Presence results do not have a numeric state value field.
    void value.status.value;
  }
}
void chargeValues;

// @ts-expect-error Audio count must be checked by Rust.
usb.app80.Command.audioCapture(10, usb.app80.AudioRoute.Request82);
// @ts-expect-error Playback lane choices cannot be supplied as display mode choices.
usb.app80.Command.audioPlayback(usb.app80.DisplayMode.Option0);
// @ts-expect-error Storage erase selectors are enum choices, not booleans.
usb.app80.Command.eraseStorage(true);
async function storageValues(): Promise<void> {
  const value = (await typedReply).value;
  if (value.kind === usb.app80.ReplyKind.StorageStatus) {
    if (value.status.kind === usb.app80.StorageStatusKind.Unknown) {
      const raw: number = value.status.rawStatus;
      void raw;
    }
    // @ts-expect-error The ambiguous zero result is not a boolean success result.
    const success: boolean = value.status;
    void success;
  }
}
void storageValues;

// @ts-expect-error Radio channels must be checked quantity objects.
usb.app80.RadioAction.secondary(usb.app80.RadioMode.Mode0, 1);
// @ts-expect-error Radio mode is a different enum group from a toggle.
usb.app80.RadioAction.secondary(usb.app80.Toggle.Enabled, new usb.app80.RadioChannel(1));
// @ts-expect-error NAND spare choice is not a boolean.
usb.app80.Command.nandReadBlocks(true, 0);
// @ts-expect-error A peer address must be an owned six-byte value.
usb.app80.Command.signalStrength(new Uint8Array(6));
async function networkValues(): Promise<void> {
  const value = (await typedReply).value;
  if (value.kind === usb.app80.ReplyKind.NandRead) {
    if (value.status.kind === usb.app80.NandReadResultKind.Unknown) {
      const raw: number = value.status.rawStatus;
      void raw;
    }
    // @ts-expect-error NAND result is not a raw integer or host disposition.
    const status: number = value.status;
    void status;
  }
  if (value.kind === usb.app80.ReplyKind.SignalStrength) {
    const raw: number = value.rawValue;
    void raw;
  }
}
void networkValues;

// @ts-expect-error WPS requires an exact WPS peer, which excludes the broadcast sentinel.
usb.app80.WpsTarget.peer(new usb.app80.PeerAddress(new Uint8Array(6)));
// @ts-expect-error Station scan has its own selector enum.
usb.app80.StationAction.scan(usb.app80.Toggle.Enabled);
// @ts-expect-error WPS actions cannot be replaced by station actions.
usb.app80.Command.wps(usb.app80.StationAction.stop());
async function wpsValues(): Promise<void> {
  const value = (await typedReply).value;
  if (value.kind === usb.app80.ReplyKind.WpsProgress) {
    if (value.snapshot.kind === usb.app80.WpsProgressKind.State) {
      const phase: import('../src/index.js').WpsPhase = value.snapshot.phase;
      const connection: import('../src/index.js').ConnectionState = value.snapshot.connection;
      void phase;
      void connection;
    }
  }
}
void wpsValues;

// @ts-expect-error Transfer operation selectors are not integer quantities.
usb.app80.Command.transferSelectOperation(123);
// @ts-expect-error Channel and operation are distinct selector groups.
usb.app80.Command.transferSelectChannel(usb.app80.TransferOperation.Operation1);
usb.app80.NetworkReconfigureAction.apply(
  0,
  new usb.app80.RadioChannel(1),
  // @ts-expect-error AP radio mode is distinct from the direct-radio selector.
  usb.app80.RadioMode.Mode0,
  usb.app80.AccessPointSecurity.Option0,
  new usb.app80.CountryBytes(new Uint8Array(2)),
);

// @ts-expect-error CPU memory word addresses do not belong to the transfer word space.
usb.app80.Command.transferReadWord(new usb.app80.WordAddress(4));
// @ts-expect-error The full-width halfword space is distinct from the bounded word space.
usb.app80.Command.transferReadHalfword(new usb.app80.TransferWordAddress(4));
// @ts-expect-error Option selectors are enum choices, not arbitrary bytes.
usb.app80.Command.transferReadOptionWord(99);
// @ts-expect-error Different selected-word command groups remain distinct.
usb.app80.Command.transferSetWord7(usb.app80.TransferWord16Kind.Kind16, 0);

usb.app80.Command.readConfigurationFlags(
  // @ts-expect-error Configuration mode and bank are different selector groups.
  usb.app80.ConfigurationMode.Mode1,
  usb.app80.Toggle.Enabled,
);
// @ts-expect-error Unknown configuration flag discriminants are rejected.
usb.app80.Command.selectConfigurationMode(255, usb.app80.ConfigurationMode.Mode0);

// @ts-expect-error Battery actions have their own enum choices.
usb.app80.Command.batteryControl(usb.app80.Toggle.Enabled);
// @ts-expect-error Display patterns require a typed action factory.
usb.app80.Command.displayPattern(usb.app80.BatteryAction.ResetPending);

// @ts-expect-error Service adjustment modes differ from configuration selectors.
usb.app80.Command.selectAdjustmentMode(usb.app80.ConfigurationMode.Mode0);
usb.app80.AdjustmentConfigurationAction.apply(
  usb.app80.AdjustmentProfile.Profile0,
  // @ts-expect-error A lens index cannot substitute for a duration despite identical Wasm getters.
  new usb.app80.LensSelectionIndex(1),
);
// @ts-expect-error A lens index must use its checked type, not an arbitrary quantity.
usb.app80.LensTarget.index(new usb.app80.TransferPayloadByte(1));

// @ts-expect-error Opaque option bytes have a distinct checked handle type.
usb.app80.Command.queryAdjustmentOption(new usb.app80.TransferPayloadByte(1));
