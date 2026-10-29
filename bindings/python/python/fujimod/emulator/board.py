"""Typed host client for a Rust board and its execution session."""

from __future__ import annotations

import json
import sys
import weakref
from collections.abc import Buffer, Mapping, Sequence
from dataclasses import dataclass
from os import PathLike, fspath
from typing import Literal, Protocol, Self, TypedDict, TypeGuard, cast

from fujimod._native import emulator as _native

from .definition import Definition, Xp4Definition
from .processor import Processor

BootStatus = Literal["rec", "instruction-limit", "stopped"]
ExecutionPolicy = Literal["exact", "interactive"]


class BootReport(TypedDict):
    """Passive observation returned by :meth:`Board.boot_to_rec`."""

    status: BootStatus
    instructions: int
    virtual_ticks: int
    ui_mode: int
    system_mode: int
    menu_active: int
    pending_mode: int
    menu_node: int
    pending: bool
    setup_presses: int
    translation_cache_size_bytes: int
    failure: str | None


@dataclass(frozen=True)
class ScanoutFrame:
    """One passive read of a modeled display pipe."""

    raw: bytes
    rgb: bytes | None
    state: dict[str, object]
    width: int
    height: int


def _is_json_object(value: object) -> TypeGuard[dict[str, object]]:
    return isinstance(value, dict) and all(isinstance(key, str) for key in value)


def _json_object(value: object) -> dict[str, object]:
    if not _is_json_object(value):
        raise TypeError("Native JSON result must be a string-keyed object")
    return value


def _boot_report(value: object) -> BootReport:
    report = _json_object(value)
    status = report.get("status")
    if status not in ("rec", "instruction-limit", "stopped"):
        raise RuntimeError("Native boot report has an unknown status")
    failure = report.get("failure")
    if failure is not None and not isinstance(failure, str):
        raise RuntimeError("Native boot report failure must be text or None")
    result: BootReport = {
        "status": cast(BootStatus, status),
        "instructions": _report_integer(report, "instructions"),
        "virtual_ticks": _report_integer(report, "virtual_ticks"),
        "ui_mode": _report_integer(report, "ui_mode"),
        "system_mode": _report_integer(report, "system_mode"),
        "menu_active": _report_integer(report, "menu_active"),
        "pending_mode": _report_integer(report, "pending_mode"),
        "menu_node": _report_integer(report, "menu_node"),
        "pending": _report_boolean(report, "pending"),
        "setup_presses": _report_integer(report, "setup_presses"),
        "translation_cache_size_bytes": _report_integer(
            report, "translation_cache_size_bytes"
        ),
        "failure": failure,
    }
    return result


def _report_integer(report: dict[str, object], name: str) -> int:
    value = report.get(name)
    if type(value) is not int or value < 0:
        raise RuntimeError(f"Native boot report {name} must be a nonnegative integer")
    return value


def _report_boolean(report: dict[str, object], name: str) -> bool:
    value = report.get(name)
    if type(value) is not bool:
        raise RuntimeError(f"Native boot report {name} must be a boolean")
    return value


class Board:
    """Own one native board and its execution session.

    Construct a board from resolved graph facts and bytes whose identities
    match ``Definition.assets``. Use ``with`` or call :meth:`close` to release
    native execution resources promptly.
    """

    def __init__(
        self, definition: Definition | Xp4Definition, assets: Mapping[str, Buffer]
    ) -> None:
        if not isinstance(definition, (Definition, Xp4Definition)):
            raise TypeError("definition must be a Definition or Xp4Definition")
        if type(definition.quantum) is not int or definition.quantum < 1:
            raise ValueError("Board quantum must be a positive integer")
        if not isinstance(assets, Mapping):
            raise TypeError("assets must map declared asset IDs to bytes")
        if any(type(asset_id) is not str for asset_id in assets):
            raise TypeError("asset IDs must be strings")
        for asset_id, data in assets.items():
            if not isinstance(data, Buffer):
                raise TypeError(f"asset {asset_id} must be bytes-like")
        loaded = {asset_id: bytes(data) for asset_id, data in assets.items()}
        _native.verify_input_assets(
            json.dumps([asset.manifest() for asset in definition.assets]), loaded
        )
        images = []
        for image in definition.images:
            if image.asset not in loaded:
                raise ValueError("Image names an undeclared asset: " + image.asset)
            images.append((image, loaded[image.asset]))
        self.quantum = definition.quantum
        if isinstance(definition, Definition):
            if definition.engine not in ("unicorn", "hypervisor"):
                raise ValueError("engine must be 'unicorn' or 'hypervisor'")
            if definition.mode not in ("strict", "discovery"):
                raise ValueError("mode must be 'strict' or 'discovery'")
        if isinstance(definition, Xp4Definition):
            if any(image.core is not None for image, _ in images):
                raise ValueError("XP4 input images require physical addresses")
            if definition.storage is not None and definition.storage not in loaded:
                raise ValueError(
                    "Storage names an undeclared asset: " + definition.storage
                )
            self._native = _native.Board.from_xp4(
                json.dumps(dict(definition.options)),
                storage_image=None
                if definition.storage is None
                else loaded[definition.storage],
            )
        else:
            self._native = _native.Board(
                json.dumps(dict(definition.execution)),
                json.dumps(dict(definition.devices)),
                json.dumps(dict(definition.scene)),
                loaded,
                dict(definition.adapters),
                execution_mode=definition.mode,
                continue_refusals=definition.continue_refusals,
                engine=definition.engine,
            )
        try:
            for image, data in images:
                if image.core is None:
                    self._native.physical_write(image.address, data)
                else:
                    self._native.mem_write(image.core, image.address, data)
        except BaseException:
            self._native.close()
            raise
        self._adopt_native(self._native, self.quantum, boot_capable=False)

    @classmethod
    def from_inputs(
        cls,
        *,
        configuration_id: str,
        overlays: Sequence[str] = (),
        update: str | PathLike[str],
        boot_rom: str | PathLike[str],
        nand: str | PathLike[str] | None = None,
        settings: str | PathLike[str] | None = None,
        clock_seconds_2000: int | None = None,
        preferred_languages: Sequence[str] = (),
        camera: str | PathLike[str] | None = None,
    ) -> Self:
        """Boot a catalog runtime from files selected by the caller.

        ``configuration_id`` selects a catalog run configuration. Its body
        and firmware references are resolved by the catalog, not supplied as
        separate caller identities.

        ``overlays`` selects named catalog overlays in order. Each overlay
        must apply to the selected run configuration.

        ``update`` and ``boot_rom`` are required files. ``nand`` and
        ``settings`` supply optional persistent storage inputs. No file path
        is inferred from the selected configuration.

        ``clock_seconds_2000`` sets the real-time clock in seconds since
        2000-01-01, or uses the configuration's default if None. Must be
        an unsigned 32-bit integer if provided.

        ``preferred_languages`` supplies BCP 47 tags in preference order for
        stock first-start setup.

        ``camera`` is a camera directory holding the ``nand-<section>.bin.zst``
        files a configuration's power-on settings name, such as
        ``catalog/bodies/x70/cameras/reference``.
        """
        configuration_id = cls._identifier(configuration_id, "configuration_id")
        if isinstance(overlays, (str, bytes)) or not isinstance(overlays, Sequence):
            raise TypeError("overlays must be a sequence of catalog overlay IDs")
        selected_overlays = tuple(
            cls._identifier(overlay, "overlay ID") for overlay in overlays
        )
        if len(selected_overlays) != len(set(selected_overlays)):
            raise ValueError("overlays must contain unique catalog identities")
        update = cls._input_path(update, "update")
        boot_rom = cls._input_path(boot_rom, "boot_rom")
        nand = None if nand is None else cls._input_path(nand, "nand")
        settings = None if settings is None else cls._input_path(settings, "settings")
        camera = None if camera is None else fspath(camera)
        if isinstance(preferred_languages, (str, bytes)) or not isinstance(preferred_languages, Sequence):
            raise TypeError("preferred_languages must be a sequence of BCP 47 tags")
        if any(not isinstance(tag, str) for tag in preferred_languages):
            raise TypeError("preferred_languages must contain strings")
        if clock_seconds_2000 is not None:
            if type(clock_seconds_2000) is bool or type(clock_seconds_2000) is not int:
                raise TypeError("clock_seconds_2000 must be a non-bool integer or None")
            if not 0 <= clock_seconds_2000 <= 0xFFFFFFFF:
                raise ValueError("clock_seconds_2000 must be in 0..=0xFFFFFFFF")
        native = _native.Board.from_inputs(
            configuration_id=configuration_id,
            overlays=selected_overlays,
            update=update,
            boot_rom=boot_rom,
            nand=nand,
            settings=settings,
            clock_seconds_2000=clock_seconds_2000,
            preferred_languages=list(preferred_languages),
            camera=camera,
        )
        quantum = native.boot_quantum
        if type(quantum) is not int or quantum < 1:
            native.close()
            raise RuntimeError("Native runtime returned an invalid board quantum")
        instance = cls.__new__(cls)
        try:
            instance._adopt_native(native, quantum, boot_capable=True)
        except BaseException:
            native.close()
            raise
        return instance

    def _adopt_native(
        self, native: _native.Board, quantum: int, *, boot_capable: bool
    ) -> None:
        self._native = native
        self.quantum = quantum
        self._boot_capable = boot_capable
        self._closed = False
        self.cpus = tuple(
            Processor(self, core) for core in range(len(native.instructions))
        )
        self._session = Session(self)

    @staticmethod
    def _identifier(value: str, name: str) -> str:
        if (
            type(value) is not str
            or not value
            or len(value) > 128
            or not value[0].isascii()
            or not value[0].isalnum()
            or any(
                not character.isascii()
                or not (character.isalnum() or character in "-_.")
                for character in value
            )
            or value in (".", "..")
        ):
            raise ValueError(f"{name} must be a bounded identifier")
        return value

    @staticmethod
    def _input_path(value: str | PathLike[str], name: str) -> str:
        if not isinstance(value, (str, PathLike)):
            raise TypeError(f"{name} must be a filesystem path")
        path = fspath(value)
        if type(path) is not str or not path:
            raise ValueError(f"{name} must be a nonempty text path")
        return path

    def _ensure_open(self) -> None:
        if self._closed:
            raise RuntimeError("runtime is closed")

    @property
    def session(self) -> Session:
        """Typed external input and observation interface for this board.

        The facade shares this board's native owner. Closing the board also
        closes the session, and session methods reject calls after that point.
        """
        self._ensure_open()
        return self._session

    @property
    def closed(self) -> bool:
        """Whether :meth:`close` has released this board's native session."""
        return self._closed

    @property
    def execution_policy(self) -> ExecutionPolicy:
        """Current instruction-accounting policy for a graph-backed board."""
        self._ensure_open()
        policy = self._native.execution_policy()
        if policy not in ("exact", "interactive"):
            raise RuntimeError("Native board returned an unknown execution policy")
        return policy

    def set_execution_policy(self, policy: ExecutionPolicy) -> None:
        """Select instruction accounting for the next board execution.

        ``exact`` stops at requested instruction boundaries. ``interactive``
        permits backend-defined block-boundary overshoot. The board must be
        stopped before the policy can change.
        """
        self._ensure_open()
        if type(policy) is not str:
            raise TypeError("execution policy must be text")
        if policy not in ("exact", "interactive"):
            raise ValueError("execution policy must be 'exact' or 'interactive'")
        self._native.set_execution_policy(policy)

    @property
    def native(self) -> _native.Board:
        self._ensure_open()
        return self._native

    @property
    def instructions(self) -> int:
        self._ensure_open()
        return sum(self._native.instructions)

    @property
    def virtual_ticks(self) -> int:
        self._ensure_open()
        return self._native.now

    @property
    def quiescent(self) -> bool:
        self._ensure_open()
        return self._native.quiescent

    @property
    def failure(self) -> dict[str, object] | None:
        self._ensure_open()
        failure = json.loads(self._native.failure())
        if failure is None:
            return None
        if isinstance(failure, str):
            return {"message": failure}
        return _json_object(failure)

    def run(
        self,
        limit: int = 100000,
        *,
        pause_after: int | None = None,
        pause_at_tick: int | None = None,
        store_watches: list[tuple[int, int]] | None = None,
        device_watches: list[str] | None = None,
    ) -> bool:
        """Run within instruction and optional pause limits.

        ``store_watches`` contains half-open physical address ranges;
        ``device_watches`` contains nodes in the selected device graph.
        Watches apply only to this call. Return whether a watched store or
        device publication ended the continuation, including at the current tick.
        """
        self._ensure_open()
        if type(limit) is not int or limit < 0:
            raise ValueError("instruction limit must be a nonnegative integer")
        for name, value in (
            ("pause_after", pause_after),
            ("pause_at_tick", pause_at_tick),
        ):
            if value is not None and (type(value) is not int or value < 0):
                raise ValueError(f"{name} must be a nonnegative integer or None")
        # Stores name half-open canonical physical ranges. Device names are
        # nodes in the resolved graph. Watches last only for this host call.
        return self._native.run(
            limit,
            self.quantum,
            pause_after,
            pause_at_tick,
            store_watches,
            device_watches,
        )

    def run_until(self, tick: int) -> None:
        """Advance to an absolute guest tick."""
        self._ensure_open()
        if type(tick) is not int or tick < 0:
            raise ValueError("tick must be a nonnegative integer")
        self._native.run_until(tick, self.quantum)

    def boot_to_rec(self, instruction_limit: int = 2_000_000_000) -> BootReport:
        """Boot a catalog runtime through first start and return its report."""
        self._ensure_open()
        if not self._boot_capable:
            raise RuntimeError("boot_to_rec requires a board created by from_inputs")
        if type(instruction_limit) is not int or instruction_limit < 1:
            raise ValueError("instruction_limit must be a positive integer")
        return _boot_report(json.loads(self._native.boot_to_rec(instruction_limit)))

    def read_memory(self, address: int, size: int, cpu_id: int = 0) -> bytes:
        """Read a processor's virtual address space."""
        self._ensure_open()
        self._validate_range(address, size)
        self._validate_cpu(cpu_id)
        return self._native.mem_read(cpu_id, address, size)

    def write_memory(self, address: int, data: Buffer, cpu_id: int = 0) -> None:
        """Write a processor's virtual address space."""
        self._ensure_open()
        payload = bytes(data)
        self._validate_range(address, len(payload))
        self._validate_cpu(cpu_id)
        self._native.mem_write(cpu_id, address, payload)

    def read_physical(self, address: int, size: int) -> bytes:
        """Read canonical physical memory."""
        self._ensure_open()
        self._validate_range(address, size)
        return self._native.physical_read(address, size)

    def inspect_physical(self, address: int, size: int) -> bytes:
        """Read canonical physical memory that an active wait observes but does
        not depend on: stores to it do not end the wait."""
        self._ensure_open()
        self._validate_range(address, size)
        return self._native.physical_inspect(address, size)

    def read_physical_many(
        self, ranges: Sequence[tuple[int, int]]
    ) -> tuple[bytes, ...]:
        """Read bounded physical memory spans at one stopped execution point."""
        self._ensure_open()
        if isinstance(ranges, (str, bytes, bytearray)) or not isinstance(
            ranges, Sequence
        ):
            raise TypeError("ranges must be a sequence of (address, size) pairs")
        if len(ranges) > 4096:
            raise ValueError("at most 4096 physical memory ranges are supported")
        checked: list[tuple[int, int]] = []
        for row in ranges:
            if not isinstance(row, Sequence) or isinstance(
                row, (str, bytes, bytearray)
            ) or len(row) != 2:
                raise TypeError("each physical memory range must be an (address, size) pair")
            address, size = row
            self._validate_range(address, size)
            checked.append((address, size))
        return tuple(self._native.physical_read_many(checked))

    def write_physical(self, address: int, data: Buffer) -> None:
        """Write canonical physical memory."""
        self._ensure_open()
        payload = bytes(data)
        self._validate_range(address, len(payload))
        self._native.physical_write(address, payload)

    def state(self) -> dict[str, object]:
        """Return the current execution and device state."""
        self._ensure_open()
        return _json_object(json.loads(self._native.state()))

    def fingerprint(self) -> dict[str, object]:
        """Return a stable snapshot of execution-relevant state."""
        self._ensure_open()
        return _json_object(json.loads(self._native.fingerprint()))

    def checkpoint(
        self, previous: _native.Checkpoint | None = None
    ) -> _native.Checkpoint:
        """Capture a checkpoint, optionally reusing a previous checkpoint."""
        self._ensure_open()
        return self._native.checkpoint(previous)

    def restore_checkpoint(self, saved: _native.Checkpoint) -> None:
        """Restore an in-memory or portable checkpoint for this board."""
        self._ensure_open()
        self._native.restore_checkpoint(saved)

    def close(self) -> None:
        """Release the native board; repeated calls are safe."""
        if self._closed:
            return
        self._native.close()
        self._closed = True

    def __enter__(self) -> Self:
        self._ensure_open()
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    @staticmethod
    def _validate_range(address: int, size: int) -> None:
        if type(address) is not int or not 0 <= address < 1 << 64:
            raise ValueError("address must be an unsigned 64-bit integer")
        if type(size) is not int or not 0 <= size < 1 << 64:
            raise ValueError("size must be an unsigned 64-bit integer")
        if address + size > 1 << 64:
            raise ValueError("address range exceeds the 64-bit address space")

    def _validate_cpu(self, cpu_id: int) -> None:
        if type(cpu_id) is not int or not 0 <= cpu_id < len(self.cpus):
            raise ValueError("cpu_id must identify a processor on this board")


def make_board(
    definition: Definition | Xp4Definition, assets: Mapping[str, Buffer]
) -> Board:
    """Construct a board from resolved facts and matching asset bytes."""
    return Board(definition, assets)


class NativeBoardOwner(Protocol):
    """Any board object that owns one native emulator board."""

    @property
    def native(self) -> _native.Board: ...


class Session:
    """Typed external inputs and passive observations for a native board.

    Obtain a facade from ``board.session``. It does not own a second native
    runtime: closing the board invalidates every session operation.
    """

    def __init__(self, board: NativeBoardOwner) -> None:
        if not isinstance(board.native, _native.Board):
            raise TypeError("board must own a native emulator board")
        self._board_ref: weakref.ReferenceType[NativeBoardOwner] = weakref.ref(board)

    def _native_board(self) -> _native.Board:
        board = self._board_ref()
        if board is None:
            raise RuntimeError("board no longer exists")
        return board.native

    def adc_samples(self, node: str) -> tuple[int, ...]:
        """Return the current externally supplied ADC sample channels."""
        return tuple(self._native_board().adc_samples(self._node(node)))

    def xp5_gpio_input(self, port: int, pin: int, high: bool) -> None:
        """Drive a physical input pad on an XP5 board."""
        if type(port) is not int or type(pin) is not int or type(high) is not bool:
            raise TypeError("XP5 GPIO input needs an integer port/pin and boolean level")
        self._native_board().xp5_gpio_input(port, pin, high)

    def xp5_gpio_level(self, port: int, pin: int) -> bool:
        """Observe a physical input pad on an XP5 board."""
        if type(port) is not int or type(pin) is not int:
            raise TypeError("XP5 GPIO input needs an integer port and pin")
        return self._native_board().xp5_gpio_level(port, pin)

    def xp5_key(self, at_ns: int, key: str, pressed: bool) -> None:
        """Queue a resistor-key edge at an absolute guest nanosecond."""
        if type(at_ns) is not int or not 0 <= at_ns < 1 << 64 or type(pressed) is not bool:
            raise TypeError("XP5 key edge needs an unsigned time and boolean pressed state")
        if not isinstance(key, str) or not key:
            raise TypeError("XP5 key must have a name")
        self._native_board().xp5_key(at_ns, key, pressed)

    def set_adc_samples(self, node: str, samples: Sequence[int]) -> None:
        """Replace the ADC channels with unsigned 16-bit sample values."""
        if isinstance(samples, (str, bytes, bytearray)) or not isinstance(
            samples, Sequence
        ):
            raise TypeError("samples must be a sequence of unsigned 16-bit integers")
        values = tuple(samples)
        if any(type(value) is not int or not 0 <= value <= 0xFFFF for value in values):
            raise ValueError("ADC samples must be unsigned 16-bit integers")
        self._native_board().set_adc_samples(self._node(node), list(values))

    def input_graph(self) -> dict[str, object]:
        """Return input-capable nodes and their checked graph mappings."""
        return _json_object(json.loads(self._native_board().input_graph()))

    def input_level(self, name: str, high: bool) -> None:
        """Drive a named logical input high or low."""
        if type(high) is not bool:
            raise TypeError("high must be a bool")
        self._native_board().input_level(self._node(name), high)

    def inputs_changed(self, node: str) -> None:
        """Notify a device after a caller-owned external input changes."""
        self._native_board().inputs_changed(self._node(node))

    def gpio_input(self, name: str, port: int, pin: int, high: bool) -> None:
        """Drive one externally owned GPIO input pad."""
        self._native_board().gpio_input(
            self._node(name),
            self._byte(port, "port"),
            self._byte(pin, "pin"),
            self._bool(high),
        )

    def gpio_input_levels(self, name: str) -> tuple[int, ...]:
        """Return the 16 input-level bitmaps for a GPIO node."""
        return tuple(self._native_board().gpio_input_levels(self._node(name)))

    def gpio_level(self, name: str, port: int, pin: int) -> bool:
        """Read the resolved level at one modeled GPIO pad."""
        return self._native_board().gpio_level(
            self._node(name), self._byte(port, "port"), self._byte(pin, "pin")
        )

    def gpio_pad_state(self, name: str, port: int, pin: int) -> dict[str, object]:
        """Return the modeled electrical state for one GPIO pad."""
        state = self._native_board().gpio_pad_state(
            self._node(name), self._byte(port, "port"), self._byte(pin, "pin")
        )
        return _json_object(json.loads(state))

    def controls_connected(self) -> bool:
        """Return whether physical-control inputs are configured."""
        return self._native_board().controls_connected()

    def control_state(self) -> dict[str, object]:
        """Return available controls, current positions and queued transitions."""
        return _json_object(json.loads(self._native_board().control_state()))

    def set_position(self, control_id: str, position: str) -> None:
        """Set a named physical control to one of its declared positions."""
        self._native_board().set_position(
            self._control(control_id), self._position(position)
        )

    def press(self, control_id: str, *, stage: str | None = None) -> None:
        """Queue a checked physical press, optionally to a named stage."""
        if stage is not None:
            stage = self._position(stage)
        self._native_board().contact_input(self._control(control_id), stage, False)

    def release(self, control_id: str) -> None:
        """Queue a checked release after any pending physical stages."""
        self._native_board().contact_input(self._control(control_id), None, True)

    def rotate(self, control_id: str, steps: int) -> None:
        """Rotate an encoder by a signed number of logical detents."""
        if type(steps) is not int or not -(1 << 31) <= steps < (1 << 31):
            raise ValueError("steps must be a signed 32-bit integer")
        self._native_board().rotate_control(self._control(control_id), steps)

    def scanout_state(self, node: str) -> dict[str, object]:
        """Return passive display-pipe register and format metadata."""
        return _json_object(
            json.loads(self._native_board().scanout_state(self._node(node)))
        )

    def scanout(
        self,
        node: str,
        instance: int = 0,
        *,
        pixels: bool = True,
        diagnostic_hashes: bool = False,
        main: bool = False,
    ) -> ScanoutFrame | None:
        """Read one modeled display pipe without advancing guest time.

        ``pixels=False`` returns register and source-byte observations without
        converting them to RGB. Set ``main=True`` to select the pipe's main
        scanout path.
        """
        instance = self._instance(instance)
        pixels = self._bool(pixels)
        diagnostic_hashes = self._bool(diagnostic_hashes)
        main = self._bool(main)
        native = self._native_board()
        result = (
            native.scanout_main(self._node(node), instance, pixels, diagnostic_hashes)
            if main
            else native.scanout(self._node(node), instance, pixels, diagnostic_hashes)
        )
        if result is None:
            return None
        raw, rgb, state, width, height = result
        return ScanoutFrame(
            raw=raw,
            rgb=rgb,
            state=_json_object(json.loads(state)),
            width=width,
            height=height,
        )

    def screenshot(self) -> bytes:
        """Return a PNG screenshot for a catalog runtime after it has a frame."""
        return self._native_board().screenshot()

    def set_image_rgb(
        self, name: str, width: int, height: int, rgb: Buffer
    ) -> None:
        """Replace a named external RGB image without advancing guest time."""
        if type(width) is not int or not 1 <= width <= sys.maxsize:
            raise ValueError("width must be a positive host-sized integer")
        if type(height) is not int or not 1 <= height <= sys.maxsize:
            raise ValueError("height must be a positive host-sized integer")
        payload = bytes(rgb)
        if len(payload) != width * height * 3:
            raise ValueError("RGB input must contain exactly width * height * 3 bytes")
        self._native_board().image_rgb(self._node(name), width, height, payload)

    def set_sensor_raw16(
        self, name: str, width: int, height: int, raw16: Buffer
    ) -> None:
        """Replace one sensor input with a little-endian U16 RAW raster."""
        if type(width) is not int or not 1 <= width <= sys.maxsize:
            raise ValueError("width must be a positive host-sized integer")
        if type(height) is not int or not 1 <= height <= sys.maxsize:
            raise ValueError("height must be a positive host-sized integer")
        payload = bytes(raw16)
        if len(payload) != width * height * 2:
            raise ValueError("RAW input must contain exactly width * height U16 samples")
        self._native_board().sensor_raw16(self._node(name), width, height, payload)

    def frame_state(self, node: str) -> dict[str, object]:
        """Return the current state of a clocked frame source."""
        return _json_object(
            json.loads(self._native_board().frame_state(self._node(node)))
        )

    @staticmethod
    def _node(value: str) -> str:
        if type(value) is not str or not value:
            raise ValueError("node name must be nonempty text")
        return value

    @staticmethod
    def _control(value: str) -> str:
        if type(value) is not str or not value:
            raise ValueError("control id must be nonempty text")
        return value

    @staticmethod
    def _position(value: str) -> str:
        if type(value) is not str or not value:
            raise ValueError("control position must be nonempty text")
        return value

    @staticmethod
    def _byte(value: int, name: str) -> int:
        if type(value) is not int or not 0 <= value <= 0xFF:
            raise ValueError(f"{name} must be an unsigned 8-bit integer")
        return value

    @staticmethod
    def _instance(value: int) -> int:
        if type(value) is not int or not 0 <= value <= sys.maxsize:
            raise ValueError("instance must be a nonnegative host-sized integer")
        return value

    @staticmethod
    def _bool(value: bool) -> bool:
        if type(value) is not bool:
            raise TypeError("value must be a bool")
        return value
