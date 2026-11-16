"""Synthetic instruction contracts for checked isolated AArch64 calls."""

import struct

import pytest
from fujimod.emulator.aarch64_harness import (
    Aarch64DriverHarness,
    Aarch64Image,
    Aarch64Inputs,
    StubContext,
)
from fujimod.emulator.driver_harness import DriverHarness

BASE = 0x1_0000_0000
CODE, DATA, SP, RETURN = BASE + 0x1000, BASE + 0x4000, BASE + 0x9000, BASE + 0xB000


def words(*values: int) -> bytes:
    return struct.pack("<" + "I" * len(values), *values)


def harness(
    code: bytes,
    *,
    patches: tuple[tuple[int, bytes], ...] | list[tuple[int, bytes]] = (),
    fixture: list[tuple[int, bytes]] | None = None,
) -> Aarch64DriverHarness:
    image = Aarch64Image("code", CODE, code)
    inputs = Aarch64Inputs(
        images=(image,),
        stack=(SP - 0x1000, SP + 64),
        stack_pointer=SP,
        return_address=RETURN,
        scratch=(BASE + 0xC000, BASE + 0x10000),
    )
    result = DriverHarness(
        inputs, ram_fixture=fixture or [(DATA, bytes(32))], patches=patches
    )

    assert isinstance(result, Aarch64DriverHarness)
    return result


@pytest.mark.parametrize('changes', [
    {'name': ''}, {'name': '../code'}, {'name': 'x' * 129},
    {'address': -1}, {'address': (1 << 64) - 1}, {'address': True},
    {'data': b''}, {'data': bytearray(4)}, {'executable': 1},
])
def test_image_retains_name_byte_address_and_execution_checks(changes) -> None:
    values = dict(name='code', address=CODE, data=words(0xD65F03C0))
    with pytest.raises(ValueError):
        Aarch64Image(**(values | changes))


def test_arguments_and_returns_preserve_all_64_bits() -> None:
    # Sum x0..x7, then return through x30.
    code = words(*(0x8B000000 | (r << 16) for r in range(1, 8)), 0xD65F03C0)
    with harness(code) as h:
        args = tuple(BASE + n for n in range(8))
        result = h.call(CODE, args=args, max_instructions=20)
        assert result["returned"] and result["return_value"] == sum(args)
        assert result["registers"]["sp"] == SP
        assert result["instructions"] == 8
        assert result["images"] == [{"name": "code", "address": CODE, "size": len(code)}]
        assert result["ram_fixture"] == [{"address": DATA, "size": 32}]
        assert result["allowed_code"] == [[CODE, CODE + len(code)]]


@pytest.mark.parametrize(
    "code, reason",
    [
        (words(0x91000673, 0xD65F03C0), "x19"),
        (words(0x910043FF, 0xD65F03C0), "sp"),
        (words(0x14000000), "Instruction limit"),
        (words(0xD5381000, 0xD65F03C0), "Unsupported instruction"),
        (words(0xD5181000, 0xD65F03C0), "Unsupported instruction"),
        (words(0xD503207F), "Unsupported instruction"),
        (words(0xD4000001), "Unsupported instruction"),
        (words(0xDAC10020), "Unsupported instruction"),
        (words(0xC8007C22), "exclusive store outside declared RAM"),
    ],
)
def test_strict_refusals(code: bytes, reason: str) -> None:
    with harness(code) as h:
        result = h.call(CODE, max_instructions=8)
        assert not result["returned"]
        assert reason in result["failure"]


def test_exclusive_increment_on_declared_ram() -> None:
    # retry: ldxr w2,[x1]; add w2,w2,#1; stxr w3,w2,[x1]; cbnz w3,retry; ret
    code = words(0x885F7C22, 0x11000442, 0x88037C22, 0x35FFFFA3, 0xD65F03C0)
    with harness(code, fixture=[(DATA, struct.pack("<I", 41) + bytes(28))]) as h:
        result = h.call(CODE, args=(0, DATA), max_instructions=20)
        assert result["failure"] is None and result["returned"]
        assert h.read(DATA, 4) == struct.pack("<I", 42)
        assert [(s["address"], s["size"]) for s in result["stores"]] == [(DATA, 4)]


def test_exclusive_access_outside_declared_ram_refuses() -> None:
    code = words(0x885F7C22, 0xD65F03C0)  # ldxr w2,[x1]; ret
    with harness(code) as h:
        result = h.call(CODE, args=(0, BASE + 0xA000), max_instructions=4)
        assert not result["returned"]
        assert "exclusive read outside declared RAM" in result["failure"]


def test_stack_alignment_rejected_before_execution() -> None:
    with harness(words(0xD65F03C0)) as h, pytest.raises(ValueError, match="16-byte"):
        h.call(CODE, sp=SP - 8)


def test_fetch_cannot_run_past_declared_code() -> None:
    with harness(words(0xD503201F)) as h:
        result = h.call(CODE, max_instructions=10)
        assert not result["returned"]
        assert "fetch" in result["failure"]
        assert result["instructions"] == 1


def test_pair_store_checks_both_transfers_before_either_mutates_memory() -> None:
    with harness(words(0xA9000820, 0xD65F03C0)) as h:  # stp x0,x2,[x1]
        before = h.read(DATA, 32)
        result = h.call(
            CODE, args=(BASE + 3, DATA, BASE + 7), allowed_stores=[(DATA, DATA + 8)]
        )
        assert not result["returned"] and "store" in result["failure"]
        assert h.read(DATA, 32) == before
        assert result["stores"] == [] and result["instructions"] == 0


def test_permitted_store_records_actual_value_and_limit_prevents_next_store() -> None:
    with harness(words(0xF9000020, 0xF9000420, 0xD65F03C0)) as h:
        result = h.call(
            CODE,
            args=(BASE + 7, DATA),
            allowed_stores=[(DATA, DATA + 16)],
            store_limit=1,
        )
        assert not result["returned"] and "Store limit" in result["failure"]
        assert h.read(DATA, 16) == (BASE + 7).to_bytes(8, "little") + bytes(8)
        assert len(result["stores"]) == 1
        assert result["stores"][0]["value"] == BASE + 7


@pytest.mark.parametrize("opcode", [0xF9000020, 0xF9400020])
def test_mmio_refused_before_access(opcode: int) -> None:
    with harness(words(opcode, 0xD65F03C0)) as h:
        result = h.call(CODE, args=(7, 0xFA000000))
        assert not result["returned"] and "range" in result["failure"]
        assert result["instructions"] == 0


def test_stub_and_patch_identities_and_full_width_stub_return() -> None:
    code = words(0xAA1E03F3, 0x94000003, 0xAA1303FE, 0xD65F03C0, 0xD4200000)
    patches = [(CODE, words(0xAA1E03F1)), (CODE + 8, words(0xAA1103FE))]
    with harness(code, patches=patches) as h:
        h.stub(CODE + 16, returns=BASE + 11, label="synthetic callee")
        result = h.call(CODE)
        assert result["returned"] and result["return_value"] == BASE + 11
        assert len(result["patches"]) == 2
        assert result["stubs"][0]["label"] == "synthetic callee"
        assert len(result["stub_hits"]) == 1
        assert result["code"] == [{"address": CODE, "size": len(code)}]
        assert h.read(CODE, len(code)) == words(
            0xAA1E03F1, 0x94000003, 0xAA1103FE, 0xD65F03C0, 0xD4200000
        )


def test_callee_saved_pair_roundtrip_and_stack_store_log() -> None:
    code = words(0xA9BF7BFD, 0x910003FD, 0xA8C17BFD, 0xD65F03C0)
    with harness(code) as h:
        result = h.call(CODE)
        assert result["returned"] and result["instructions"] == 4
        assert len(result["stores"]) == 2


def test_fixture_is_only_readable_within_its_exact_declared_span() -> None:
    with harness(words(0xF9400020, 0xD65F03C0), fixture=[(DATA, b"1234")]) as h:
        result = h.call(CODE, args=(0, DATA))
        assert not result["returned"] and "read" in result["failure"]


def test_dynamic_stub_snapshots_and_declared_writes_are_recorded() -> None:
    code = words(0xAA1E03F1, 0x94000003, 0xAA1103FE, 0xD65F03C0, 0xD4200000)
    captured = []
    with harness(code, fixture=[(DATA, b"original" + bytes(24))]) as h:

        def copy(context: StubContext) -> int:
            captured.append(context.read(context.arg(0), 8))
            context.write(context.arg(1), captured[-1])
            return context.arg(2) + 1

        h.stub(CODE + 16, returns=copy, reads=[(0, 8)], label="explicit copy")
        result = h.call(CODE, args=(DATA, DATA + 8, BASE))
        assert result["returned"] and result["return_value"] == BASE + 1
        assert captured == [b"original"] and h.read(DATA + 8, 8) == b"original"
        assert result["stub_hits"][0]["writes"] == [{"address": DATA + 8, "size": 8}]
        assert result["stubs"][0]["dynamic"] is True
        second = h.call(CODE, args=(DATA + 8, DATA + 16, 7))
        assert second["returned"] and h.read(DATA + 16, 8) == b"original"


def test_dynamic_stub_forbidden_effects_are_validated_before_any_write() -> None:
    code = words(0xAA1E03F1, 0x94000003, 0xAA1103FE, 0xD65F03C0, 0xD4200000)
    with harness(code) as h:

        def writes(context: StubContext) -> int:
            context.write(DATA, b"allowed")
            context.write(CODE, b"forbidden")
            return 0

        h.stub(CODE + 16, returns=writes)
        result = h.call(CODE)
        assert not result["returned"] and "Stub write" in result["failure"]
        assert h.read(DATA, 8) == bytes(8)


def test_partial_frame_requires_explicit_fragment_exit_expectations() -> None:
    code = words(
        0xF84107F3, 0x910043FF, 0xD65F03C0
    )  # ldr x19,[sp],#16; add sp,sp,#16; ret
    with harness(code, fixture=[(SP, (0xAAAA).to_bytes(8, "little"))]) as h:
        result = h.call(
            CODE, registers={"x19": 0x38}, fragment_exit={"sp": SP + 32, "x19": 0xAAAA}
        )
        assert result["returned"] and result["registers"]["x19"] == 0xAAAA
        assert result["caller"]["fragment_exit"] == {"sp": SP + 32, "x19": 0xAAAA}


def test_simd_pair_guard_checks_full_width_before_first_store() -> None:
    with harness(words(0xAD000420, 0xD65F03C0)) as h:  # stp q0,q1,[x1]
        result = h.call(CODE, args=(0, DATA), allowed_stores=[(DATA, DATA + 16)])
        assert not result["returned"] and "store" in result["failure"]
        assert h.read(DATA, 32) == bytes(32) and result["stores"] == []


def test_declared_stop_before_instruction_with_explicit_fragment_exit() -> None:
    with harness(words(0x91000673, 0xD4200000)) as h:
        h.stop_at(CODE + 4, label="fragment end")
        result = h.call(CODE, fragment_exit={"x19": 1})
        assert not result["returned"] and result["failure"] is None
        assert result["stopped_at"] == CODE + 4 and result["instructions"] == 1
        assert result["stops"] == [{"address": CODE + 4, "label": "fragment end"}]


def test_argument_sized_stub_read_does_not_read_beyond_short_source() -> None:
    code = words(0xAA1E03F1, 0x94000003, 0xAA1103FE, 0xD65F03C0, 0xD4200000)
    with harness(code, fixture=[(DATA, b"short")]) as h:
        h.stub(
            CODE + 16,
            returns=lambda c: int.from_bytes(c.read(c.arg(0), c.arg(1)), "little"),
            reads=[(0, "x1")],
        )
        result = h.call(CODE, args=(DATA, 5))
        assert result["returned"] and result["return_value"] == int.from_bytes(
            b"short", "little"
        )


def test_indirect_stub_snapshots_capture_packet_before_stack_reuse() -> None:
    code = words(0xAA1E03F1, 0x94000003, 0xAA1103FE, 0xD65F03C0, 0xD4200000)
    captured = []
    fixture = [(DATA, bytes(8) + (DATA + 16).to_bytes(8, "little") + b"packet!!")]
    with harness(code, fixture=fixture) as h:

        def capture(c: StubContext) -> int:
            pointer = int.from_bytes(c.read(c.arg(1) + 8, 8), "little")
            captured.append(c.read(pointer, 8))
            return 0

        h.stub(
            CODE + 16, returns=capture, reads=[(1, 16), (1, {"indirect": 8, "size": 8})]
        )
        result = h.call(CODE, args=(0, DATA))
        assert result["returned"] and captured == [b"packet!!"]
        assert result["stub_hits"][0]["reads"][-1] == {"address": DATA + 16, "size": 8}


def test_fixture_inside_image_is_data_only_and_successful_calls_share_changes() -> None:
    code = words(0xF9400020, 0x91000400, 0xF9000020, 0xD65F03C0) + bytes(16)
    with harness(code, fixture=[(CODE + 16, (7).to_bytes(8, "little"))]) as h:
        for expected in (8, 9):
            result = h.call(CODE, args=(0, CODE + 16), allowed_code=[(CODE, CODE + 16)])
            assert result["returned"] and result["return_value"] == expected
        with pytest.raises(ValueError, match="fixture.*executable"):
            h.call(CODE)


def test_narrow_fetch_allowlist_does_not_make_remaining_image_writable() -> None:
    code = words(0xF9000020, 0xD65F03C0) + bytes(24)
    with harness(code, fixture=[(CODE + 16, bytes(8))]) as h:
        result = h.call(CODE, args=(7, CODE + 24), allowed_code=[(CODE, CODE + 8)])
        assert not result["returned"] and "store" in result["failure"]
        assert h.read(CODE + 24, 8) == bytes(8)


def test_vector_caller_state_is_initialized_for_each_call() -> None:
    code = words(0x3DC00020, 0xD65F03C0, 0x3D800020, 0xD65F03C0)
    with harness(code, fixture=[(DATA, bytes(range(32)))]) as h:
        assert h.call(CODE, args=(0, DATA))["returned"]
        result = h.call(CODE + 8, args=(0, DATA + 16))
        assert result["returned"] and h.read(DATA + 16, 16) == bytes(16)


def test_passive_observers_capture_intermediate_return_value_without_changing_execution() -> (
    None
):
    code = words(0xD2800060, 0xD2800700, 0xD65F03C0)  # x0=3; x0=0x38; ret
    with harness(code) as plain, harness(code) as observed:
        observed.observe_at(CODE + 4, label="intermediate result")
        expected = plain.call(CODE)
        actual = observed.call(CODE)
        assert actual["returned"] and actual["return_value"] == 0x38
        assert actual["observations"][0]["registers"]["x0"] == 3
        assert actual["observations"][0]["pc"] == CODE + 4
        assert actual["observers"] == [
            {"address": CODE + 4, "label": "intermediate result"}
        ]
        for key in ("registers", "instructions", "virtual_ticks", "stores"):
            assert actual[key] == expected[key]


def test_observer_bound_refuses_before_the_next_observed_instruction() -> None:
    with harness(words(0x91000400, 0x17FFFFFF)) as h:
        h.observe_at(CODE)
        result = h.call(CODE, observer_limit=2)
        assert not result["returned"] and "Observer limit" in result["failure"]
        assert [o["registers"]["x0"] for o in result["observations"]] == [0, 1]
        assert result["registers"]["x0"] == 2 and result["instructions"] == 4


@pytest.mark.parametrize("entry", [RETURN, CODE + 4])
def test_initial_entry_requires_an_admitted_instruction(entry: int) -> None:
    with harness(words(0xD65F03C0)) as h:
        with pytest.raises(ValueError, match="Initial entry.*executable"):
            h.call(entry)
        # An invalid attempted entry must not consume or poison the caller.
        result = h.call(CODE)
        assert result["returned"] and result["instructions"] == 1


def test_initial_admitted_stub_can_return_to_the_sentinel() -> None:
    with harness(words(0xD4200000)) as h:
        h.stub(CODE, returns=BASE + 9, label="entry substitution")
        result = h.call(CODE)
        assert result["returned"] and result["return_value"] == BASE + 9
        assert result["instructions"] == 0 and len(result["stub_hits"]) == 1


def test_declared_current_el_reads_the_model_and_records_the_observation() -> None:
    # MRS x0,CurrentEL; STR x0,[x1]; RET.
    with harness(words(0xD5384240, 0xF9000020, 0xD65F03C0)) as h:
        result = h.call(CODE, args=(0, DATA), system_reads=[("CurrentEL", 4)])
        assert result["returned"]
        assert result["registers"]["x0"] == 4
        assert h.read(DATA, 8) == (4).to_bytes(8, "little")
        assert result["system_read_model_values"] == [["CurrentEL", 4]]
        assert result["system_read_observations"] == [
            {
                "pc": CODE,
                "name": "CurrentEL",
                "encoding": 0xC212,
                "destination": 0,
                "value": 4,
            }
        ]


def test_system_read_mismatch_is_rejected_before_execution() -> None:
    with (
        harness(words(0xD5384240, 0xD65F03C0)) as h,
        pytest.raises((ValueError, RuntimeError), match="model value"),
    ):
        h.call(CODE, system_reads=[("CurrentEL", 0)])
