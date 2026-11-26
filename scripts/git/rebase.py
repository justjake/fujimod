"""Rebase the checked-out topic branch onto its pull request's base and push it.

    mise run rebase [-- --base BRANCH]

The base is the branch the topic's open pull request targets: ``main``, or
another topic when the pull request is stacked on it; ``main`` when the branch
has no pull request yet. ``--base`` names it instead of asking GitHub. The
rebase is ``sync.py --onto BASE``: each commit not on ``origin/BASE`` is
replayed onto it (commits it already has, such as a landed stacked base's, are
dropped), uncommitted edits are carried, and the branch is pushed with
``--force-with-lease``. Submodule checkouts on branches are rebased first, each
onto its own pull request's base (or ``--base``), so the superproject's pin
commits map to the rebased submodule commits. A detached submodule checkout is
skipped.

Plain ``sync.py`` never rebases; it only catches up with the branch's own
``origin`` copy, which is safe to run in the background.
"""

import argparse
import json
import subprocess
import sys
from pathlib import Path
from typing import cast

from repository import Failure, Repository
from sync import follows_superproject, rebase_and_report

DEFAULT_BASE = "main"


def pull_request_base(repository: Repository, branch: str) -> str:
    """The base branch of ``branch``'s open pull request, or main without one."""
    try:
        listed = subprocess.run(
            [
                "gh",
                "pr",
                "list",
                "--head",
                branch,
                "--state",
                "open",
                "--json",
                "baseRefName,number",
            ],
            cwd=repository.root,
            capture_output=True,
            text=True,
            check=False,
        )
    except FileNotFoundError:
        raise Failure("gh is not installed; pass --base BRANCH") from None
    if listed.returncode:
        raise Failure(
            "could not ask GitHub for the pull request's base; pass --base BRANCH:\n"
            + listed.stderr.strip()
        )
    pulls = json.loads(listed.stdout or "[]")
    if not isinstance(pulls, list):
        raise Failure("GitHub returned an unexpected pull request list")
    if len(pulls) > 1:
        numbers = ", ".join(
            f"#{cast(dict[str, object], pull)['number']}" for pull in pulls
        )
        raise Failure(
            f"{branch} has several open pull requests ({numbers}); pass --base BRANCH"
        )
    return (
        cast(str, cast(dict[str, object], pulls[0])["baseRefName"])
        if pulls
        else DEFAULT_BASE
    )


def branch_submodules(repository: Repository) -> list[tuple[str, Repository]]:
    """Initialized submodule checkouts whose HEAD is a branch, in path order."""
    listed = repository.git(
        "config",
        "-f",
        ".gitmodules",
        "--get-regexp",
        r"^submodule\..*\.path$",
        check=False,
    )
    if listed.returncode:
        return []
    found = []
    for line in listed.stdout.splitlines():
        _, _, path = line.partition(" ")
        root = Path(repository.root, path.strip())
        if not path.strip() or not Path(root, ".git").exists():
            continue
        checkout = Repository(root)
        if checkout.git("symbolic-ref", "-q", "HEAD", check=False).returncode:
            continue
        found.append((path.strip(), checkout))
    return found


def rebase_tree(
    repository: Repository,
    base: str | None,
    seen: set[Path],
    pin_maps: dict[Path, dict[str, str]],
) -> None:
    """Rebase branch submodules first, then ``repository`` itself."""
    root = repository.root.resolve()
    if root in seen:
        return
    seen.add(root)
    submodules = branch_submodules(repository)
    maps: dict[str, dict[str, str]] = {}
    for path, submodule in submodules:
        rebase_tree(submodule, base, seen, pin_maps)
        if pin_maps.get(submodule.root.resolve()):
            maps[path] = pin_maps[submodule.root.resolve()]
    if follows_superproject(repository):
        return
    branch = repository.branch().removeprefix("refs/heads/")
    target = base or pull_request_base(repository, branch)
    print(f"rebase: {repository.root.name} {branch} onto origin/{target}")
    rebased = rebase_and_report(repository, target, maps or None)
    if rebased.commits:
        pin_maps[root] = rebased.commits


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--repo",
        type=Path,
        action="append",
        help="checkout to rebase (repeatable, in order; default: the current one)",
    )
    parser.add_argument(
        "--base", help="rebase onto this branch instead of the pull request's base"
    )
    args = parser.parse_args()
    seen: set[Path] = set()
    pin_maps: dict[Path, dict[str, str]] = {}
    for repo in args.repo or [Path.cwd()]:
        try:
            repository = Repository(Repository.toplevel(repo))
            rebase_tree(repository, args.base, seen, pin_maps)
        except (Failure, subprocess.CalledProcessError) as error:
            print(f"rebase: {error}", file=sys.stderr)
            sys.exit(1)


if __name__ == "__main__":
    main()
