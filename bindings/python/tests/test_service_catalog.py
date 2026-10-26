"""Native enum conversions and service composition; synthetic endpoints only."""

import asyncio
import importlib
import json

import pytest
from contract_assertions import expect
from fujimod import catalog, usb
from fujimod.catalog import service as data
from fujimod.usb import app80, testing
from test_app80 import config_read, config_write
from test_async import dispatched, run
from test_async import opened as async_opened
from test_stream import LIMITS, E, opened


def definition(
    rows: list[tuple[int, str, list[int], int | None]] | None = None,
    *,
    firmware_id: str = "synthetic-b-v2",
    expected: catalog.FirmwareIdentity | None = None,
) -> data.Definition:
    if rows is None:
        rows = [(153, "flush_configuration", [7], 4)]
    document = {
        "schema": 1,
        "firmware": {
            "id": firmware_id,
            "model": "Body B",
            "version": "2",
            "sha256": "ab" * 32,
        },
        "mailbox": {"page": 3, "offset": 0x240, "configuration_limit": 512},
        "entries": [
            {
                "code": code,
                "label": codec,
                "codec": codec,
                "slots": slots,
                "enabled_modes": modes,
                "next_mode": 0,
                "firmware_kind": 19,
                "handler": 4096,
                "parameter_table": 8192,
            }
            for code, codec, slots, modes in rows
        ],
    }
    identity = (
        expected
        if expected is not None
        else catalog.FirmwareIdentity(firmware_id, "ab" * 32)
    )
    return data.Definition.from_json(json.dumps(document), identity)


def save() -> app80.Invocation:
    service = app80.Service(definition())
    return service.prepare(data.ServiceModes(4), app80.Command.FlushConfiguration())


def exchange(
    invocation: app80.Invocation, raw: bytes | bytearray
) -> list[testing.Exchange]:
    return [
        *config_write(0x241, invocation.raw[1:], page=3),
        *config_write(0x240, invocation.raw, page=3),
        *config_read(0x240, raw, page=3),
    ]


def test_shared_data_namespace_keeps_enum_metadata_and_explicit_identity() -> None:
    assert importlib.import_module("fujimod.catalog") is catalog
    assert importlib.import_module("fujimod.catalog.service") is data
    d = definition([(153, "flush_configuration", [7], None)])
    assert (d.identity.id, d.identity.sha256) == ("synthetic-b-v2", "ab" * 32)
    assert (d.mailbox.page, d.mailbox.offset, d.mailbox.configuration_limit) == (
        3,
        0x240,
        512,
    )
    (entry,) = d.entries
    assert entry.codec == data.Codec.FlushConfiguration
    assert isinstance(entry.enabled_modes, data.ModeAvailability.Unreviewed)
    assert isinstance(entry.transition, data.ModeTransition.Keep)
    assert entry.slots == (7,) and entry.firmware_kind == 19
    assert d.select(153, data.ServiceModes(4)) is None
    d = definition()
    assert d.select(153, data.ServiceModes(1)) is None
    selected = d.select(153, data.ServiceModes(4))
    assert selected is not None
    assert isinstance(selected.enabled_modes, data.ModeAvailability.Modes)
    with pytest.raises(TypeError):
        d.select(153, 4)
    with pytest.raises(catalog.DefinitionError) as failed:
        data.ServiceModes(0)
    assert isinstance(failed.value.reason, catalog.DefinitionFailure.EmptyModes)
    with pytest.raises(catalog.DefinitionError) as failed:
        data.Definition.from_json(
            "{}", catalog.FirmwareIdentity("elsewhere", "cd" * 32)
        )
    assert isinstance(failed.value.reason, catalog.DefinitionFailure.Json)
    with pytest.raises(catalog.DefinitionError) as failed:
        catalog.FirmwareIdentity("wrong", "é" * 32)
    assert isinstance(failed.value.reason, catalog.DefinitionFailure.InvalidDigest)
    with pytest.raises(catalog.DefinitionError) as failed:
        definition(expected=catalog.FirmwareIdentity("another-body", "ab" * 32))
    assert isinstance(failed.value.reason, catalog.DefinitionFailure.FirmwareMismatch)
    with pytest.raises(AttributeError):
        entry.codec = data.Codec.Research


def test_enum_inputs_do_not_accept_integers_or_strings_as_selectors() -> None:
    assert app80.ProfileBank.First != 0 and app80.ProfileBank.First != "First"
    for constructor, argument in [
        (app80.PinBank, 16),
        (app80.ProfileIndex, 16),
        (app80.WordAddress, 3),
    ]:
        with pytest.raises(app80.CommandError) as failed:
            constructor(argument)
        assert isinstance(failed.value.reason, app80.CommandFailure.Argument)
    with pytest.raises(TypeError):
        app80.Command.ReadPins(bank=0)  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    with pytest.raises(TypeError):
        app80.Command.ProfileByte(action="read")  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    with pytest.raises(TypeError):
        app80.ProfileAction.Read(bank=0, index=app80.ProfileIndex(1))  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    with pytest.raises(TypeError):
        app80.Service(definition()).prepare(4, app80.Command.FlushConfiguration())  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    with pytest.raises(TypeError):
        app80.Service(definition()).prepare(data.ServiceModes(4), "save")  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    with pytest.raises(OverflowError):
        app80.Command.WriteChannel(channel=1, value=65536)


def test_every_command_variant_converts_to_the_core_codec_without_dispatch_strings() -> (
    None
):
    cases = [
        (
            "write_pins",
            [0, 1, 2, 3, 4],
            app80.Command.WritePins(
                bank=app80.PinBank(2), levels=0x1234, exclude=0x5678
            ),
            [2, 0x12, 0x34, 0x56, 0x78],
        ),
        (
            "read_pins",
            [0, 1, 2],
            app80.Command.ReadPins(bank=app80.PinBank(2)),
            [2, 0, 0],
        ),
        ("read_channel", [0, 1, 2], app80.Command.ReadChannel(channel=30), [30, 0, 0]),
        (
            "write_channel",
            [0, 1, 2],
            app80.Command.WriteChannel(channel=30, value=0x1234),
            [30, 0x12, 0x34],
        ),
        (
            "profile_byte",
            [0, 1, 2],
            app80.Command.ProfileByte(
                action=app80.ProfileAction.Read(
                    bank=app80.ProfileBank.First, index=app80.ProfileIndex(3)
                )
            ),
            [1, 3, 0],
        ),
        (
            "profile_byte",
            [0, 1, 2],
            app80.Command.ProfileByte(
                action=app80.ProfileAction.Write(
                    bank=app80.ProfileBank.Second, index=app80.ProfileIndex(4), value=37
                )
            ),
            [4, 4, 37],
        ),
        (
            "profile_byte",
            [0, 1, 2],
            app80.Command.ProfileByte(action=app80.ProfileAction.Reset()),
            [5, 0, 0],
        ),
        ("download_status", [0], app80.Command.DownloadStatus(), [0]),
        ("copy_configuration", [0], app80.Command.CopyConfiguration(page=9), [9]),
        ("flush_configuration", [0], app80.Command.FlushConfiguration(), [255]),
        (
            "memory_word",
            list(range(10)),
            app80.Command.ReadWord(address=app80.WordAddress(0x40001000)),
            [0, 0, 0x40, 0, 0x10, 0, 0, 0, 0, 0],
        ),
        (
            "memory_word",
            list(range(10)),
            app80.Command.WriteWord(
                address=app80.WordAddress(0x40001000), value=0x12345678
            ),
            [1, 0, 0x40, 0, 0x10, 0, 0x12, 0x34, 0x56, 0x78],
        ),
    ]
    for codec, slots, command, expected in cases:
        # Permute positions and use a different command identifier from stock.
        slots = [(s * 3 + 7) % 32 for s in slots]
        service = app80.Service(definition([(153, codec, slots, 4)]))
        invocation = service.prepare(data.ServiceModes(4), command)
        assert invocation.raw[0] == 153
        assert [invocation.raw[1 + s] for s in slots] == expected
        assert invocation.configuration_limit == 512


def test_selection_failures_are_structured_and_unknown_values_stay_data() -> None:
    service = app80.Service(definition())
    with pytest.raises(app80.CommandError) as failed:
        service.prepare(data.ServiceModes(1), app80.Command.FlushConfiguration())
    assert isinstance(failed.value.reason, app80.CommandFailure.Unsupported)
    assert failed.value.reason.codec == data.Codec.FlushConfiguration
    with pytest.raises(app80.CommandError) as failed:
        service.prepare_research(app80.CommandCode(153), data.ServiceModes(1), b"")
    assert isinstance(failed.value.reason, app80.CommandFailure.Unavailable)
    with pytest.raises(app80.CommandError) as failed:
        service.prepare_research(
            app80.CommandCode(153), data.ServiceModes(4), bytes(33)
        )
    assert isinstance(failed.value.reason, app80.CommandFailure.OversizeArguments)
    ambiguous = app80.Service(
        definition([
            (153, "flush_configuration", [7], 4),
            (154, "flush_configuration", [7], 4),
        ])
    )
    with pytest.raises(app80.CommandError) as failed:
        ambiguous.prepare(data.ServiceModes(4), app80.Command.FlushConfiguration())
    assert isinstance(failed.value.reason, app80.CommandFailure.Ambiguous)
    invocation = save()
    result = invocation.decode(invocation.raw)
    assert isinstance(result.value, app80.ReplyValue.ConfigurationSaved)
    assert isinstance(result.value.status, app80.SaveStatus.Unknown)
    assert result.value.status.raw_status == 255 and result.raw == invocation.raw
    with pytest.raises(usb.ProtocolError):
        invocation.decode(bytes(33))
    research = service.prepare_research(
        app80.CommandCode(153), data.ServiceModes(4), b""
    )
    assert isinstance(research.decode(bytes(33)).value, app80.ReplyValue.Research)


async def test_blocking_execution_uses_the_selected_mailbox_and_does_not_poll_status_one() -> (
    None
):
    invocation = save()
    raw = bytearray(invocation.raw)
    raw[8] = 1
    session, script = await opened(exchange(invocation, raw))
    reply = await usb.app80.execute(session, invocation)
    assert isinstance(
        expect(reply.value, app80.ReplyValue.ConfigurationSaved).status,
        app80.SaveStatus.Failed,
    )
    assert reply.raw == raw
    assert session.state == usb.SessionState.Ready and script.remaining == 0


def test_result_variants_keep_values_rejection_and_unknown_status_separate() -> None:
    def decode(
        codec: str, slots: list[int], command: app80.Command, changes: dict[int, int]
    ) -> app80.ReplyValue:
        invocation = app80.Service(definition([(153, codec, slots, 4)])).prepare(
            data.ServiceModes(4), command
        )
        raw = bytearray(invocation.raw)
        for position, byte in changes.items():
            raw[1 + slots[position]] = byte
        return invocation.decode(raw).value

    value = decode(
        "read_pins",
        [7, 9, 3],
        app80.Command.ReadPins(bank=app80.PinBank(2)),
        {1: 0x12, 2: 0x34},
    )
    assert isinstance(value, app80.ReplyValue.Pins) and value.levels == 0x1234
    value = decode(
        "read_channel",
        [7, 9, 3],
        app80.Command.ReadChannel(channel=30),
        {1: 0x12, 2: 0x34},
    )
    assert isinstance(value, app80.ReplyValue.Channel) and value.value == 0x1234
    command = app80.Command.ProfileByte(
        action=app80.ProfileAction.Read(
            bank=app80.ProfileBank.First, index=app80.ProfileIndex(0)
        )
    )
    value = decode("profile_byte", [7, 9, 3], command, {2: 57})
    assert isinstance(value, app80.ReplyValue.ProfileByte) and value.value == 57
    value = decode("download_status", [7], app80.Command.DownloadStatus(), {0: 1})
    assert isinstance(value, app80.ReplyValue.DownloadStatus) and value.raw_status == 1
    value = decode(
        "copy_configuration", [7], app80.Command.CopyConfiguration(page=1), {}
    )
    assert isinstance(value, app80.ReplyValue.NoPayload)
    read = app80.Command.ReadWord(address=app80.WordAddress(0x40001000))
    value = decode(
        "memory_word", list(range(10)), read, {1: 1, 6: 0x12, 7: 0x34, 8: 0x56, 9: 0x78}
    )
    assert isinstance(value, app80.ReplyValue.ReadWord)
    assert (
        isinstance(value.result, app80.WordReadResult.Value)
        and value.result.value == 0x12345678
    )
    assert isinstance(
        expect(
            decode("memory_word", list(range(10)), read, {1: 0}),
            app80.ReplyValue.ReadWord,
        ).result,
        app80.WordReadResult.Rejected,
    )
    unknown = expect(
        decode("memory_word", list(range(10)), read, {1: 42}), app80.ReplyValue.ReadWord
    ).result
    assert (
        isinstance(unknown, app80.WordReadResult.Unknown) and unknown.raw_status == 42
    )
    write = app80.Command.WriteWord(
        address=app80.WordAddress(0x40001000), value=0x12345678
    )
    assert isinstance(
        expect(
            decode("memory_word", list(range(10)), write, {1: 1}),
            app80.ReplyValue.WriteWord,
        ).result,
        app80.WordWriteResult.Confirmed,
    )
    assert isinstance(
        expect(
            decode("memory_word", list(range(10)), write, {1: 0}),
            app80.ReplyValue.WriteWord,
        ).result,
        app80.WordWriteResult.Rejected,
    )
    unknown = expect(
        decode("memory_word", list(range(10)), write, {1: 42}),
        app80.ReplyValue.WriteWord,
    ).result
    assert (
        isinstance(unknown, app80.WordWriteResult.Unknown) and unknown.raw_status == 42
    )


def test_async_execution_preserves_typed_results_and_fences_foreign_replies() -> None:
    async def test() -> None:
        invocation = save()
        raw = bytearray(invocation.raw)
        raw[8] = 0
        session, script = await async_opened(exchange(invocation, raw))
        reply = await app80.execute(session, invocation)
        assert isinstance(
            expect(reply.value, app80.ReplyValue.ConfigurationSaved).status,
            app80.SaveStatus.Success,
        )
        assert session.state == usb.SessionState.Ready and script.remaining == 0
        session, script = await async_opened(exchange(invocation, bytes(33)))
        with pytest.raises(usb.ProtocolError):
            await app80.execute(session, invocation)
        assert (
            session.state == usb.SessionState.CompletionUnknown
            and script.remaining == 0
        )
        with pytest.raises(usb.CompletionUnknownError):
            await session.close()

    run(test)


def test_cancellation_during_publish_restores_the_fenced_session_owner() -> None:
    async def test() -> None:
        invocation = save()
        steps = [
            *config_write(0x241, invocation.raw[1:], page=3),
            *config_write(0x240, invocation.raw, page=3),
        ]
        steps[-1] = E.pending_read(36)
        session, script = await async_opened(steps)
        task = asyncio.create_task(app80.execute(session, invocation))
        await dispatched(script)
        assert session.state == usb.SessionState.InFlight
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert session.state == usb.SessionState.CompletionUnknown
        with pytest.raises(usb.CompletionUnknownError):
            await session.ping()

    run(test)


def test_session_limit_is_checked_before_staging_and_allows_a_smaller_host_limit() -> (
    None
):
    async def test() -> None:
        invocation = save()
        raw = bytearray(invocation.raw)
        raw[8] = 0
        # The definition's 512-byte limit is not the size of this 33-byte request.
        session, script = await async_opened(exchange(invocation, raw), limits=LIMITS)
        assert isinstance(
            expect(
                (await app80.execute(session, invocation)).value,
                app80.ReplyValue.ConfigurationSaved,
            ).status,
            app80.SaveStatus.Success,
        )
        assert script.remaining == 0
        limits = usb.FirmwareLimits(
            memory_read=256, memory_write=256, configuration=32, console=256
        )
        session, script = await async_opened(limits=limits)
        with pytest.raises(ValueError):
            await app80.execute(session, invocation)
        assert script.remaining == 0 and session.state == usb.SessionState.Ready

    run(test)
