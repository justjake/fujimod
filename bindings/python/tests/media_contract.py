"""Media value contract shared by native Python and real Pyodide."""

from collections.abc import Callable

from contract_assertions import expect
from fujimod.usb import app80


def check_media_values() -> bool:
    import json

    from fujimod import catalog
    from fujimod.usb import app80 as a

    layouts = [
        ("display_option", 4),
        ("display_mode", 1),
        ("sound", 3),
        ("hdmi", 19),
        ("parameter_service", 1),
        ("charge_control", 3),
        ("charge_status", 2),
        ("audio_capture", 3),
        ("audio_playback", 2),
        ("erase_storage", 2),
        ("storage_status", 1),
        ("format_storage", 0),
        ("display_setup", 0),
    ]
    slots = {
        codec: [(n * 5 + 7) % 32 for n in range(count)] for codec, count in layouts
    }
    entries = [
        dict(
            code=210 + n,
            label=codec,
            codec=codec,
            slots=slots[codec],
            enabled_modes=4,
            next_mode=0,
            firmware_kind=1,
            handler=0,
            parameter_table=0,
        )
        for n, (codec, _) in enumerate(layouts)
    ]
    document = dict(
        schema=1,
        firmware=dict(id="media-v2", model="Synthetic", version="2", sha256="ab" * 32),
        mailbox=dict(page=3, offset=576, configuration_limit=512),
        entries=entries,
    )
    definition = catalog.service.Definition.from_json(
        json.dumps(document), catalog.FirmwareIdentity("media-v2", "ab" * 32)
    )
    service = a.Service(definition)
    modes = catalog.service.ServiceModes(4)

    def prepare(command: app80.Command) -> app80.Invocation:
        return service.prepare(modes, command)

    def logical(invocation: app80.Invocation, codec: str) -> list[int]:
        return [invocation.raw[1 + s] for s in slots[codec]]

    def decode(
        invocation: app80.Invocation, codec: str, index: int, value: int
    ) -> app80.ReplyValue:
        raw = bytearray(invocation.raw)
        raw[1 + slots[codec][index]] = value
        reply = invocation.decode(raw)
        assert reply.raw == raw
        return reply.value

    def invalid(function: Callable[[], object], kind: app80.ArgumentKind) -> None:
        try:
            function()
        except a.CommandError as error:
            assert (
                isinstance(error.reason, a.CommandFailure.Argument)
                and error.reason.kind == kind
            )
        else:
            raise AssertionError("invalid media quantity accepted")

    for command, codec, expected in [
        (
            a.Command.DisplayOption(action=a.DisplayOptionAction.IdlePins()),
            "display_option",
            [0, 0, 0, 0],
        ),
        (
            a.Command.DisplayOption(
                action=a.DisplayOptionAction.ForwardPair(first=7, second=8)
            ),
            "display_option",
            [0, 1, 7, 8],
        ),
        (a.Command.DisplayMode(mode=a.DisplayMode.Option0), "display_mode", [0]),
        (a.Command.DisplayMode(mode=a.DisplayMode.Option1), "display_mode", [1]),
        (
            a.Command.ParameterService(enabled=a.Toggle.Disabled),
            "parameter_service",
            [0],
        ),
        (
            a.Command.ParameterService(enabled=a.Toggle.Enabled),
            "parameter_service",
            [1],
        ),
    ]:
        invocation = prepare(command)
        assert logical(invocation, codec) == expected
        assert isinstance(
            invocation.decode(invocation.raw).value, a.ReplyValue.NoPayload
        )
    invalid(lambda: a.SoundClip(23), a.ArgumentKind.SoundClip)
    assert a.SoundClip(22).value == 22
    assert a.SoundClip(value=22).value == 22
    try:
        a.SoundClip(22, constructor=None)  # ty: ignore[unknown-argument]  # Deliberately invalid input: verify runtime rejection.
    except TypeError:
        pass
    else:
        raise AssertionError("generated constructor exposed a backend override")
    for format, wire in [
        (a.SoundFormat.Format1, 0),
        (a.SoundFormat.Format2, 1),
        (a.SoundFormat.Format3, 2),
    ]:
        for clip in [0, 22]:
            invocation = prepare(
                a.Command.Sound(
                    action=a.SoundAction.Clip(clip=a.SoundClip(clip), format=format)
                )
            )
            assert logical(invocation, "sound") == [1, clip, wire]
            assert isinstance(
                invocation.decode(invocation.raw).value, a.ReplyValue.NoPayload
            )
        for track, base in [
            (a.SoundTrack.Track2, 23),
            (a.SoundTrack.Track3, 26),
            (a.SoundTrack.Track4, 29),
        ]:
            for operation, offset in [
                (a.SoundOperation.First, 0),
                (a.SoundOperation.Second, 1),
                (a.SoundOperation.Third, 2),
            ]:
                invocation = prepare(
                    a.Command.Sound(
                        action=a.SoundAction.Track(
                            track=track, operation=operation, format=format
                        )
                    )
                )
                assert invocation.raw[0] == 212
                assert logical(invocation, "sound") == [1, base + offset, wire]
    assert logical(prepare(a.Command.Sound(action=a.SoundAction.Noop())), "sound") == [
        0,
        0,
        0,
    ]

    for length in [0, 15, 17]:
        invalid(
            lambda length=length: a.HdmiPacket(bytes(length)),
            a.ArgumentKind.HdmiPacketLength,
        )
    invalid(lambda: a.HdmiValue(4096), a.ArgumentKind.HdmiValue)
    assert a.HdmiValue(4095).value == 4095
    original = bytearray(range(16))
    packet = a.HdmiPacket(original)
    original[:] = bytes([255] * 16)
    assert packet.data == bytes(range(16))
    invocation = prepare(a.Command.Hdmi(action=a.HdmiAction.Packet(packet=packet)))
    assert logical(invocation, "hdmi") == [0, 1, 0] + list(reversed(range(16)))
    assert isinstance(invocation.decode(invocation.raw).value, a.ReplyValue.NoPayload)
    invocation = prepare(
        a.Command.Hdmi(
            action=a.HdmiAction.Triplet(
                first=a.HdmiValue(0x123),
                second=a.HdmiValue(0x456),
                third=a.HdmiValue(4095),
            )
        )
    )
    assert (
        logical(invocation, "hdmi") == [1, 0, 0, 1, 0x23, 4, 0x56, 15, 255] + [0] * 10
    )
    assert (
        logical(prepare(a.Command.Hdmi(action=a.HdmiAction.ResetStub())), "hdmi")
        == [0] * 19
    )

    for action, expected in [
        (a.ChargeAction.Control(enabled=a.Toggle.Disabled), [0, 0, 255]),
        (a.ChargeAction.Control(enabled=a.Toggle.Enabled), [0, 1, 255]),
        (a.ChargeAction.EnableAndSetFlag(enabled=a.Toggle.Disabled), [1, 0, 255]),
        (a.ChargeAction.EnableAndSetFlag(enabled=a.Toggle.Enabled), [1, 1, 255]),
    ]:
        invocation = prepare(a.Command.ChargeControl(action=action))
        assert logical(invocation, "charge_control") == expected
        assert isinstance(
            expect(
                invocation.decode(invocation.raw).value, a.ReplyValue.ChargeApplied
            ).status,
            a.ChargeResult.Unknown,
        )
        for raw, result in [
            (0, a.ChargeResult.Success),
            (1, a.ChargeResult.Failed),
            (42, a.ChargeResult.Unknown),
            (255, a.ChargeResult.Unknown),
        ]:
            reply = decode(invocation, "charge_control", 2, raw)
            assert isinstance(reply, a.ReplyValue.ChargeApplied) and isinstance(
                reply.status, result
            )
            if result is a.ChargeResult.Unknown:
                assert expect(reply.status, a.ChargeResult.Unknown).raw_status == raw
    invocation = prepare(a.Command.ChargeStatus(query=a.ChargeQuery.Presence))
    assert logical(invocation, "charge_status") == [0, 255]
    for raw, result in [
        (0, a.ChargePresence.Failed),
        (1, a.ChargePresence.Absent),
        (2, a.ChargePresence.Present),
        (3, a.ChargePresence.Unknown),
    ]:
        reply = decode(invocation, "charge_status", 1, raw)
        assert isinstance(reply, a.ReplyValue.ChargePresence) and isinstance(
            reply.status, result
        )
        if result is a.ChargePresence.Unknown:
            assert expect(reply.status, a.ChargePresence.Unknown).raw_status == raw
    invocation = prepare(a.Command.ChargeStatus(query=a.ChargeQuery.State))
    assert logical(invocation, "charge_status") == [1, 255]
    for raw, result in [
        (0, a.ReportedChargeState.Failed),
        (1, a.ReportedChargeState.Unknown),
        (2, a.ReportedChargeState.Unknown),
        (3, a.ReportedChargeState.Value0),
        (4, a.ReportedChargeState.Values2To5),
        (5, a.ReportedChargeState.Value6),
        (6, a.ReportedChargeState.OtherValue),
        (255, a.ReportedChargeState.Unknown),
    ]:
        reply = decode(invocation, "charge_status", 1, raw)
        assert isinstance(reply, a.ReplyValue.ChargeState) and isinstance(
            reply.status, result
        )
        if result is a.ReportedChargeState.Unknown:
            assert expect(reply.status, a.ReportedChargeState.Unknown).raw_status == raw
    try:
        a.Command.ParameterService(enabled=1)  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    except TypeError:
        pass
    else:
        raise AssertionError("integer toggle accepted")
    try:
        a.SoundAction.Clip(clip=a.SoundClip(0), format=a.DisplayMode.Option0)  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    except TypeError:
        pass
    else:
        raise AssertionError("wrong enum group accepted")
    invalid(lambda: a.AudioCount(11), a.ArgumentKind.AudioCount)
    invalid(lambda: a.AudioCount(255), a.ArgumentKind.AudioCount)
    for count in [0, 10]:
        assert a.AudioCount(count).value == count
        for route, wire in [(a.AudioRoute.Request82, 0), (a.AudioRoute.Request81, 1)]:
            invocation = prepare(
                a.Command.AudioCapture(count=a.AudioCount(count), route=route)
            )
            assert logical(invocation, "audio_capture") == [count, wire, 255]
            for raw, result in [
                (0, a.AudioResult.Success),
                (1, a.AudioResult.Failed),
                (42, a.AudioResult.Unknown),
                (255, a.AudioResult.Unknown),
            ]:
                reply = decode(invocation, "audio_capture", 2, raw)
                assert isinstance(reply, a.ReplyValue.AudioCaptured) and isinstance(
                    reply.status, result
                )
                if result is a.AudioResult.Unknown:
                    assert expect(reply.status, a.AudioResult.Unknown).raw_status == raw
    for mode, wire in [
        (a.AudioPlayback.AllLanes, 0),
        (a.AudioPlayback.ZeroSecondLane, 1),
        (a.AudioPlayback.ZeroFirstLane, 2),
    ]:
        invocation = prepare(a.Command.AudioPlayback(mode=mode))
        assert logical(invocation, "audio_playback") == [wire, 255]
        for raw, result in [
            (0, a.AudioResult.Success),
            (1, a.AudioResult.Failed),
            (255, a.AudioResult.Unknown),
        ]:
            reply = decode(invocation, "audio_playback", 1, raw)
            assert isinstance(reply, a.ReplyValue.AudioPlayed) and isinstance(
                reply.status, result
            )
    for action, wire in [
        (a.StorageErase.Kind1Regions, 1),
        (a.StorageErase.Kind0And2Regions, 2),
        (a.StorageErase.Kind0And1And2Regions, 3),
        (a.StorageErase.AuxiliaryRegion, 4),
        (a.StorageErase.ResetProfiles, 5),
    ]:
        invocation = prepare(a.Command.EraseStorage(action=action))
        assert logical(invocation, "erase_storage") == [wire, 255]
        for raw, result in [
            (0, a.StorageEraseResult.Success),
            (1, a.StorageEraseResult.Failed),
            (42, a.StorageEraseResult.Unknown),
            (255, a.StorageEraseResult.Unknown),
        ]:
            reply = decode(invocation, "erase_storage", 1, raw)
            assert isinstance(reply, a.ReplyValue.StorageErased) and isinstance(
                reply.status, result
            )
            if result is a.StorageEraseResult.Unknown:
                assert (
                    expect(reply.status, a.StorageEraseResult.Unknown).raw_status == raw
                )
    invocation = prepare(a.Command.StorageStatus())
    assert logical(invocation, "storage_status") == [255]
    for raw, result in [
        (0, a.StorageStatus.Zero),
        (4, a.StorageStatus.Value4),
        (8, a.StorageStatus.Value8),
        (16, a.StorageStatus.Value16),
        (32, a.StorageStatus.Value32),
        (1, a.StorageStatus.Unknown),
        (255, a.StorageStatus.Unknown),
    ]:
        reply = decode(invocation, "storage_status", 0, raw)
        assert isinstance(reply, a.ReplyValue.StorageStatus) and isinstance(
            reply.status, result
        )
        if result is a.StorageStatus.Unknown:
            assert expect(reply.status, a.StorageStatus.Unknown).raw_status == raw
    for command, codec in [
        (a.Command.FormatStorage(), "format_storage"),
        (a.Command.DisplaySetup(), "display_setup"),
    ]:
        invocation = prepare(command)
        assert logical(invocation, codec) == []
        assert invocation.raw[1:] == bytes(32)
        assert isinstance(
            invocation.decode(invocation.raw).value, a.ReplyValue.NoPayload
        )
    for create in [
        lambda: a.Command.AudioCapture(count=1, route=a.AudioRoute.Request81),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        lambda: a.Command.AudioPlayback(mode=a.DisplayMode.Option0),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        lambda: a.Command.EraseStorage(action=1),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    ]:
        try:
            create()
        except TypeError:
            pass
        else:
            raise AssertionError("untyped audio/storage choice accepted")
    return True
