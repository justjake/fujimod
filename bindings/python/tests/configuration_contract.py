"""Shared native/Pyodide configuration command contract."""

from contract_assertions import expect
from fujimod.usb import app80


def check_configuration_values() -> bool:
    import json

    from fujimod import catalog
    from fujimod.usb import app80 as a

    layouts = [
        ("configuration_flags", 11),
        ("configuration_mode", 2),
        ("configuration_pack_options", 4),
        ("configuration_read_options", 4),
        ("adjustment_configuration", 5),
        ("select_adjustment_mode", 1),
    ]
    slots = {name: [(i * 5 + 7) % 32 for i in range(n)] for name, n in layouts}
    doc = {
        "schema": 1,
        "firmware": {
            "id": "config-b",
            "model": "Synthetic",
            "version": "2",
            "sha256": "ab" * 32,
        },
        "mailbox": {"page": 3, "offset": 576, "configuration_limit": 512},
        "entries": [
            {
                "code": 200 + n,
                "label": name,
                "codec": name,
                "slots": slots[name],
                "enabled_modes": 4,
                "next_mode": 0,
                "firmware_kind": 1,
                "handler": 0,
                "parameter_table": 0,
            }
            for n, (name, _) in enumerate(layouts)
        ],
    }
    service = a.Service(
        catalog.service.Definition.from_json(
            json.dumps(doc), catalog.FirmwareIdentity("config-b", "ab" * 32)
        )
    )
    modes = catalog.service.ServiceModes(4)

    def prepare(
        command: app80.Command, codec: str, expected: list[int]
    ) -> app80.Invocation:
        p = service.prepare(modes, command)
        assert [p.raw[1 + i] for i in slots[codec]] == expected
        return p

    for bank, b in [(a.ConfigurationBank.Bank0, 0), (a.ConfigurationBank.Bank1, 1)]:
        for option, o in [(a.Toggle.Disabled, 0), (a.Toggle.Enabled, 1)]:
            p = prepare(
                a.Command.ReadConfigurationFlags(bank=bank, option=option),
                "configuration_flags",
                [0, b, o] + [255] * 8,
            )
            value = p.decode(p.raw).value
            assert isinstance(value, a.ReplyValue.ConfigurationFlags)
            assert bytes(value.raw_data) == bytes([255] * 8)
            data = bytearray(p.raw)
            expected = bytes([0, 1, 127, 128, 253, 254, 255, 42])
            for i, byte in enumerate(expected):
                data[1 + slots["configuration_flags"][3 + i]] = byte
            assert (
                bytes(
                    expect(
                        p.decode(data).value, a.ReplyValue.ConfigurationFlags
                    ).raw_data
                )
                == expected
            )
            p = prepare(
                a.Command.ApplyConfigurationFlags(bank=bank, option=option),
                "configuration_flags",
                [1, b, o] + [0] * 8,
            )
            assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
    for flag, f in [(a.Toggle.Disabled, 0), (a.Toggle.Enabled, 1)]:
        for m in range(4):
            p = prepare(
                a.Command.SelectConfigurationMode(
                    flag=flag, mode=getattr(a.ConfigurationMode, f"Mode{m}")
                ),
                "configuration_mode",
                [f, m],
            )
            assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
        for option, o in [(a.Toggle.Disabled, 0), (a.Toggle.Enabled, 1)]:
            for upper, lower in [(0, 255), (255, 0), (137, 171)]:
                p = prepare(
                    a.Command.PackConfigurationOptions(
                        flag=flag, option=option, upper=upper, lower=lower
                    ),
                    "configuration_pack_options",
                    [f, o, upper, lower],
                )
                assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
    p = prepare(
        a.Command.ReadConfigurationOptions(), "configuration_read_options", [255] * 4
    )
    assert (
        expect(p.decode(p.raw).value, a.ReplyValue.ConfigurationOptions).raw_value
        == 0xFFFFFFFF
    )
    data = bytearray(p.raw)
    for i, byte in enumerate([137, 171, 205, 239]):
        data[1 + slots["configuration_read_options"][i]] = byte
    assert (
        expect(p.decode(data).value, a.ReplyValue.ConfigurationOptions).raw_value
        == 0x89ABCDEF
    )
    for create in [
        lambda: a.Command.ReadConfigurationFlags(bank=1, option=a.Toggle.Enabled),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        lambda: a.Command.SelectConfigurationMode(
            flag=a.Toggle.Enabled,
            mode=a.ConfigurationBank.Bank0,  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        ),
        lambda: a.Command.PackConfigurationOptions(
            flag=a.Toggle.Enabled,
            option=1,  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
            upper=0,
            lower=0,
        ),
    ]:
        try:
            create()
        except TypeError:
            pass
        else:
            raise AssertionError("invalid configuration selector type accepted")
    for raw in [0, 15361, 65535]:
        try:
            a.AdjustmentDuration(raw)
        except a.CommandError as error:
            assert isinstance(error.reason, a.CommandFailure.Argument)
            assert error.reason.kind == a.ArgumentKind.AdjustmentDuration
        else:
            raise AssertionError("invalid adjustment duration accepted")
    for profile, selector in [
        (getattr(a.AdjustmentProfile, f"Profile{n}"), n) for n in range(7)
    ]:
        for seconds, minutes, remainder in [
            (1, 0, 1),
            (60, 1, 0),
            (61, 1, 1),
            (15360, 255, 60),
        ]:
            duration = a.AdjustmentDuration(seconds)
            assert duration.value == seconds
            p = prepare(
                a.Command.AdjustmentConfiguration(
                    action=a.AdjustmentConfigurationAction.Apply(
                        profile=profile, duration=duration
                    )
                ),
                "adjustment_configuration",
                [1, selector, minutes, remainder, 0],
            )
            assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
    p = prepare(
        a.Command.AdjustmentConfiguration(
            action=a.AdjustmentConfigurationAction.ReadStatus()
        ),
        "adjustment_configuration",
        [0, 0, 0, 0, 255],
    )
    for raw, cls in [
        (0, a.AdjustmentConfigurationState.NotConfigured),
        (1, a.AdjustmentConfigurationState.Configured),
        (255, a.AdjustmentConfigurationState.Unknown),
    ]:
        data = bytearray(p.raw)
        data[1 + slots["adjustment_configuration"][4]] = raw
        value = p.decode(data).value
        assert isinstance(value, a.ReplyValue.AdjustmentConfiguration) and isinstance(
            value.state, cls
        )
        if raw == 255:
            assert (
                expect(value.state, a.AdjustmentConfigurationState.Unknown).raw_status
                == 255
            )
    for mode, selector in [(a.AdjustmentMode.Mode8, 1), (a.AdjustmentMode.Mode16, 0)]:
        p = prepare(
            a.Command.SelectAdjustmentMode(mode=mode),
            "select_adjustment_mode",
            [selector],
        )
        assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
    try:
        a.Command.SelectAdjustmentMode(mode=a.ConfigurationMode.Mode0)  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    except TypeError:
        pass
    else:
        raise AssertionError("foreign mode enum accepted")
    return True
