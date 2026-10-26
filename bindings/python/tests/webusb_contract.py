"""Executed in real Pyodide by pyodide.ts; USB is replaced before import."""

import asyncio
from collections.abc import Awaitable, Callable
from typing import TYPE_CHECKING

from _fixture import log, state
from adjustment_contract import check_adjustment_values
from configuration_contract import check_configuration_values
from contract_assertions import expect
from exposure_contract import check_exposure_values
from extended_contract import check_extended_values
from fujimod import usb
from fujimod.catalog import service as definitions
from fujimod.usb import app80, data, memory, research, tracing
from lens_contract import check_lens_values
from media_contract import check_media_values
from network_contract import check_network_values
from peripheral_contract import check_peripheral_values
from transfer_contract import check_transfer_values

if TYPE_CHECKING:
    from _fujimod_js import JsBytes


async def check() -> bool:
    assert usb.InfoSelector.Product != 4
    assert usb.DataWriteMode.Echo != usb.DataReadMode.Echo
    limits = usb.FirmwareLimits(
        memory_read=256, memory_write=256, configuration=256, console=256
    )
    devices = await usb.discover(usb.DeviceFilter.Diagnostic)
    assert len(devices) == 1 and devices[0].vendor_id == 0x04CB
    assert devices[0].bus_id is None and devices[0].ports is None
    device = await usb.request_device(usb.DeviceFilter.Diagnostic)
    assert device is not None
    choices = await usb.interfaces(device)
    assert len(choices) == 1 and (
        choices[0].configuration,
        choices[0].interface,
        choices[0].alternate,
    ) == (1, 2, 3)
    trace = tracing.Trace(128, 8192)
    session = await usb.Session.open(device, choices[0], limits, trace=trace)
    assert session.state == usb.SessionState.Ready and session.token == 0x1234
    assert await session.device_info(usb.InfoSelector.Product) == "SYNTHETIC"
    assert await session.read_data(123, 5) == bytes([123, 124, 125, 126, 127])
    try:
        await session.device_info("Product")  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    except TypeError:
        pass
    else:
        raise AssertionError("string selector accepted")
    try:
        await session.read_data(0, 1.0)  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    except TypeError:
        pass
    else:
        raise AssertionError("fractional Python quantity accepted")
    request = research.Request(
        research.CommandCode(0xDEADBEEF), bytes(16), reply_capacity=32
    )
    assert request.code == research.CommandCode(0xDEADBEEF) and request.body == bytes(
        16
    )
    assert app80.Request(app80.CommandCode(2), b"hi").raw == b"\x02hi" + bytes(30)
    window = usb.DataWindow(0x1000, 0x1000, 32, 4, usb.OffsetMode.Checked)
    async with data.read_chunks(session, window, 0x1000, 6) as reader:
        assert session.state == usb.SessionState.ReadingRange
        assert [chunk async for chunk in reader] == [bytes(range(4)), b"\x04\x05"]
        assert reader.bytes_completed == 6
    mutable = bytearray(b"abc")
    result = await data.upload(
        session, memoryview(mutable), piece_bytes=2, mode=usb.DataWriteMode.Preserve
    )
    assert result.bytes_written == 3 and result.acknowledgment == b"\x01\x02\x03\x04"
    assert result.setup.status == usb.CommandStatus.Success
    assert result.setup.fields == [0, 0, 0, 0]
    assert await session.close() == b"\xaa\xbb"
    assert session.state == usb.SessionState.Closed
    batch = trace.drain()
    assert batch.dropped_events == 0 and batch.dropped_bytes == 0
    assert batch.protocol_events
    assert all(e.layer == tracing.Layer.Ff80 for e in batch.protocol_events)
    assert any(e.command == 0x150000 and e.data for e in batch.protocol_events)
    assert len({e.sequence for e in [*batch.events, *batch.protocol_events]}) == len(
        batch.events
    ) + len(batch.protocol_events)
    assert any(
        e.stage == tracing.Stage.BulkIn and e.endpoint == 0x82 for e in batch.events
    )
    assert all(
        type(e.sequence) is int and e.timestamp > 1_500_000_000 for e in batch.events
    )
    try:
        await session.ping()
    except usb.ClosedSessionError:
        pass
    else:
        raise AssertionError("closed session accepted request")
    second = await usb.Session.open(device, choices[0], limits)
    state.hangControl = 0xC2
    task = asyncio.create_task(second.ping())
    while not any(entry[0] == "control" and entry[1] == 0xC2 for entry in log):
        await asyncio.sleep(0)
    task.cancel()
    try:
        await task
    except asyncio.CancelledError:
        pass
    else:
        raise AssertionError("cancel did not propagate")
    assert second.state == usb.SessionState.CompletionUnknown
    try:
        await second.close()
    except usb.CompletionUnknownError:
        pass
    else:
        raise AssertionError("canceled exchange allowed cleanup")
    second.abandon()
    assert second.state == usb.SessionState.Abandoned
    assert sum(entry[0] == "control" and entry[1] == 0xC4 for entry in log) == 1
    return True


async def check_pending_reader() -> bool:
    state.hangControl = None
    device = (await usb.discover(usb.DeviceFilter.Diagnostic))[0]
    interface = (await usb.interfaces(device))[0]
    limits = usb.FirmwareLimits(
        memory_read=256, memory_write=256, configuration=256, console=256
    )
    session = await usb.Session.open(device, interface, limits)
    state.hangControl = 0xE1
    reader = data.read_chunks(
        session,
        usb.DataWindow(0x1000, 0x1000, 16, 4, usb.OffsetMode.Checked),
        0x1000,
        8,
    )
    closed = object()

    async def read_one() -> object:
        try:
            return await anext(reader)
        except usb.ClosedSessionError:
            return closed

    pending = asyncio.create_task(read_one())
    while state.kind != 0xE1:
        await asyncio.sleep(0)
    await asyncio.gather(reader.aclose(), reader.aclose())
    assert await pending is closed
    assert session.state == usb.SessionState.CompletionUnknown
    assert reader.bytes_completed == 0
    assert [chunk async for chunk in reader] == []
    session.abandon()
    state.hangControl = None
    trace = tracing.Trace(64, 8192)
    session = await usb.Session.open(
        device, interface, limits, timeout_ms=10, trace=trace
    )
    state.hangControl = 0xC2
    try:
        await session.ping()
    except usb.TransferError as error:
        assert error.partial == b"" and error.transferred is None
    else:
        raise AssertionError("hung control completed")
    assert session.state == usb.SessionState.CompletionUnknown
    failures = [e for e in trace.drain().events if e.failure_kind is not None]
    assert any(
        e.failure_kind == tracing.FailureKind.Timeout and e.error for e in failures
    )
    session.abandon()
    return True


async def check_commands() -> bool:
    state.hangControl = None
    device = (await usb.discover(usb.DeviceFilter.Diagnostic))[0]
    interface = (await usb.interfaces(device))[0]
    limits = usb.FirmwareLimits(
        memory_read=4, memory_write=4, configuration=256, console=256
    )
    s = await usb.Session.open(device, interface, limits)
    await memory.write_range(s, 0x2000, b"abcdef")
    assert await memory.read_range(s, 0x2000, 6) == b"abcdef"
    await s.write_memory(0x2000, b"xyz")
    assert await s.read_memory(0x2000, 3) == b"xyz"
    geometry = await s.configuration_geometry(usb.ConfigurationView.Ram)
    assert (geometry.capacity, geometry.page_bytes) == (4096, 256) and len(
        geometry.raw
    ) == 36
    await s.write_configuration(usb.ConfigurationView.Ram, 0x181, b"abc", page=2)
    assert (
        await s.read_configuration(usb.ConfigurationView.Ram, 0x181, 3, page=2)
        == b"abc"
    )
    mailbox = app80.Mailbox(page=2, offset=0x180, configuration_limit=256)
    request = app80.Request(app80.CommandCode(1), b"hi")
    assert await app80.exchange(s, mailbox, request) == request.raw
    result = await s.research_command(
        research.Request(
            research.CommandCode(0xDEADBEEF), bytes(range(16)), reply_capacity=32
        )
    )
    assert result.code == research.CommandCode(0xDEADBEEF) and result.body == bytes(
        range(16)
    )
    assert result.status == usb.CommandStatus.DeviceError and (
        result.primary,
        result.secondary,
    ) == (1, 0xFFFF)
    reply = await s.diagnostic_query(usb.DiagnosticTag.bulk_source())
    assert reply.status == usb.CommandStatus.DeviceError and reply.fields == [
        9,
        8,
        7,
        6,
    ]
    assert (
        await s.prepare_data_read(usb.DataReadMode.Echo)
    ).status == usb.CommandStatus.Success
    assert (
        await s.prepare_data_write(4, usb.DataWriteMode.FillOnes)
    ).status == usb.CommandStatus.Success
    await s.write_data(b"xyz")
    assert await s.empty_response() == b"\x01\x02\x03\x04"
    caps = await s.prepare_capabilities()
    assert caps.byte_length == 48 and len(caps.raw) == 32
    assert (
        await s.firmware_download_status()
    ).state == usb.FirmwareDownloadState.Writing
    assert (
        await s.clear_firmware_download_status()
    ).state == usb.FirmwareDownloadState.Normal
    assert await s.send_key("S1", usb.KeyState.On) == b""
    assert await s.request_mode(usb.CameraMode.Record) == b""
    assert await s.command_event("TEST") == b""
    await s.submit_console(b"help\n")
    log_reply = await s.read_log(8)
    assert log_reply.status == 7 and log_reply.data == b"log"
    console = await s.poll_console(8)
    assert (
        console.progress == usb.ConsoleProgress.Unknown
        and console.unknown_progress == 42
    )
    assert console.status == 7 and console.output == b"log"
    assert s.state == usb.SessionState.Ready
    assert await s.close() == b"\xaa\xbb"
    return True


async def run() -> bool:
    await check()
    await check_install_helpers()
    await check_pending_reader()
    await check_commands()
    await check_service_catalog()
    assert check_peripheral_values()
    assert check_media_values()
    assert check_network_values()
    assert check_exposure_values()
    assert check_extended_values()
    assert check_adjustment_values()
    assert check_lens_values()
    assert check_configuration_values()
    assert check_transfer_values()
    return True


async def check_install_helpers() -> None:
    import struct

    limits = usb.FirmwareLimits(
        memory_read=256, memory_write=256, configuration=256, console=256
    )
    device = await usb.request_device(usb.DeviceFilter.Diagnostic)
    assert device is not None
    interface = (await usb.interfaces(device))[0]
    session = await usb.Session.open(device, interface, limits)
    raw = bytes(range(256)) + b"tail"
    for batch in usb.CommandSequence.verified_writes(
        0xA400, raw, limits, session.packet_bytes
    ):
        await session.execute_sequence(batch)
    assert await memory.read_range(session, 0xA400, len(raw)) == raw
    await session.write_memory(0xB000, struct.pack("<I", 0xC000))
    await session.write_memory(0xB100, b"orig")
    plan = usb.InstallQueries(0xB000, 0xC000, 0xB100, b"orig")
    queried, output = plan.call(0xC100, 17, [(1, 0xFFFF)], b"seed")
    replies = await session.execute_sequence(plan.finish())
    assert len(replies[queried]) == 32 and replies[output] == b"seed"
    assert await session.read_memory(0xB000, 4) == struct.pack("<I", 0xC000)
    assert await session.read_memory(0xB100, 4) == b"orig"
    await session.write_memory(0xA000, struct.pack("<I", 0x8000))
    await session.write_memory(0xA004, struct.pack("<I", 32))
    image = usb.OwnedUpload(0x4000, 16, 0x4000, bytes(range(11)), 8, 4)
    assert image.padded_bytes == usb.OwnedUpload.padded_size(11, 8) == 16
    assert image.image_bytes == 11
    receipt = await session.upload_owned(image, 0xA000, 0x8000, 0xA004, 32)
    assert receipt.bytes_written == 16
    assert await session.read_memory(0xA000, 4) == struct.pack("<I", 0x8000)
    # This fixture's E1 source is the declared incrementing byte pattern.
    await session.verify_owned(
        image, usb.DataWindow(0x4000, 0x4000, 16, 4, usb.OffsetMode.Checked)
    )
    assert await session.close() == b"\xaa\xbb"


async def check_service_catalog() -> bool:
    import importlib
    import json

    from fujimod import catalog
    from fujimod.catalog import service as definitions
    from js import Promise
    from pyodide.ffi import create_proxy

    def definition(
        rows: list[tuple[int, str, list[int], int | None]] | None = None,
        expected_id: str = "synthetic-b-v2",
    ) -> definitions.Definition:
        if rows is None:
            rows = [(153, "flush_configuration", [7], 4)]
        document = dict(
            schema=1,
            firmware=dict(
                id="synthetic-b-v2", model="Body B", version="2", sha256="ab" * 32
            ),
            mailbox=dict(page=3, offset=576, configuration_limit=512),
            entries=[
                dict(
                    code=code,
                    label=codec,
                    codec=codec,
                    slots=slots,
                    enabled_modes=modes,
                    next_mode=0,
                    firmware_kind=19,
                    handler=4294967296,
                    parameter_table=8192,
                )
                for code, codec, slots, modes in rows
            ],
        )
        return definitions.Definition.from_json(
            json.dumps(document), catalog.FirmwareIdentity(expected_id, "ab" * 32)
        )

    def failure[E: Exception](cls: type[E], action: Callable[[], object]) -> E:
        try:
            action()
        except cls as error:
            return error
        raise AssertionError(f"{cls.__name__} was not raised")

    assert importlib.import_module("fujimod.catalog") is catalog
    assert importlib.import_module("fujimod.catalog.service") is definitions
    modes = definitions.ServiceModes(4)
    d = definition()
    assert d.identity.id == "synthetic-b-v2" and d.identity.sha256 == "ab" * 32
    assert (d.mailbox.page, d.mailbox.offset, d.mailbox.configuration_limit) == (
        3,
        576,
        512,
    )
    (entry,) = d.entries
    assert entry.handler == 4294967296 and entry.slots == (7,)
    assert (
        entry.codec == definitions.Codec.FlushConfiguration
        and entry.firmware_kind == 19
    )
    assert (
        isinstance(entry.enabled_modes, definitions.ModeAvailability.Modes)
        and entry.enabled_modes.modes == modes
    )
    assert isinstance(entry.transition, definitions.ModeTransition.Keep)
    assert d.select(153, modes).code == 153
    assert d.select(153, definitions.ServiceModes(1)) is None
    unreviewed = definition([(153, "flush_configuration", [7], None)])
    assert isinstance(
        unreviewed.entries[0].enabled_modes, definitions.ModeAvailability.Unreviewed
    )
    assert unreviewed.select(153, modes) is None
    assert isinstance(
        failure(
            catalog.DefinitionError, lambda: definition(expected_id="another-body")
        ).reason,
        catalog.DefinitionFailure.FirmwareMismatch,
    )
    assert isinstance(
        failure(
            catalog.DefinitionError, lambda: catalog.FirmwareIdentity("wrong", "é" * 32)
        ).reason,
        catalog.DefinitionFailure.InvalidDigest,
    )
    assert isinstance(
        failure(catalog.DefinitionError, lambda: definitions.ServiceModes(0)).reason,
        catalog.DefinitionFailure.EmptyModes,
    )
    assert isinstance(
        failure(app80.CommandError, lambda: app80.PinBank(16)).reason,
        app80.CommandFailure.Argument,
    )
    assert (
        failure(app80.CommandError, lambda: app80.WordAddress(3)).reason.kind
        == app80.ArgumentKind.WordAlignment
    )
    failure(TypeError, lambda: app80.Command())
    failure(TypeError, lambda: app80.Command.ReadPins(bank=0))
    failure(
        TypeError, lambda: app80.ProfileAction.Read(bank=0, index=app80.ProfileIndex(0))
    )
    failure(TypeError, lambda: app80.Command.ProfileByte(action="read"))
    assert app80.ProfileBank.First != 0 and app80.ProfileBank.First != "First"

    C, P = app80.Command, app80.ProfileAction
    cases = [
        (
            "write_pins",
            C.WritePins(bank=app80.PinBank(2), levels=0x1234, exclude=0x5678),
            [2, 0x12, 0x34, 0x56, 0x78],
        ),
        ("read_pins", C.ReadPins(bank=app80.PinBank(2)), [2, 0, 0]),
        ("read_channel", C.ReadChannel(channel=30), [30, 0, 0]),
        ("write_channel", C.WriteChannel(channel=30, value=0x1234), [30, 0x12, 0x34]),
        (
            "profile_byte",
            C.ProfileByte(
                action=P.Read(bank=app80.ProfileBank.First, index=app80.ProfileIndex(3))
            ),
            [1, 3, 0],
        ),
        (
            "profile_byte",
            C.ProfileByte(
                action=P.Write(
                    bank=app80.ProfileBank.Second, index=app80.ProfileIndex(4), value=37
                )
            ),
            [4, 4, 37],
        ),
        ("profile_byte", C.ProfileByte(action=P.Reset()), [5, 0, 0]),
        ("download_status", C.DownloadStatus(), [0]),
        ("copy_configuration", C.CopyConfiguration(page=9), [9]),
        ("flush_configuration", C.FlushConfiguration(), [255]),
        (
            "memory_word",
            C.ReadWord(address=app80.WordAddress(0x40001000)),
            [0, 0, 0x40, 0, 0x10, 0, 0, 0, 0, 0],
        ),
        (
            "memory_word",
            C.WriteWord(address=app80.WordAddress(0x40001000), value=0x12345678),
            [1, 0, 0x40, 0, 0x10, 0, 0x12, 0x34, 0x56, 0x78],
        ),
    ]
    for codec, command, expected in cases:
        assert isinstance(command, C)
        slots = [(i * 3 + 7) % 32 for i in range(len(expected))]
        invocation = app80.Service(definition([(153, codec, slots, 4)])).prepare(
            modes, command
        )
        assert invocation.raw[0] == 153
        assert [invocation.raw[1 + s] for s in slots] == expected
        assert invocation.configuration_limit == 512
        if command.__match_args__:
            failure(
                AttributeError,
                lambda command=command: setattr(command, command.__match_args__[0], 0),
            )

    def decode(
        codec: str, command: app80.Command, slots: list[int], changes: dict[int, int]
    ) -> app80.ReplyValue:
        invocation = app80.Service(definition([(153, codec, slots, 4)])).prepare(
            modes, command
        )
        raw = bytearray(invocation.raw)
        for position, byte in changes.items():
            raw[1 + slots[position]] = byte
        reply = invocation.decode(raw)
        assert reply.raw == raw
        assert isinstance(reply.value, app80.ReplyValue)
        return reply.value

    value = decode(
        "read_pins", C.ReadPins(bank=app80.PinBank(0)), [7, 9, 3], {1: 0x12, 2: 0x34}
    )
    assert isinstance(value, app80.ReplyValue.Pins) and value.levels == 0x1234
    value = decode(
        "read_channel", C.ReadChannel(channel=30), [7, 9, 3], {1: 0x12, 2: 0x34}
    )
    assert isinstance(value, app80.ReplyValue.Channel) and value.value == 0x1234
    value = decode(
        "profile_byte",
        C.ProfileByte(
            action=P.Read(bank=app80.ProfileBank.First, index=app80.ProfileIndex(0))
        ),
        [7, 9, 3],
        {2: 57},
    )
    assert isinstance(value, app80.ReplyValue.ProfileByte) and value.value == 57
    value = decode("download_status", C.DownloadStatus(), [7], {0: 1})
    assert isinstance(value, app80.ReplyValue.DownloadStatus) and value.raw_status == 1
    assert isinstance(
        decode("copy_configuration", C.CopyConfiguration(page=1), [7], {}),
        app80.ReplyValue.NoPayload,
    )
    slots = list(range(10))
    read, write = (
        C.ReadWord(address=app80.WordAddress(0x40001000)),
        C.WriteWord(address=app80.WordAddress(0x40001000), value=1),
    )
    value = decode(
        "memory_word", read, slots, {1: 1, 6: 0x12, 7: 0x34, 8: 0x56, 9: 0x78}
    )
    assert (
        isinstance(value.result, app80.WordReadResult.Value)
        and value.result.value == 0x12345678
    )
    assert isinstance(
        decode("memory_word", read, slots, {1: 0}).result, app80.WordReadResult.Rejected
    )
    assert decode("memory_word", read, slots, {1: 42}).result.raw_status == 42
    assert isinstance(
        decode("memory_word", write, slots, {1: 1}).result,
        app80.WordWriteResult.Confirmed,
    )
    assert isinstance(
        decode("memory_word", write, slots, {1: 0}).result,
        app80.WordWriteResult.Rejected,
    )
    assert decode("memory_word", write, slots, {1: 42}).result.raw_status == 42
    service = app80.Service(d)
    invocation = service.prepare(modes, C.FlushConfiguration())
    assert isinstance(
        expect(
            invocation.decode(invocation.raw).value, app80.ReplyValue.ConfigurationSaved
        ).status,
        app80.SaveStatus.Unknown,
    )
    assert (
        expect(
            expect(
                invocation.decode(invocation.raw).value,
                app80.ReplyValue.ConfigurationSaved,
            ).status,
            app80.SaveStatus.Unknown,
        ).raw_status
        == 255
    )
    research = service.prepare_research(app80.CommandCode(153), modes, b"")
    assert isinstance(research.decode(bytes(33)).value, app80.ReplyValue.Research)
    failure(usb.ProtocolError, lambda: invocation.decode(bytes(33)))
    failure(TypeError, lambda: service.prepare(4, C.FlushConfiguration()))
    failure(TypeError, lambda: service.prepare(modes, "save"))
    assert isinstance(
        failure(
            app80.CommandError,
            lambda: service.prepare(
                definitions.ServiceModes(1), C.FlushConfiguration()
            ),
        ).reason,
        app80.CommandFailure.Unsupported,
    )
    assert isinstance(
        failure(
            app80.CommandError,
            lambda: service.prepare_research(
                app80.CommandCode(153), definitions.ServiceModes(1), b""
            ),
        ).reason,
        app80.CommandFailure.Unavailable,
    )
    assert isinstance(
        failure(
            app80.CommandError,
            lambda: service.prepare_research(app80.CommandCode(153), modes, bytes(33)),
        ).reason,
        app80.CommandFailure.OversizeArguments,
    )
    ambiguous = app80.Service(
        definition([
            (153, "flush_configuration", [7], 4),
            (154, "flush_configuration", [7], 4),
        ])
    )
    assert isinstance(
        failure(
            app80.CommandError, lambda: ambiguous.prepare(modes, C.FlushConfiguration())
        ).reason,
        app80.CommandFailure.Ambiguous,
    )

    state.hangControl = None
    device = (await usb.discover(usb.DeviceFilter.Diagnostic))[0]
    interface = (await usb.interfaces(device))[0]
    limits = usb.FirmwareLimits(
        memory_read=256, memory_write=256, configuration=256, console=256
    )
    sessions = []
    behavior = "failed"
    published = 0
    never = Promise.new(lambda resolve, reject: None)

    def publish(page: int, address: int, raw: JsBytes) -> Awaitable[object] | None:
        nonlocal published
        if page == 3 and address == 576:
            published += 1
            assert raw[0] == 153 and raw[8] == 255
            if behavior == "foreign":
                state.configuration.set("3:576", 0)
            elif behavior == "pending":
                return never
            else:
                state.configuration.set("3:584", 1)

    callback = create_proxy(publish)
    state.onConfigurationWrite = callback
    try:
        s = await usb.Session.open(device, interface, limits)
        sessions.append(s)
        reply = await app80.execute(s, invocation)
        assert isinstance(
            expect(reply.value, app80.ReplyValue.ConfigurationSaved).status,
            app80.SaveStatus.Failed,
        )
        assert s.state == usb.SessionState.Ready and published == 1
        await s.close()
        behavior = "foreign"
        s = await usb.Session.open(device, interface, limits)
        sessions.append(s)
        try:
            await app80.execute(s, invocation)
        except usb.ProtocolError:
            pass
        else:
            raise AssertionError("foreign nested reply accepted")
        assert s.state == usb.SessionState.CompletionUnknown
        s.abandon()
        behavior = "pending"
        s = await usb.Session.open(device, interface, limits)
        sessions.append(s)
        task = asyncio.create_task(app80.execute(s, invocation))

        async def wait_for_publication() -> None:
            while published < 3:
                await asyncio.sleep(0)

        await asyncio.wait_for(wait_for_publication(), 2)
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass
        else:
            raise AssertionError("service publication cancellation did not propagate")
        assert s.state == usb.SessionState.CompletionUnknown
        s.abandon()
    finally:
        state.onConfigurationWrite = None
        callback.destroy()
        for session in sessions:
            if session.state not in (
                usb.SessionState.Closed,
                usb.SessionState.Abandoned,
            ):
                session.abandon()
    return True
