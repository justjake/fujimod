"""Git access shared by the commit and sync tools."""

import subprocess
import sys
import time
from pathlib import Path

# Other Git processes in a shared checkout hold .git/index.lock for moments at a
# time; only a crashed process leaves it for minutes.
LOCK_WAIT_SECONDS = 600
# Git keeps index.lock open while it works; an unopened lock this old was left by
# a killed process.
STALE_LOCK_SECONDS = 30


class Failure(Exception):
    pass


class Repository:
    def __init__(self, root: Path) -> None:
        self.root = root

    def git(
        self,
        *arguments: str,
        env: dict[str, str] | None = None,
        input: str | None = None,
        check: bool = True,
    ) -> subprocess.CompletedProcess[str]:
        start = time.monotonic()
        waiting = False
        while True:
            result = subprocess.run(
                ["git", *arguments],
                cwd=self.root,
                env=env,
                input=input,
                capture_output=True,
                text=True,
                check=False,
            )
            # Another Git process in the shared checkout holds the index briefly.
            if "index.lock': File exists" not in result.stderr:
                break
            if time.monotonic() - start > LOCK_WAIT_SECONDS:
                break
            self.clear_stale_index_lock()
            if not waiting and time.monotonic() - start > 10:
                print("waiting for another Git process's index.lock", file=sys.stderr)
                waiting = True
            time.sleep(0.2)
        if check and result.returncode:
            command = " ".join(arguments)
            raise Failure(f"git {command} failed:\n{result.stderr.strip()}")
        return result

    def clear_stale_index_lock(self) -> bool:
        lock = Path(
            subprocess.run(
                [
                    "git",
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-path",
                    "index.lock",
                ],
                cwd=self.root,
                capture_output=True,
                text=True,
                check=True,
            ).stdout.strip()
        )
        try:
            age = time.time() - lock.stat().st_mtime
        except FileNotFoundError:
            return False
        if age < STALE_LOCK_SECONDS:
            return False
        holders = subprocess.run(
            ["lsof", "-t", "--", str(lock)], capture_output=True, text=True, check=False
        ).stdout.split()
        if holders:
            return False
        lock.unlink(missing_ok=True)
        print(f"removed stale {lock} left {age:.0f} s ago", file=sys.stderr)
        return True

    def reset_index(self) -> None:
        """Make the shared index equal HEAD; files on disk are never touched.

        Nobody stages in a shared checkout's index (commits use a private one), so
        an entry that differs from HEAD is stale and would revert committed work.
        """
        self.git("reset", "-q", "--no-refresh")

    def file_object(self, path: str) -> str:
        """Object ID of a working-tree file after Git's filters (LFS pointers)."""
        return self.output("hash-object", "--", path)

    def output(self, *arguments: str) -> str:
        return self.git(*arguments).stdout.strip()

    def names(self, *arguments: str) -> list[str]:
        return self.git(*arguments, "--name-only", "-z").stdout.split("\0")[:-1]

    def branch(self) -> str:
        result = self.git("symbolic-ref", "-q", "HEAD", check=False)
        if result.returncode:
            raise Failure("HEAD is detached; check out the branch to commit to")
        return result.stdout.strip()

    @staticmethod
    def toplevel(path: Path) -> Path:
        return Path(
            subprocess.run(
                ["git", "rev-parse", "--show-toplevel"],
                cwd=path,
                capture_output=True,
                text=True,
                check=True,
            ).stdout.strip()
        )
