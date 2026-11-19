"""Rebase the checked-out topic branch onto its pull request's base and push it.

    mise run rebase [-- --base BRANCH]

The base is the branch the topic's open pull request targets: ``main``, or
another topic when the pull request is stacked on it; ``main`` when the branch
has no pull request yet. ``--base`` names it instead of asking GitHub. The
rebase is ``sync.py --onto BASE``: each commit not on ``origin/BASE`` is
replayed onto it (commits it already has, such as a landed stacked base's, are
dropped), uncommitted edits are carried, and the branch is pushed with
``--force-with-lease``. A detached submodule checkout is skipped.

Plain ``sync.py`` never rebases; it only catches up with the branch's own
``origin`` copy, which is safe to run in the background.
"""

import argparse
import json
import subprocess
import sys
from pathlib import Path

from repository import Failure, Repository
from sync import follows_superproject, rebase_and_report

DEFAULT_BASE = "main"


def pull_request_base(repository: Repository, branch: str) -> str:
    """The base branch of ``branch``'s open pull request, or main without one."""
    try:
        listed = subprocess.run(
            ["gh", "pr", "list", "--head", branch, "--state", "open",
             "--json", "baseRefName,number"],
            cwd=repository.root, capture_output=True, text=True, check=False,
        )
    except FileNotFoundError:
        raise Failure("gh is not installed; pass --base BRANCH") from None
    if listed.returncode:
        raise Failure(
            "could not ask GitHub for the pull request's base; pass --base BRANCH:\n"
            + listed.stderr.strip()
        )
    pulls = json.loads(listed.stdout or "[]")
    if len(pulls) > 1:
        numbers = ", ".join(f"#{pull['number']}" for pull in pulls)
        raise Failure(f"{branch} has several open pull requests ({numbers}); pass --base BRANCH")
    return pulls[0]["baseRefName"] if pulls else DEFAULT_BASE


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--repo", type=Path, action="append",
        help="checkout to rebase (repeatable, in order; default: the current one)",
    )
    parser.add_argument("--base", help="rebase onto this branch instead of the pull request's base")
    args = parser.parse_args()
    for repo in args.repo or [Path.cwd()]:
        try:
            repository = Repository(Repository.toplevel(repo))
            if follows_superproject(repository):
                continue
            branch = repository.branch().removeprefix("refs/heads/")
            base = args.base or pull_request_base(repository, branch)
            print(f"rebase: {repository.root.name} {branch} onto origin/{base}")
            rebase_and_report(repository, base)
        except (Failure, subprocess.CalledProcessError) as error:
            print(f"rebase: {error}", file=sys.stderr)
            sys.exit(1)


if __name__ == "__main__":
    main()
