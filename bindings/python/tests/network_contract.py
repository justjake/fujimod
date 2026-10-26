"""Shared native/Pyodide network and NAND command value contract."""

from collections.abc import Callable

from contract_assertions import expect
from fujimod.usb import app80


def check_network_values() -> bool:
    import json

    from fujimod import catalog
    from fujimod.usb import app80 as a

    layouts = [
        ("nand_read_blocks", 3),
        ("option41", 1),
        ("wlan_control", 1),
        ("radio_control", 4),
        ("signal_strength", 7),
        ("iperf", 1),
        ("wps", 8),
        ("station", 4),
        ("network_reconfigure", 14),
    ]
    slots = {name: [(i * 5 + 7) % 32 for i in range(count)] for name, count in layouts}
    entries = [
        dict(
            code=200 + n,
            label=name,
            codec=name,
            slots=slots[name],
            enabled_modes=4,
            next_mode=0,
            firmware_kind=1,
            handler=0,
            parameter_table=0,
        )
        for n, (name, _) in enumerate(layouts)
    ]
    doc = dict(
        schema=1,
        firmware=dict(id="radio-b", model="Synthetic", version="2", sha256="ab" * 32),
        mailbox=dict(page=3, offset=576, configuration_limit=512),
        entries=entries,
    )
    service = a.Service(
        catalog.service.Definition.from_json(
            json.dumps(doc), catalog.FirmwareIdentity("radio-b", "ab" * 32)
        )
    )
    modes = catalog.service.ServiceModes(4)

    def prepare(command: app80.Command) -> app80.Invocation:
        return service.prepare(modes, command)

    def logical(p: app80.Invocation, codec: str) -> list[int]:
        return [p.raw[1 + i] for i in slots[codec]]

    def decode(
        p: app80.Invocation, codec: str, index: int, raw: int
    ) -> app80.ReplyValue:
        buf = bytearray(p.raw)
        buf[1 + slots[codec][index]] = raw
        reply = p.decode(buf)
        assert reply.raw == buf
        return reply.value

    def invalid(create: Callable[[], object], kind: app80.ArgumentKind) -> None:
        try:
            create()
        except a.CommandError as error:
            assert (
                isinstance(error.reason, a.CommandFailure.Argument)
                and error.reason.kind == kind
            )
        else:
            raise AssertionError("invalid network quantity accepted")

    invalid(lambda: a.RadioChannel(15), a.ArgumentKind.RadioChannel)
    invalid(lambda: a.RadioPower(128), a.ArgumentKind.RadioPower)
    for size in [0, 5, 7]:
        invalid(
            lambda size=size: a.PeerAddress(bytes(size)),
            a.ArgumentKind.PeerAddressLength,
        )
    for mode, wire in [(a.RadioMode.Mode0, 0), (a.RadioMode.Mode1, 1)]:
        for channel in [0, 14]:
            assert a.RadioChannel(channel).value == channel
            for power in [0, 11, 12, 127]:
                assert a.RadioPower(power).value == power
                for option, bit in [(a.Toggle.Disabled, 0), (a.Toggle.Enabled, 128)]:
                    p = prepare(
                        a.Command.RadioControl(
                            action=a.RadioAction.Primary(
                                mode=mode,
                                channel=a.RadioChannel(channel),
                                power=a.RadioPower(power),
                                option=option,
                            )
                        )
                    )
                    assert p.raw[0] == 203 and logical(p, "radio_control") == [
                        wire,
                        channel,
                        power | bit,
                        0,
                    ]
                    assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
        p = prepare(
            a.Command.RadioControl(
                action=a.RadioAction.Secondary(mode=mode, channel=a.RadioChannel(14))
            )
        )
        assert logical(p, "radio_control") == [wire, 14, 0, 1]
    for enabled, wire in [(a.Toggle.Disabled, 0), (a.Toggle.Enabled, 1)]:
        p = prepare(a.Command.WlanControl(enabled=enabled))
        assert logical(p, "wlan_control") == [wire]
        assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
    for spare, wire in [(a.NandSpare.Omit, 0), (a.NandSpare.Include, 1)]:
        for block in [0, 255]:
            p = prepare(a.Command.NandReadBlocks(spare=spare, block_group=block))
            assert logical(p, "nand_read_blocks") == [wire, block, 255]
            for raw, variant in [
                (0, a.NandReadResult.Success),
                (1, a.NandReadResult.RejectedOrWaitError),
                (2, a.NandReadResult.OtherError),
                (42, a.NandReadResult.Unknown),
                (255, a.NandReadResult.Unknown),
            ]:
                value = decode(p, "nand_read_blocks", 2, raw)
                assert isinstance(value, a.ReplyValue.NandRead) and isinstance(
                    value.status, variant
                )
                if variant is a.NandReadResult.Unknown:
                    assert (
                        expect(value.status, a.NandReadResult.Unknown).raw_status == raw
                    )
    p = prepare(a.Command.Option41())
    assert logical(p, "option41") == [255]
    for raw, variant in [
        (0, a.Option41State.NonZero),
        (1, a.Option41State.Zero),
        (2, a.Option41State.Unknown),
        (255, a.Option41State.Unknown),
    ]:
        value = decode(p, "option41", 0, raw)
        assert isinstance(value, a.ReplyValue.Option41) and isinstance(
            value.state, variant
        )
        if variant is a.Option41State.Unknown:
            assert expect(value.state, a.Option41State.Unknown).raw_status == raw
    data = bytearray(range(6))
    address = a.PeerAddress(data)
    data[:] = bytes(6)
    assert address.data == bytes(range(6))
    p = prepare(a.Command.SignalStrength(address=address))
    assert logical(p, "signal_strength") == list(range(6)) + [255]
    iperf = prepare(a.Command.Iperf())
    assert logical(iperf, "iperf") == [255]
    for raw in [0, 1, 128, 255]:
        value = decode(p, "signal_strength", 6, raw)
        assert isinstance(value, a.ReplyValue.SignalStrength) and value.raw_value == raw
        value = decode(iperf, "iperf", 0, raw)
        assert isinstance(value, a.ReplyValue.Iperf) and value.raw_result == raw
    for create in [
        lambda: a.Command.NandReadBlocks(spare=1, block_group=0),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        lambda: a.RadioAction.Secondary(
            mode=a.Toggle.Enabled,  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
            channel=a.RadioChannel(1),
        ),
        lambda: a.RadioAction.Primary(
            mode=a.RadioMode.Mode0,
            channel=1,  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
            power=a.RadioPower(1),
            option=a.Toggle.Enabled,
        ),
        lambda: a.Command.WlanControl(enabled=1),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        lambda: a.Command.SignalStrength(address=b"123456"),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    ]:
        try:
            create()
        except TypeError:
            pass
        else:
            raise AssertionError("untyped network selector accepted")
    for data in [b"", b"12345", b"1234567", bytes(range(6))]:
        invalid(lambda data=data: a.WpsPeerAddress(data), a.ArgumentKind.WpsPeerAddress)
    data = bytearray([2, 1, 2, 3, 4, 5])
    peer = a.WpsPeerAddress(data)
    data[:] = bytes(6)
    assert peer.data == bytes([2, 1, 2, 3, 4, 5])
    for target, expected in [
        (a.WpsTarget.Broadcast(), [1, 0, 0, 0, 0, 0, 0, 255]),
        (a.WpsTarget.Peer(address=peer), [1, 2, 1, 2, 3, 4, 5, 255]),
    ]:
        p = prepare(a.Command.Wps(action=a.WpsAction.Start(target=target)))
        assert logical(p, "wps") == expected
        for raw, variant in [
            (0, a.WpsCallbackResult.Success),
            (1, a.WpsCallbackResult.Timeout),
            (2, a.WpsCallbackResult.Failed),
            (3, a.WpsCallbackResult.Overlap),
            (255, a.WpsCallbackResult.Unknown),
        ]:
            value = decode(p, "wps", 7, raw)
            assert isinstance(value, a.ReplyValue.WpsCallback)
            assert isinstance(value.snapshot, variant)
            if variant is a.WpsCallbackResult.Unknown:
                assert (
                    expect(value.snapshot, a.WpsCallbackResult.Unknown).raw_status
                    == raw
                )
    for action, code in [(a.WpsAction.Stop(), 0), (a.WpsAction.Connect(), 2)]:
        p = prepare(a.Command.Wps(action=action))
        assert logical(p, "wps") == [code, 0, 0, 0, 0, 0, 0, 0]
        assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
    p = prepare(a.Command.Wps(action=a.WpsAction.PublishProgress()))
    assert logical(p, "wps") == [3, 0, 0, 0, 0, 0, 0, 255]
    phases = [
        a.WpsPhase.Idle,
        a.WpsPhase.PushButton,
        a.WpsPhase.Pin,
        a.WpsPhase.CredentialsReceived,
        a.WpsPhase.Timeout,
        a.WpsPhase.Failed,
        a.WpsPhase.Overlap,
        a.WpsPhase.Error,
    ]
    for raw in range(256):
        value = decode(p, "wps", 7, raw)
        assert isinstance(value, a.ReplyValue.WpsProgress)
        if raw & 15 < 8 and raw >> 4 < 2:
            assert isinstance(value.snapshot, a.WpsProgress.State)
            assert value.snapshot.phase == phases[raw & 15]
            assert value.snapshot.connection == (
                a.ConnectionState.Connected
                if raw >> 4
                else a.ConnectionState.Disconnected
            )
        else:
            assert isinstance(value.snapshot, a.WpsProgress.Unknown)
            assert value.snapshot.raw_value == raw
    p = prepare(a.Command.Station(action=a.StationAction.Stop()))
    assert logical(p, "station") == [0, 0, 0, 0]
    assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
    p = prepare(a.Command.Station(action=a.StationAction.StartStored()))
    assert logical(p, "station") == [0, 1, 255, 0]
    for raw in [0, 1, 255]:
        value = decode(p, "station", 2, raw)
        assert (
            isinstance(value, a.ReplyValue.StationConnection)
            and value.raw_result == raw
        )
    for mode, wire in [(a.StationScanMode.Mode0, 0), (a.StationScanMode.Mode1, 1)]:
        p = prepare(a.Command.Station(action=a.StationAction.Scan(mode=mode)))
        assert logical(p, "station") == [1, wire, 255, 255]
        for raw in [0, 1, 255]:
            buf = bytearray(p.raw)
            buf[1 + slots["station"][2]] = raw
            buf[1 + slots["station"][3]] = 255 - raw
            value = p.decode(buf).value
            assert isinstance(value, a.ReplyValue.StationScan)
            assert (value.raw_result, value.raw_count) == (raw, 255 - raw)
    for create in [
        lambda: a.WpsTarget.Peer(address=a.PeerAddress(b"123456")),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        lambda: a.WpsAction.Start(target=1),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        lambda: a.Command.Wps(action=a.StationAction.Stop()),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        lambda: a.StationAction.Scan(mode=a.Toggle.Enabled),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    ]:
        try:
            create()
        except TypeError:
            pass
        else:
            raise AssertionError("untyped WPS/station selector accepted")
    invalid(lambda: a.CountryBytes(b"USA"), a.ArgumentKind.CountryLength)
    data = bytearray(b"US")
    country = a.CountryBytes(data)
    data[:] = b"??"
    assert country.data == b"US"
    p = prepare(
        a.Command.NetworkReconfigure(
            action=a.NetworkReconfigureAction.ReadConfiguration()
        )
    )
    assert logical(p, "network_reconfigure") == [
        1,
        0,
        255,
        0,
        0,
        0,
        0,
        0,
        255,
        255,
        255,
        255,
        255,
        255,
    ]
    assert bytes(
        expect(p.decode(p.raw).value, a.ReplyValue.NetworkConfiguration).data
    ) == bytes([255] * 6)
    for radio, radio_wire in [
        (a.AccessPointRadioMode.Mode2, 0),
        (a.AccessPointRadioMode.Mode3, 1),
    ]:
        for security, security_wire in [
            (a.AccessPointSecurity.Option0, 0),
            (a.AccessPointSecurity.Option2, 2),
            (a.AccessPointSecurity.Option3, 3),
        ]:
            p = prepare(
                a.Command.NetworkReconfigure(
                    action=a.NetworkReconfigureAction.Apply(
                        number=255,
                        channel=a.RadioChannel(14),
                        radio=radio,
                        security=security,
                        country=country,
                    )
                )
            )
            assert logical(p, "network_reconfigure") == [
                1,
                255,
                14,
                0,
                radio_wire,
                security_wire,
                85,
                83,
                255,
                255,
                255,
                255,
                255,
                255,
            ]
            buf = bytearray(p.raw)
            for i, byte in enumerate([0, 1, 128, 254, 255, 42]):
                buf[1 + slots["network_reconfigure"][8 + i]] = byte
            value = p.decode(buf).value
            assert isinstance(value, a.ReplyValue.NetworkConfiguration) and bytes(
                value.data
            ) == bytes([0, 1, 128, 254, 255, 42])
    p = prepare(
        a.Command.NetworkReconfigure(action=a.NetworkReconfigureAction.StopServer())
    )
    assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
    try:
        a.NetworkReconfigureAction.Apply(
            number=1,
            channel=a.RadioChannel(1),
            radio=a.RadioMode.Mode0,  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
            security=a.AccessPointSecurity.Option0,
            country=country,
        )
    except TypeError:
        pass
    else:
        raise AssertionError("wrong AP selector type accepted")
    return True
