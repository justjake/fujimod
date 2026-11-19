"""Keep a shared checkout's branch at origin without disturbing anyone's edits.

    python sync.py [--repo PATH]
    python sync.py --onto main [--repo PATH]
    python sync.py --resolved PATH...
    python sync.py --install-service [--repo PATH] [--interval SECONDS]

When origin is ahead, the branch and working tree move to origin's commit and
the shared index is reset to it. The index of a shared checkout only mirrors
HEAD: commits are built in a private index, so nobody stages here, and an entry
that differs from HEAD is stale. A file someone has edited that origin also
changed is merged three ways with ``git merge-file``: the old commit's version
is the base, the file on disk is ours and origin's version is theirs, so the edit
survives on top of the new commit. Only a file whose merge overlaps, or that is
binary, LFS, added or deleted on one side, keeps its local content untouched;
it is recorded as a sync conflict, reported, and refused by the commit tool
until its owner merges it and runs ``--resolved``.

``--onto main`` rebases a topic branch: after catching up with the branch's
own ``origin`` copy, each commit not on ``origin/main`` is replayed onto it
with ``git merge-tree`` (nothing touches the working tree until every commit
applies), the working tree moves to the result carrying uncommitted edits the
same three-way way, and the branch is pushed with ``--force-with-lease``. A
commit that does not apply cleanly stops the rebase with nothing changed.

``--install-service`` installs a macOS launchd agent that runs the sync every
``--interval`` seconds; a run with origin unchanged costs one ``ls-remote``.
"""

import argparse
import fcntl
import json
import os
import plistlib
import subprocess
import sys
import tempfile
from collections.abc import Generator
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path

from repository import Failure, Repository

GITLINK = "160000"
CONFLICTS = "fujimod-sync-conflicts.json"
LOCK = "fujimod-sync.lock"


@dataclass
class Result:
    head: str
    merged: list[str] = field(default_factory=list)
    conflicts: list[str] = field(default_factory=list)


@contextmanager
def locked(repository: Repository) -> Generator[None]:
    """Serialize syncs and commits in one checkout."""
    path = Path(repository.root, repository.output("rev-parse", "--git-path", LOCK))
    with path.open("w") as handle:
        fcntl.flock(handle, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(handle, fcntl.LOCK_UN)


def conflicts_path(repository: Repository) -> Path:
    return Path(
        repository.root, repository.output("rev-parse", "--git-path", CONFLICTS)
    )


def load_conflicts(repository: Repository) -> dict[str, dict[str, str]]:
    path = conflicts_path(repository)
    if not path.exists():
        return {}
    loaded: object = json.loads(path.read_text())
    if not isinstance(loaded, dict):
        raise Failure(f"{path} is not a JSON object")
    return {
        str(name): {str(key): str(value) for key, value in dict(record).items()}
        for name, record in loaded.items()
    }


def save_conflicts(
    repository: Repository, conflicts: dict[str, dict[str, str]]
) -> None:
    path = conflicts_path(repository)
    if conflicts:
        path.write_text(json.dumps(conflicts, indent=2, sort_keys=True) + "\n")
    else:
        path.unlink(missing_ok=True)


def resolve(repository: Repository, paths: list[str]) -> None:
    conflicts = load_conflicts(repository)
    unknown = [path for path in paths if path not in conflicts]
    if unknown:
        raise Failure("not recorded as sync conflicts: " + ", ".join(unknown))
    for path in paths:
        del conflicts[path]
    save_conflicts(repository, conflicts)


def entry(repository: Repository, tree: str, path: str) -> tuple[str, str] | None:
    """(mode, object) of a path in a commit, or None when absent."""
    line = repository.output("ls-tree", "-z", tree, "--", path).rstrip("\0")
    if not line:
        return None
    meta, _ = line.split("\t", 1)
    mode, _, obj = meta.split()
    return mode, obj


def blob(repository: Repository, obj: str) -> bytes:
    return subprocess.run(
        ["git", "cat-file", "blob", obj],
        cwd=repository.root,
        capture_output=True,
        check=True,
    ).stdout


def is_text(repository: Repository, path: str, *contents: bytes) -> bool:
    attribute = repository.output("check-attr", "filter", "--", path)
    if attribute.endswith(": lfs"):
        return False
    return all(b"\0" not in content[:8000] for content in contents)


def merge(base: bytes, ours: bytes, theirs: bytes) -> bytes | None:
    """Three-way merge; None when the edits overlap."""
    with tempfile.TemporaryDirectory() as scratch:
        files = []
        for name, content in (("ours", ours), ("base", base), ("theirs", theirs)):
            file = Path(scratch, name)
            file.write_bytes(content)
            files.append(str(file))
        result = subprocess.run(
            ["git", "merge-file", "-p", *files], capture_output=True, check=False
        )
    if result.returncode < 0 or result.returncode > 127:
        raise Failure(f"git merge-file failed: {result.stderr.decode().strip()}")
    return None if result.returncode else result.stdout


def hash_blob(repository: Repository, content: bytes) -> str:
    return (
        subprocess
        .run(
            ["git", "hash-object", "-w", "--stdin"],
            cwd=repository.root,
            input=content,
            capture_output=True,
            check=True,
        )
        .stdout.decode()
        .strip()
    )


def ancestor(repository: Repository, older: str, newer: str) -> bool:
    return (
        repository.git(
            "merge-base", "--is-ancestor", older, newer, check=False
        ).returncode
        == 0
    )


def remote_head(repository: Repository, ref: str) -> str:
    output = repository.output("ls-remote", "origin", ref)
    if not output:
        raise Failure(f"origin has no {ref}")
    return output.split()[0]


def sync(repository: Repository) -> Result:
    with locked(repository):
        return update(repository)


def fetch(repository: Repository, name: str) -> str | None:
    """Origin's commit for branch ``name``, or None when origin has no such branch."""
    fetched = repository.git(
        "fetch", "-q", "origin", f"+refs/heads/{name}:refs/remotes/origin/{name}",
        check=False,
    )
    if fetched.returncode:
        if not repository.output("ls-remote", "origin", f"refs/heads/{name}"):
            return None
        raise Failure(f"git fetch origin {name} failed:\n{fetched.stderr.strip()}")
    return repository.output("rev-parse", f"refs/remotes/origin/{name}")


def update(repository: Repository) -> Result:
    """Bring the branch to origin; the caller holds the sync lock.

    A branch origin does not have yet (a topic not pushed) is left as it is.
    """
    ref = repository.branch()
    name = ref.removeprefix("refs/heads/")
    current = repository.output("rev-parse", "HEAD")
    remote = fetch(repository, name)
    if remote is None:
        return Result(current)
    if remote == current or ancestor(repository, remote, current):
        return Result(current)
    if not ancestor(repository, current, remote):
        raise Failure(
            f"local {name} {current[:10]} and origin {remote[:10]} have diverged; "
            "resolve that before syncing"
        )
    return advance(repository, ref, current, remote)


@dataclass
class Rebased:
    head: str
    replayed: int
    pushed: bool
    moved: Result | None = None


def replay(repository: Repository, onto: str, commits: list[str]) -> str:
    """Copy ``commits`` (oldest first) onto ``onto`` without touching any checkout."""
    head = onto
    for commit in commits:
        parents = repository.output("rev-list", "--parents", "-n", "1", commit).split()[1:]
        if len(parents) != 1:
            raise Failure(f"{commit[:10]} is a merge; rebase it by hand")
        merged = repository.git(
            "merge-tree", "--write-tree", "--name-only", "--no-messages",
            f"--merge-base={parents[0]}", head, commit, check=False,
        )
        if merged.returncode:
            lines = merged.stdout.strip().splitlines()
            subject = repository.output("log", "-1", "--format=%s", commit)
            raise Failure(
                f"{commit[:10]} {subject!r} does not apply onto {onto[:10]}; nothing was "
                "changed. Conflicting files: " + ", ".join(lines[1:] or ["(unknown)"])
            )
        tree = merged.stdout.split()[0]
        fields = repository.output(
            "log", "-1", "--format=%an%x00%ae%x00%ad", "--date=raw", commit
        ).split("\0")
        env = dict(
            os.environ,
            GIT_AUTHOR_NAME=fields[0],
            GIT_AUTHOR_EMAIL=fields[1],
            GIT_AUTHOR_DATE=fields[2],
        )
        message = repository.git("log", "-1", "--format=%B", commit).stdout
        head = repository.git(
            "commit-tree", tree, "-p", head, env=env, input=message
        ).stdout.strip()
    return head


def rebase(repository: Repository, base: str) -> Rebased:
    """Rebase the checked-out topic branch onto origin's ``base`` and push it."""
    ref = repository.branch()
    name = ref.removeprefix("refs/heads/")
    if name == base:
        raise Failure(f"{name} is the base branch; check out a topic branch to rebase")
    with locked(repository):
        update(repository)
        pushed_before = fetch(repository, name)
        onto = fetch(repository, base)
        if onto is None:
            raise Failure(f"origin has no {base}")
        current = repository.output("rev-parse", "HEAD")
        commits = repository.output(
            "rev-list", "--reverse", "--topo-order", f"{onto}..{current}"
        ).split()
        moved = None
        head = current
        if not ancestor(repository, onto, current):
            head = replay(repository, onto, commits)
            moved = advance(repository, ref, current, head)
        if pushed_before == head:
            return Rebased(head, 0, False, moved)
        lease = f"--force-with-lease=refs/heads/{name}:{pushed_before or ''}"
        pushed = repository.git(
            "push", "-q", lease, "origin", f"{head}:refs/heads/{name}", check=False
        )
        if pushed.returncode:
            raise Failure(
                f"rebased {name} locally to {head[:10]} but the push failed (origin "
                "moved or refused); run the sync again:\n" + pushed.stderr.strip()
            )
        repository.git("update-ref", f"refs/remotes/origin/{name}", head)
        return Rebased(head, len(commits) if moved else 0, True, moved)


def advance(repository: Repository, ref: str, current: str, remote: str) -> Result:
    # Compare files on disk with the current commit, not with stale index entries.
    repository.reset_index()
    changed = repository.names("diff", "--no-renames", current, remote)
    edited = set(repository.names("diff", "--no-renames", current)).intersection(
        changed
    )
    # Untracked files where origin adds a path.
    present = {
        path
        for path in changed
        if entry(repository, current, path) is None
        and Path(repository.root, path).exists()
    }
    conflicts = load_conflicts(repository)
    result = Result(remote)
    clean_present: list[str] = []
    clean_deleted: list[str] = []

    for path in changed:
        old = entry(repository, current, path)
        new = entry(repository, remote, path)
        if GITLINK in (old or ("",))[0] or GITLINK in (new or ("",))[0]:
            follow_pin(repository, path, old, new)
            continue
        if path not in edited and path not in present:
            (clean_present if new else clean_deleted).append(path)
            continue
        if merge_path(repository, path, old, new):
            result.merged.append(path)
        else:
            result.conflicts.append(path)
            conflicts[path] = {
                "base": old[1] if old else "",
                "theirs": new[1] if new else "",
                "commit": remote,
            }

    if clean_present:
        repository.git(
            "checkout",
            remote,
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
            input="\0".join(clean_present) + "\0",
        )
    for path in clean_deleted:
        Path(repository.root, path).unlink(missing_ok=True)
    if repository.git("update-ref", ref, remote, current, check=False).returncode:
        raise Failure(
            f"{ref} moved during sync; files were updated to {remote[:10]} but the "
            "branch was not. Run the sync again."
        )
    repository.reset_index()
    # A recorded conflict whose file now matches the branch needs no merge.
    for path in list(conflicts):
        head = entry(repository, remote, path)
        file = Path(repository.root, path)
        if head and file.is_file() and repository.file_object(path) == head[1]:
            del conflicts[path]
    save_conflicts(repository, conflicts)
    return result


def follow_pin(
    repository: Repository,
    path: str,
    old: tuple[str, str] | None,
    new: tuple[str, str] | None,
) -> None:
    """Move a pinned submodule checkout to origin's pin.

    A submodule on a branch syncs itself. A detached checkout still at the old
    pin, with no edits, follows the new pin; anything else is someone's work and
    stays as it is.
    """
    checkout = Repository(Path(repository.root, path))
    if old is None or new is None or not Path(checkout.root, ".git").exists():
        return
    if checkout.git("symbolic-ref", "-q", "HEAD", check=False).returncode == 0:
        return
    if checkout.output("rev-parse", "HEAD") != old[1] or checkout.output(
        "status", "--porcelain"
    ):
        return
    if checkout.git("cat-file", "-e", f"{new[1]}^{{commit}}", check=False).returncode:
        checkout.git("fetch", "-q", "origin", new[1])
    checkout.git("checkout", "-q", "--detach", new[1])


def follows_superproject(repository: Repository) -> bool:
    """A detached submodule checkout moves with its superproject's pin, not a branch."""
    if repository.git("symbolic-ref", "-q", "HEAD", check=False).returncode == 0:
        return False
    return bool(repository.output("rev-parse", "--show-superproject-working-tree"))


def merge_path(
    repository: Repository,
    path: str,
    old: tuple[str, str] | None,
    new: tuple[str, str] | None,
) -> bool:
    """Merge one locally changed file onto origin's version; False leaves it untouched."""
    file = Path(repository.root, path)
    if old is None or new is None or not file.is_file():
        # Added or deleted on one side: someone has to decide, unless both agree.
        return (
            new is not None
            and file.is_file()
            and repository.file_object(path) == new[1]
        )
    base, theirs = blob(repository, old[1]), blob(repository, new[1])
    ours = file.read_bytes()
    if not is_text(repository, path, base, theirs, ours):
        return repository.file_object(path) == new[1]
    merged = merge(base, ours, theirs)
    if merged is None:
        return False
    if file.read_bytes() != ours:
        return False  # Edited again while merging; leave it for the next sync.
    file.write_bytes(merged)
    os.chmod(file, 0o755 if new[0] == "100755" else 0o644)
    return True


def install_service(repository: Repository, interval: int) -> Path:
    digest = hash_blob(repository, str(repository.root).encode())[:12]
    label = f"dev.fujimod.git-sync.{repository.root.name}.{digest}"
    plist = Path.home() / "Library/LaunchAgents" / f"{label}.plist"
    logs = Path.home() / "Library/Logs" / f"{label}.log"
    plist.parent.mkdir(parents=True, exist_ok=True)
    plist.write_bytes(
        plistlib.dumps({
            "Label": label,
            "ProgramArguments": [
                sys.executable,
                str(Path(__file__).resolve()),
                "--repo",
                str(repository.root),
                "--quiet",
            ],
            "StartInterval": interval,
            "RunAtLoad": True,
            "StandardOutPath": str(logs),
            "StandardErrorPath": str(logs),
            "EnvironmentVariables": {"PATH": os.environ.get("PATH", "/usr/bin:/bin")},
        })
    )
    domain = f"gui/{os.getuid()}"
    subprocess.run(
        ["launchctl", "bootout", domain, str(plist)], capture_output=True, check=False
    )
    subprocess.run(["launchctl", "bootstrap", domain, str(plist)], check=True)
    return plist


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--repo",
        type=Path,
        action="append",
        help="checkout to sync (repeatable, in order; default: the current one)",
    )
    parser.add_argument("--resolved", nargs="+", metavar="PATH")
    parser.add_argument(
        "--onto", metavar="BRANCH", help="rebase the topic branch onto origin's BRANCH"
    )
    parser.add_argument("--install-service", action="store_true")
    parser.add_argument("--interval", type=int, default=30)
    parser.add_argument(
        "--quiet", action="store_true", help="print only when something changed"
    )
    args = parser.parse_args()
    if (args.resolved or args.install_service) and len(args.repo or []) > 1:
        parser.error("--resolved and --install-service take one --repo")
    for repo in args.repo or [Path.cwd()]:
        run(args, repo)


def run(args: argparse.Namespace, repo: Path) -> None:
    try:
        repository = Repository(Repository.toplevel(repo))
        repository.clear_stale_index_lock()
        if args.resolved:
            resolve(repository, args.resolved)
            return
        if args.install_service:
            print(f"installed {install_service(repository, args.interval)}")
            return
        if follows_superproject(repository):
            if not args.quiet:
                print("sync: detached submodule; the superproject's sync moves its pin")
            return
        if args.onto:
            rebased = rebase(repository, args.onto)
            report(rebased.moved)
            if rebased.replayed:
                print(f"sync: replayed {rebased.replayed} commits onto origin/{args.onto}")
            print(
                f"sync: at {rebased.head[:10]}"
                + ("; pushed" if rebased.pushed else "; origin already has it")
            )
            return
        ref = repository.branch()
        if args.quiet and remote_head(repository, ref) == repository.output(
            "rev-parse", "HEAD"
        ):
            return
        result = sync(repository)
    except (Failure, subprocess.CalledProcessError) as error:
        print(f"sync: {error}", file=sys.stderr)
        sys.exit(1)
    if result.merged or result.conflicts or not args.quiet:
        print(f"sync: at {result.head[:10]}")
    report(result)


def report(result: Result | None) -> None:
    if result is None:
        return
    for path in result.merged:
        print(f"  merged local edits onto origin: {path}")
    for path in result.conflicts:
        print(f"  CONFLICT, local content kept, merge by hand then --resolved: {path}")


if __name__ == "__main__":
    main()
