"""Nested App80 wire types and the Rust mailbox composition."""

from collections.abc import Buffer
from typing import TYPE_CHECKING

from .._bridge import Handle, buffer_in, buffer_out, call, jsusb, uint, unwrap
from . import Session

if TYPE_CHECKING:
    pass


class CommandCode(Handle["JsApp80CommandCode"]):
    def __init__(self, value: int) -> None:
        self._js = call(jsusb.app80.CommandCode.new, uint(value, 8))

    @property
    def value(self) -> int:
        return self._js.value

    def __eq__(self, other: object) -> bool:
        return type(other) is type(self) and self.value == other.value

    def __hash__(self) -> int:
        return hash((type(self), self.value))


class Request(Handle["JsApp80Request"]):
    def __init__(self, code: CommandCode, arguments: Buffer) -> None:
        self._js = call(
            jsusb.app80.Request.new, unwrap(code, CommandCode), buffer_in(arguments)
        )

    @property
    def raw(self) -> bytes:
        return buffer_out(self._js.raw)


class Mailbox(Handle["JsMailbox"]):
    def __init__(self, *, page: int, offset: int, configuration_limit: int) -> None:
        self._js = call(
            jsusb.app80.Mailbox.new, uint(page), uint(offset), uint(configuration_limit)
        )


async def exchange(session: Session, mailbox: Mailbox, request: Request) -> bytes:
    return await session._call(
        jsusb.app80.exchange,
        buffer_out,
        unwrap(session, Session),
        unwrap(mailbox, Mailbox),
        unwrap(request, Request),
    )


# The catalog uses CommandCode; finish the wire types before importing it.
from ._catalog import (  # noqa: E402
    CommandError,
    CommandFailure,
    CommandReply,
    Invocation,
    Service,
    _value_types,
    execute,
)

__all__ = [
    "CommandCode",
    "Request",
    "Mailbox",
    "exchange",
    "CommandError",
    "CommandFailure",
    "CommandReply",
    "Invocation",
    "Service",
    "execute",
]

globals().update(_value_types)
__all__ += list(_value_types)

if TYPE_CHECKING:
    from _fujimod_service_values import (
        AccessPointRadioMode as AccessPointRadioMode,
    )
    from _fujimod_service_values import (
        AccessPointSecurity as AccessPointSecurity,
    )
    from _fujimod_service_values import (
        AccumulatorAction as AccumulatorAction,
    )
    from _fujimod_service_values import (
        AdjustmentConfigurationAction as AdjustmentConfigurationAction,
    )
    from _fujimod_service_values import (
        AdjustmentConfigurationState as AdjustmentConfigurationState,
    )
    from _fujimod_service_values import (
        AdjustmentControlAction as AdjustmentControlAction,
    )
    from _fujimod_service_values import (
        AdjustmentDuration as AdjustmentDuration,
    )
    from _fujimod_service_values import (
        AdjustmentMode as AdjustmentMode,
    )
    from _fujimod_service_values import (
        AdjustmentOptionByte as AdjustmentOptionByte,
    )
    from _fujimod_service_values import (
        AdjustmentOptionStatus as AdjustmentOptionStatus,
    )
    from _fujimod_service_values import (
        AdjustmentPacketAction as AdjustmentPacketAction,
    )
    from _fujimod_service_values import (
        AdjustmentPacketPayload2 as AdjustmentPacketPayload2,
    )
    from _fujimod_service_values import (
        AdjustmentPacketPayload3 as AdjustmentPacketPayload3,
    )
    from _fujimod_service_values import (
        AdjustmentPacketPayload8 as AdjustmentPacketPayload8,
    )
    from _fujimod_service_values import (
        AdjustmentPacketRead as AdjustmentPacketRead,
    )
    from _fujimod_service_values import (
        AdjustmentPacketReadData as AdjustmentPacketReadData,
    )
    from _fujimod_service_values import (
        AdjustmentPacketWrite as AdjustmentPacketWrite,
    )
    from _fujimod_service_values import (
        AdjustmentProfile as AdjustmentProfile,
    )
    from _fujimod_service_values import (
        ArgumentKind as ArgumentKind,
    )
    from _fujimod_service_values import (
        AudioCount as AudioCount,
    )
    from _fujimod_service_values import (
        AudioPlayback as AudioPlayback,
    )
    from _fujimod_service_values import (
        AudioResult as AudioResult,
    )
    from _fujimod_service_values import (
        AudioRoute as AudioRoute,
    )
    from _fujimod_service_values import (
        BatteryAction as BatteryAction,
    )
    from _fujimod_service_values import (
        ChargeAction as ChargeAction,
    )
    from _fujimod_service_values import (
        ChargePresence as ChargePresence,
    )
    from _fujimod_service_values import (
        ChargeQuery as ChargeQuery,
    )
    from _fujimod_service_values import (
        ChargeResult as ChargeResult,
    )
    from _fujimod_service_values import (
        Command as Command,
    )
    from _fujimod_service_values import (
        ConfigurationBank as ConfigurationBank,
    )
    from _fujimod_service_values import (
        ConfigurationMode as ConfigurationMode,
    )
    from _fujimod_service_values import (
        ConnectionState as ConnectionState,
    )
    from _fujimod_service_values import (
        CountryBytes as CountryBytes,
    )
    from _fujimod_service_values import (
        Device9Action as Device9Action,
    )
    from _fujimod_service_values import (
        Device9Count as Device9Count,
    )
    from _fujimod_service_values import (
        Device9Payload as Device9Payload,
    )
    from _fujimod_service_values import (
        DisplayMode as DisplayMode,
    )
    from _fujimod_service_values import (
        DisplayOptionAction as DisplayOptionAction,
    )
    from _fujimod_service_values import (
        DisplayPatternAction as DisplayPatternAction,
    )
    from _fujimod_service_values import (
        ExposureDuration as ExposureDuration,
    )
    from _fujimod_service_values import (
        ExposureModeOption as ExposureModeOption,
    )
    from _fujimod_service_values import (
        ExposureOption as ExposureOption,
    )
    from _fujimod_service_values import (
        ExposureReadiness as ExposureReadiness,
    )
    from _fujimod_service_values import (
        ExposureStatusOption as ExposureStatusOption,
    )
    from _fujimod_service_values import (
        HdmiAction as HdmiAction,
    )
    from _fujimod_service_values import (
        HdmiPacket as HdmiPacket,
    )
    from _fujimod_service_values import (
        HdmiValue as HdmiValue,
    )
    from _fujimod_service_values import (
        I2cAction as I2cAction,
    )
    from _fujimod_service_values import (
        I2cLength as I2cLength,
    )
    from _fujimod_service_values import (
        I2cPayload as I2cPayload,
    )
    from _fujimod_service_values import (
        I2cPort as I2cPort,
    )
    from _fujimod_service_values import (
        LensCompletionParameterFlag as LensCompletionParameterFlag,
    )
    from _fujimod_service_values import (
        LensPositionRange as LensPositionRange,
    )
    from _fujimod_service_values import (
        LensSelectionIndex as LensSelectionIndex,
    )
    from _fujimod_service_values import (
        LensSelectionRange as LensSelectionRange,
    )
    from _fujimod_service_values import (
        LensTarget as LensTarget,
    )
    from _fujimod_service_values import (
        NandReadResult as NandReadResult,
    )
    from _fujimod_service_values import (
        NandSpare as NandSpare,
    )
    from _fujimod_service_values import (
        NetworkReconfigureAction as NetworkReconfigureAction,
    )
    from _fujimod_service_values import (
        Option41State as Option41State,
    )
    from _fujimod_service_values import (
        PeerAddress as PeerAddress,
    )
    from _fujimod_service_values import (
        PinBank as PinBank,
    )
    from _fujimod_service_values import (
        PipelineAction as PipelineAction,
    )
    from _fujimod_service_values import (
        PipelineDispatchOption as PipelineDispatchOption,
    )
    from _fujimod_service_values import (
        PipelineDispatchSelector as PipelineDispatchSelector,
    )
    from _fujimod_service_values import (
        PipelinePreparationMode as PipelinePreparationMode,
    )
    from _fujimod_service_values import (
        PipelineRequestMode as PipelineRequestMode,
    )
    from _fujimod_service_values import (
        PipelineValue as PipelineValue,
    )
    from _fujimod_service_values import (
        ProfileAction as ProfileAction,
    )
    from _fujimod_service_values import (
        ProfileBank as ProfileBank,
    )
    from _fujimod_service_values import (
        ProfileIndex as ProfileIndex,
    )
    from _fujimod_service_values import (
        RadioAction as RadioAction,
    )
    from _fujimod_service_values import (
        RadioChannel as RadioChannel,
    )
    from _fujimod_service_values import (
        RadioMode as RadioMode,
    )
    from _fujimod_service_values import (
        RadioPower as RadioPower,
    )
    from _fujimod_service_values import (
        RegisterAction as RegisterAction,
    )
    from _fujimod_service_values import (
        ReplyValue as ReplyValue,
    )
    from _fujimod_service_values import (
        ReportedChargeState as ReportedChargeState,
    )
    from _fujimod_service_values import (
        SaveStatus as SaveStatus,
    )
    from _fujimod_service_values import (
        ScanSelection as ScanSelection,
    )
    from _fujimod_service_values import (
        SoundAction as SoundAction,
    )
    from _fujimod_service_values import (
        SoundClip as SoundClip,
    )
    from _fujimod_service_values import (
        SoundFormat as SoundFormat,
    )
    from _fujimod_service_values import (
        SoundOperation as SoundOperation,
    )
    from _fujimod_service_values import (
        SoundTrack as SoundTrack,
    )
    from _fujimod_service_values import (
        StationAction as StationAction,
    )
    from _fujimod_service_values import (
        StationScanMode as StationScanMode,
    )
    from _fujimod_service_values import (
        StorageErase as StorageErase,
    )
    from _fujimod_service_values import (
        StorageEraseResult as StorageEraseResult,
    )
    from _fujimod_service_values import (
        StorageStatus as StorageStatus,
    )
    from _fujimod_service_values import (
        Toggle as Toggle,
    )
    from _fujimod_service_values import (
        TransferChannel as TransferChannel,
    )
    from _fujimod_service_values import (
        TransferHalfwordAddress as TransferHalfwordAddress,
    )
    from _fujimod_service_values import (
        TransferLoadMode as TransferLoadMode,
    )
    from _fujimod_service_values import (
        TransferOperation as TransferOperation,
    )
    from _fujimod_service_values import (
        TransferOption as TransferOption,
    )
    from _fujimod_service_values import (
        TransferPairFirst as TransferPairFirst,
    )
    from _fujimod_service_values import (
        TransferPairResult as TransferPairResult,
    )
    from _fujimod_service_values import (
        TransferPayloadByte as TransferPayloadByte,
    )
    from _fujimod_service_values import (
        TransferPrepareResult as TransferPrepareResult,
    )
    from _fujimod_service_values import (
        TransferResult as TransferResult,
    )
    from _fujimod_service_values import (
        TransferSelectedWordKind as TransferSelectedWordKind,
    )
    from _fujimod_service_values import (
        TransferWord7Kind as TransferWord7Kind,
    )
    from _fujimod_service_values import (
        TransferWord16Kind as TransferWord16Kind,
    )
    from _fujimod_service_values import (
        TransferWordAddress as TransferWordAddress,
    )
    from _fujimod_service_values import (
        TransferWordReadResult as TransferWordReadResult,
    )
    from _fujimod_service_values import (
        WordAddress as WordAddress,
    )
    from _fujimod_service_values import (
        WordReadResult as WordReadResult,
    )
    from _fujimod_service_values import (
        WordWriteResult as WordWriteResult,
    )
    from _fujimod_service_values import (
        WpsAction as WpsAction,
    )
    from _fujimod_service_values import (
        WpsCallbackResult as WpsCallbackResult,
    )
    from _fujimod_service_values import (
        WpsPeerAddress as WpsPeerAddress,
    )
    from _fujimod_service_values import (
        WpsPhase as WpsPhase,
    )
    from _fujimod_service_values import (
        WpsProgress as WpsProgress,
    )
    from _fujimod_service_values import (
        WpsTarget as WpsTarget,
    )


if TYPE_CHECKING:
    from _fujimod_js import JsApp80CommandCode as JsApp80CommandCode
    from _fujimod_js import JsApp80Request as JsApp80Request
    from _fujimod_js import JsMailbox as JsMailbox
