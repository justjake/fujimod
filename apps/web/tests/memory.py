"""Host browser RSS and macOS physical charge, sampled every 100 ms.

This measures the headed host browser, not iPhone hardware. WebKit XPC
processes may be launchd children; include new processes in this browser bundle
and exclude those present before the run. Run one instance of the bundle at a
time. RSS, physical charge and Wasm capacity are distinct measurements.
"""

import asyncio
import ctypes
import sys
from pathlib import Path
from typing import TypedDict


class MemoryReport(TypedDict, total=False):
    peak_renderer_rss_bytes: int
    peak_content_rss_bytes: int
    peak_browser_rss_bytes: int
    peak_rss_bytes: int
    peak_processes: list[tuple[int, int, str]]
    peak_browser_physical_footprint_bytes: int
    footprint_processes: list[tuple[int, int | None, str]]
    memory_samples: int


async def processes() -> list[tuple[int, int, int, str]]:
    process = await asyncio.create_subprocess_exec(
        "ps", "-axo", "pid=,ppid=,rss=,comm=", stdout=asyncio.subprocess.PIPE
    )
    stdout, _ = await process.communicate()
    return [
        (int(pid), int(parent), int(rss) * 1024, command)
        for pid, parent, rss, command in (
            line.split(None, 3) for line in stdout.decode().splitlines()
        )
    ]


if sys.platform == "darwin":
    _proc = ctypes.CDLL("/usr/lib/libproc.dylib")
    _proc.proc_pid_rusage.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]
    _proc.proc_pid_rusage.restype = ctypes.c_int


def physical_footprint(pid: int) -> int | None:
    """macOS RUSAGE_INFO_V0 physical charge, including compressed pages.

    Unlike summed RSS, this excludes clean shared framework/zero pages. The
    two measurements stay separate; a footprint pass is not an RSS pass.
    """
    if sys.platform != "darwin":
        return None
    # sys/resource.h: 16-byte UUID, then ten u64 fields; footprint is field 8.
    usage = ctypes.create_string_buffer(96)
    if _proc.proc_pid_rusage(pid, 0, usage):
        return None
    return int.from_bytes(usage.raw[72:80], "little")


async def memory_samples(
    root: int,
    result: MemoryReport,
    stop: asyncio.Event,
    baseline: set[int],
    webkit_directory: str,
) -> None:
    while not stop.is_set():
        rows = await processes()
        children = {root}
        while True:
            expanded = children | {
                pid for pid, parent, _, _ in rows if parent in children
            }
            if expanded == children:
                break
            children = expanded
        if webkit_directory:
            children.update(
                pid
                for pid, _, _, command in rows
                if pid not in baseline and command.startswith(webkit_directory)
            )
        selected = [
            (pid, rss, Path(command).name)
            for pid, _, rss, command in rows
            if pid in children
        ]
        total = sum(rss for _, rss, _ in selected)
        content = sum(
            rss
            for _, rss, name in selected
            if "WebContent" in name or "Renderer" in name
        )
        renderer = max(
            (
                rss
                for _, rss, name in selected
                if "WebContent" in name or "Renderer" in name
            ),
            default=0,
        )
        result["peak_renderer_rss_bytes"] = max(
            result.get("peak_renderer_rss_bytes", 0), renderer
        )
        memory_without_driver = sum(rss for pid, rss, _ in selected if pid != root)
        result["peak_content_rss_bytes"] = max(
            result.get("peak_content_rss_bytes", 0), content
        )
        result["peak_browser_rss_bytes"] = max(
            result.get("peak_browser_rss_bytes", 0), memory_without_driver
        )
        if total > result.get("peak_rss_bytes", 0):
            result.update(peak_rss_bytes=total, peak_processes=selected)
        footprints = [
            (pid, physical_footprint(pid), name)
            for pid, _, name in selected
            if pid != root
        ]
        if footprints and all(value is not None for _, value, _ in footprints):
            charge = sum(value for _, value, _ in footprints if value is not None)
            if charge > result.get("peak_browser_physical_footprint_bytes", 0):
                result.update(
                    peak_browser_physical_footprint_bytes=charge,
                    footprint_processes=footprints,
                )
        result["memory_samples"] = result.get("memory_samples", 0) + 1
        await asyncio.sleep(0.1)
