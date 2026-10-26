"""Camera binding tests over the Rust synthetic transport."""
from dataclasses import FrozenInstanceError

import pytest

from fujimod import _native
from fujimod.remote import Camera, Readiness, RemoteReadResult, control, menu, option


def camera():
    return Camera(_native.remote.Camera.synthetic())


def test_references_are_immutable_without_a_camera():
    item = menu(1, key=20, via=menu("shooting", key=10))
    assert item.id == "1"
    assert option(item, "ON").item is item
    with pytest.raises(FrozenInstanceError):
        item.key = 30


async def test_async_navigation_selection_readback_and_frame():
    remote = camera()
    item = menu("shooting", key=20)
    value = await remote.navigate(item, deadline=100)
    assert isinstance(value, RemoteReadResult)
    assert value.value.selected
    await remote.select(option(item, "ON"), deadline=100)
    await remote.navigate(item, deadline=100)
    await remote.press(control("selector.right"), deadline=100)
    read_result = await remote.wait_until(lambda value: value.value.selected,
        option(item, "ON"), deadline=100)
    assert read_result.value.value == "ON"
    before = remote.now
    frame = await remote.screenshot(deadline=100)
    assert frame.time > before
    assert frame.frame.width == 1
    remote.close()


def test_blocking_uses_the_same_actions_and_read_result_types():
    remote = camera()
    blocking = remote.blocking
    item = menu("shooting", key=20)
    blocking.select(option(item, "ON"), deadline=100)
    blocking.navigate(item, deadline=100)
    blocking.press(control("selector.right"), deadline=100)
    value = blocking.wait_until(lambda value: value.value.selected, option(item, "ON"), deadline=100)
    assert value.value.value == "ON"
    before = remote.now
    assert blocking.read(option(item, "ON")) == value
    assert isinstance(value, RemoteReadResult)
    assert remote.now == before
    assert blocking.screenshot(deadline=100).frame.height == 1


async def test_readiness_is_read_and_awaited_apart_from_state():
    remote = camera()
    readiness = await remote.readiness()
    assert isinstance(readiness, Readiness)
    assert readiness.ready and readiness.pending == []
    assert readiness.reads["stores"]
    await remote.wait_until_ready(deadline=100)
    assert remote.blocking.readiness() == readiness
    remote.blocking.wait_until_ready(deadline=100)
    remote.close()


async def test_task_cancellation_returns_camera_ownership():
    remote = camera()
    task = remote._task("screenshot", None, 100)
    with pytest.raises(RuntimeError, match="busy"):
        remote.blocking.read(control("selector.down"))
    assert not task.poll(lambda: None)
    task.cancel()
    assert (await remote.read(control("selector.down"))).value.supported
    remote.close()


async def test_ff80_camera_leases_and_returns_existing_session():
    from fujimod import usb
    from test_async import opened
    from test_stream import E, control as envelope

    session, script = await opened([
        E.control(envelope(0xC2, 2)), E.read(2, b"ok"),
        E.control(envelope(0xC4, 2)), E.read(2, b"ok"),
    ])
    remote = Camera(_native.remote.Camera.synthetic_ff80(session))
    with pytest.raises(usb.SessionBusyError):
        await session.ping()
    remote.close()
    assert await session.ping() == b"ok"
    await session.close()
    assert script.remaining == 0


async def test_abandoned_usb_owner_fences_remote_without_more_io():
    from test_async import opened

    session, script = await opened()
    remote = Camera(_native.remote.Camera.synthetic_ff80(session))
    session.abandon()
    with pytest.raises(RuntimeError):
        await remote.frame()
    assert script.remaining == 0
    remote.close()


async def test_async_observation_predicate_deadline_and_original_exception():
    remote = camera()
    reads = {'stores': [], 'devices': []}
    calls = []

    async def frozen():
        calls.append(remote.now)
        reads['stores'].append((16, 20))
        return None

    with pytest.raises(TimeoutError, match='frozen'):
        await remote.wait_for(frozen, deadline=3, reads=reads, condition='frozen')
    assert calls == [0, 1, 2, 3]
    assert reads['stores'] == [(16, 20)]
    failure = ValueError('unsafe observation')

    async def unsafe():
        raise failure

    with pytest.raises(ValueError) as caught:
        await remote.wait_for(unsafe, deadline=9, reads=reads, condition='unsafe')
    assert caught.value is failure
    assert remote.now == 3
    remote.close()


async def test_memory_wait_repeats_raw_reads_and_predicate_in_rust():
    remote = camera()
    calls = []

    def ready(reads):
        calls.append(reads)
        address, value = reads[0]
        assert address == 768
        assert value == (3).to_bytes(4, "little")
        return len(calls) == 3

    observed = await remote.wait_until_memory(ready, [(768, 4)], deadline=10,
                                               condition="synthetic mode settled")
    assert observed == [(768, (3).to_bytes(4, "little"))]
    assert len(calls) == 3
    remote.close()


async def test_stable_memory_wait_checks_bytes_and_repeats_in_rust():
    from test_async import opened
    from test_stream import E, control as envelope, memory_read

    value = (3).to_bytes(4, "little")
    exchanges = [exchange for _ in range(3) for exchange in memory_read(768, value)]
    exchanges.extend([E.control(envelope(0xC4, 2)), E.read(2, b"\x34\x12")])
    session, script = await opened(exchanges)
    remote = Camera(_native.remote.Camera.synthetic_ff80(session))
    observed = await remote.wait_until_stable_memory(
        [(768, 4)], stable_spans=[(768, 4)], equals=[(768, value)],
        stable_comparisons=2, deadline=1_000_000_000, condition="stable synthetic bytes",
    )
    assert observed == [(768, value)]
    remote.close()
    await session.close()
    assert script.remaining == 0


async def test_staged_shutter_and_position_inputs():
    remote = camera()
    await remote.set_input(control('shutter_button'), 'stage', stage='half_press', deadline=10)
    await remote.set_input(control('shutter_button'), 'stage', stage='full_press', deadline=10)
    await remote.set_input(control('shutter_button'), 'released', deadline=10)
    await remote.set_input(control('position_dial'), 'position', position='A', deadline=10)
    remote.close()


async def test_touch_reaches_the_remote_and_awaits_the_decoded_report():
    remote = camera()
    screen = control("touchscreen")
    with pytest.raises(RuntimeError, match="No finger is down"):
        await remote.touch_end(screen, deadline=10)
    with pytest.raises(RuntimeError, match="outside"):
        await remote.tap(screen, 720, 0, deadline=10)
    with pytest.raises(RuntimeError, match="input acceptance"):
        await remote.tap(screen, 10, 20, deadline=3)
    with pytest.raises(RuntimeError, match="already down"):
        remote.blocking.touch_start(screen, 10, 20, deadline=10)
    with pytest.raises(RuntimeError, match="input acceptance"):
        remote.blocking.touch_move(screen, 11, 20, deadline=6)
    # The synthetic report holds no contact, so lifting the finger is accepted.
    remote.blocking.touch_end(screen, deadline=10)
    remote.close()


async def test_operation_scoped_camera_returns_usb_between_wait_and_observation():
    from fujimod import usb
    from fujimod.remote import _ScopedNative
    from test_async import opened
    from test_stream import E, control as envelope, memory_read

    session, script = await opened([
        E.control(envelope(0xC2, 2)), E.read(2, b'ok'),
        *memory_read(768, (3).to_bytes(4, 'little')),
        *memory_read(768, (3).to_bytes(4, 'little')),
        *memory_read(768, (3).to_bytes(4, 'little')),
        E.control(envelope(0xC2, 2)), E.read(2, b'ok'),
        E.control(envelope(0xC4, 2)), E.read(2, b'ok'),
    ])
    remote = Camera(_ScopedNative(lambda: _native.remote.Camera.synthetic_ff80(session)))
    deadline = remote.now + 1_000_000_000
    await remote.wait({'stores': [], 'devices': []}, deadline=deadline)
    assert await session.ping() == b'ok'
    value = (3).to_bytes(4, 'little')
    assert await remote.wait_until_stable_memory(
        [(768, 4)], stable_spans=[(768, 4)], equals=[(768, value)],
        stable_comparisons=2, deadline=deadline, condition='stable synthetic bytes',
    ) == [(768, value)]
    assert await session.ping() == b'ok'
    await remote.wait({'stores': [], 'devices': []}, deadline=deadline)
    remote.close()
    await session.close()
    assert script.remaining == 0


async def test_observed_board_device_names_resolve_before_waiting():
    remote = camera()
    await remote.wait({'stores': [], 'devices': ['scanout']}, deadline=3)
    assert remote.now == 1
    with pytest.raises(ValueError, match='Unknown observed device'):
        await remote.wait({'stores': [], 'devices': ['absent']}, deadline=3)
    assert (await remote.read(control('selector.down'))).value.supported
    remote.close()
