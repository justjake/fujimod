"""Shared native/Pyodide service-transfer control contract."""

from contract_assertions import expect
from fujimod.usb import app80


def check_transfer_values() -> bool:
    import json

    from fujimod import catalog
    from fujimod.usb import app80 as a

    layouts = [
        ("transfer_prepare", 1),
        ("transfer_select_operation", 2),
        ("transfer_ready", 1),
        ("transfer_select_channel", 2),
        ("transfer_start", 1),
        ("transfer_word", 10),
        ("transfer_pair", 7),
        ("transfer_bitfields", 5),
        ("transfer_byte", 3),
        ("transfer_mode_byte", 3),
        ("transfer_halfword", 7),
        ("transfer_option_word", 6),
        ("transfer_selected_word", 4),
        ("transfer_word7", 3),
        ("transfer_word16", 3),
        ("transfer_load_test", 1),
    ]
    slots = {name: [(i * 5 + 7) % 32 for i in range(count)] for name, count in layouts}
    entries = [
        dict(
            code=180 + n,
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
        firmware=dict(
            id="transfer-b", model="Synthetic", version="2", sha256="ab" * 32
        ),
        mailbox=dict(page=3, offset=576, configuration_limit=512),
        entries=entries,
    )
    service = a.Service(
        catalog.service.Definition.from_json(
            json.dumps(doc), catalog.FirmwareIdentity("transfer-b", "ab" * 32)
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
        data = bytearray(p.raw)
        data[1 + slots[codec][index]] = raw
        reply = p.decode(data)
        assert reply.raw == data
        return reply.value

    cases = [
        (
            a.Command.TransferPrepare(),
            "transfer_prepare",
            [255],
            0,
            a.ReplyValue.TransferPrepared,
        ),
        (
            a.Command.TransferReady(),
            "transfer_ready",
            [255],
            0,
            a.ReplyValue.TransferReadiness,
        ),
        (
            a.Command.TransferStart(),
            "transfer_start",
            [255],
            0,
            a.ReplyValue.TransferStarted,
        ),
    ]
    for operation, wire in [
        (a.TransferOperation.Operation0, 0),
        (a.TransferOperation.Operation1, 1),
        (a.TransferOperation.Operation2, 2),
        (a.TransferOperation.Operation3, 3),
        (a.TransferOperation.Operation6, 255),
    ]:
        cases.append((
            a.Command.TransferSelectOperation(operation=operation),
            "transfer_select_operation",
            [wire, 255],
            1,
            a.ReplyValue.TransferOperationSelected,
        ))
    for channel, wire in [
        (a.TransferChannel.Operation1, 0),
        (a.TransferChannel.Operation3, 1),
        (a.TransferChannel.Operation4, 2),
    ]:
        cases.append((
            a.Command.TransferSelectChannel(channel=channel),
            "transfer_select_channel",
            [wire, 255],
            1,
            a.ReplyValue.TransferChannelSelected,
        ))
    for command, codec, expected, index, reply_type in cases:
        p = prepare(command)
        assert logical(p, codec) == expected
        for raw in [0, 1, 2, 3, 255]:
            value = decode(p, codec, index, raw)
            assert isinstance(value, reply_type)
            if codec == "transfer_prepare":
                variants = {
                    0: a.TransferPrepareResult.Success,
                    2: a.TransferPrepareResult.PreparationError,
                    3: a.TransferPrepareResult.Status7,
                }
                cls = variants.get(raw, a.TransferPrepareResult.Unknown)
            else:
                variants = {0: a.TransferResult.Success, 1: a.TransferResult.Failed}
                cls = variants.get(raw, a.TransferResult.Unknown)
            assert isinstance(value.result, cls)
            if cls in [a.TransferPrepareResult.Unknown, a.TransferResult.Unknown]:
                assert isinstance(
                    value.result,
                    (a.TransferPrepareResult.Unknown, a.TransferResult.Unknown),
                )
                assert value.result.raw_status == raw
    for create in [
        lambda: a.Command.TransferSelectOperation(operation=1),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        lambda: a.Command.TransferSelectChannel(channel=a.TransferOperation.Operation1),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    ]:
        try:
            create()
        except TypeError:
            pass
        else:
            raise AssertionError("wrong transfer selector type accepted")
    for value in [1, 3, 65536]:
        try:
            a.TransferWordAddress(value)
        except a.CommandError as error:
            assert isinstance(error.reason, a.CommandFailure.Argument)
            assert error.reason.kind == a.ArgumentKind.TransferWordAddress
        else:
            raise AssertionError("invalid transfer word address")
    try:
        a.TransferPairFirst(512)
    except a.CommandError as error:
        assert isinstance(error.reason, a.CommandFailure.Argument)
        assert error.reason.kind == a.ArgumentKind.TransferPairFirst
    else:
        raise AssertionError("invalid transfer pair field")
    address = a.TransferWordAddress(65532)
    read = prepare(a.Command.TransferReadWord(address=address))
    assert logical(read, "transfer_word") == [
        0,
        0,
        0,
        255,
        252,
        255,
        255,
        255,
        255,
        255,
    ]
    write = prepare(a.Command.TransferWriteWord(address=address, value=0x89ABCDEF))
    assert logical(write, "transfer_word") == [
        1,
        0,
        0,
        255,
        252,
        137,
        171,
        205,
        239,
        255,
    ]
    for raw, cls in [
        (0, a.TransferWordReadResult.Value),
        (1, a.TransferWordReadResult.Rejected),
        (255, a.TransferWordReadResult.Unknown),
    ]:
        buf = bytearray(read.raw)
        for index, byte in [(5, 137), (6, 171), (7, 205), (8, 239), (9, raw)]:
            buf[1 + slots["transfer_word"][index]] = byte
        value = read.decode(buf).value
        assert isinstance(value, a.ReplyValue.TransferWordRead) and isinstance(
            value.result, cls
        )
        if raw == 0:
            assert (
                expect(value.result, a.TransferWordReadResult.Value).raw_value
                == 0x89ABCDEF
            )
        if raw == 255:
            assert (
                expect(value.result, a.TransferWordReadResult.Unknown).raw_status == 255
            )
    assert isinstance(
        expect(
            decode(write, "transfer_word", 9, 1), a.ReplyValue.TransferWordWritten
        ).result,
        a.TransferResult.Failed,
    )
    p = prepare(a.Command.TransferPair(first=a.TransferPairFirst(511), second=65535))
    assert logical(p, "transfer_pair") == [1, 255, 255, 255, 255, 255, 255]
    for raw, cls in [
        (0, a.TransferPairResult.Values),
        (3, a.TransferPairResult.CallbackError),
        (4, a.TransferPairResult.Rejected),
        (6, a.TransferPairResult.WaitError),
        (255, a.TransferPairResult.Unknown),
    ]:
        value = expect(decode(p, "transfer_pair", 6, raw), a.ReplyValue.TransferPair)
        assert isinstance(value.result, cls)
        if raw == 0:
            assert (
                expect(value.result, a.TransferPairResult.Values).first,
                expect(value.result, a.TransferPairResult.Values).second,
            ) == (255, 255)
        if raw == 255:
            assert expect(value.result, a.TransferPairResult.Unknown).raw_status == 255
    payload = a.TransferPayloadByte(255)
    assert payload.value == 255
    p = prepare(a.Command.TransferBitfields(mode=payload, first=0x1234, second=0xABCD))
    assert logical(p, "transfer_bitfields") == [255, 18, 52, 171, 205]
    assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
    for command, codec in [
        (a.Command.TransferByte(data=payload), "transfer_byte"),
        (a.Command.TransferModeByte(data=payload), "transfer_mode_byte"),
    ]:
        p = prepare(command)
        assert logical(p, codec) == [255, 255, 255]
        assert bytes(
            expect(decode(p, codec, 1, 0), a.ReplyValue.TransferBytes).data
        ) == bytes([0, 255])
    address = a.TransferHalfwordAddress(0xFFFFFFFF)
    assert address.value == 0xFFFFFFFF
    p = prepare(a.Command.TransferReadHalfword(address=address))
    assert logical(p, "transfer_halfword") == [0, 255, 255, 255, 255, 255, 255]
    assert (
        expect(p.decode(p.raw).value, a.ReplyValue.TransferHalfword).raw_value == 65535
    )
    p = prepare(a.Command.TransferWriteHalfword(address=address, value=0x89AB))
    assert logical(p, "transfer_halfword") == [1, 255, 255, 255, 255, 137, 171]
    assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
    for n in range(8):
        p = prepare(
            a.Command.TransferReadOptionWord(
                option=getattr(a.TransferOption, f"Option{n}")
            )
        )
        assert logical(p, "transfer_option_word") == [n % 4, n // 4, 255, 255, 255, 255]
        assert (
            expect(p.decode(p.raw).value, a.ReplyValue.TransferOptionWord).raw_value
            == 0xFFFFFFFF
        )
    for kind, selection, mode in [
        (a.TransferSelectedWordKind.Kind9, 2, 1),
        (a.TransferSelectedWordKind.Kind10, 2, 0),
        (a.TransferSelectedWordKind.Kind11, 1, 1),
        (a.TransferSelectedWordKind.Kind12, 1, 0),
        (a.TransferSelectedWordKind.Kind13, 0, 1),
        (a.TransferSelectedWordKind.Kind14, 0, 0),
    ]:
        p = prepare(a.Command.TransferSetSelectedWord(kind=kind, value=0x89AB))
        assert logical(p, "transfer_selected_word") == [selection, mode, 137, 171]
        assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
    for command, codec, wire in [
        (
            a.Command.TransferSetWord7(kind=a.TransferWord7Kind.Kind7, value=0x89AB),
            "transfer_word7",
            1,
        ),
        (
            a.Command.TransferSetWord7(kind=a.TransferWord7Kind.Kind8, value=0x89AB),
            "transfer_word7",
            0,
        ),
        (
            a.Command.TransferSetWord16(kind=a.TransferWord16Kind.Kind16, value=0x89AB),
            "transfer_word16",
            1,
        ),
        (
            a.Command.TransferSetWord16(kind=a.TransferWord16Kind.Kind17, value=0x89AB),
            "transfer_word16",
            0,
        ),
    ]:
        p = prepare(command)
        assert logical(p, codec) == [wire, 137, 171]
        assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
    for mode, wire in [
        (a.TransferLoadMode.Setup, 0),
        (a.TransferLoadMode.Run, 1),
        (a.TransferLoadMode.Request3One, 3),
        (a.TransferLoadMode.Request3Zero, 4),
    ]:
        p = prepare(a.Command.TransferLoadTest(mode=mode))
        assert logical(p, "transfer_load_test") == [wire]
    for create in [
        lambda: a.Command.TransferReadWord(address=a.WordAddress(4)),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        lambda: a.Command.TransferReadHalfword(address=a.TransferWordAddress(4)),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        lambda: a.Command.TransferReadOptionWord(option=1),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        lambda: a.Command.TransferSetWord7(kind=a.TransferWord16Kind.Kind16, value=1),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    ]:
        try:
            create()
        except TypeError:
            pass
        else:
            raise AssertionError("wrong transfer address/selector type accepted")
    return True
