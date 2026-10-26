"""Land an approved worktree's branch on main.

    mise run land

Run inside the approved worktree, on its branch, with every change committed.
The branch is rebased onto ``origin/main`` and pushed to ``origin`` ``main`` as a
fast-forward, never forced. The main checkout then syncs under its lock, as
``mise run commit`` does: its ``main`` moves to the landed commit and origin's
changes are merged into the edits there. If origin moves before the push lands,
the rebase repeats. A rebase conflict is aborted and reported; rebase the branch
by hand and land again.
"""

import argparse
import subprocess
import sys
from pathlib import Path

from repository import Failure, Repository
from sync import locked, update

ATTEMPTS = 5


def main_checkout(repository: Repository) -> Path:
    """The repository's main worktree, which Git always lists first."""
    listing = repository.output("worktree", "list", "--porcelain")
    return Path(listing.splitlines()[0].removeprefix("worktree "))


def land(worktree: Repository) -> str:
    git_dir = worktree.output("rev-parse", "--path-format=absolute", "--git-dir")
    common = worktree.output("rev-parse", "--path-format=absolute", "--git-common-dir")
    if git_dir == common:
        raise Failure(
            "land runs in an approved worktree; the main checkout commits with "
            "`mise run commit`"
        )
    name = worktree.branch().removeprefix("refs/heads/")
    if name == "main":
        raise Failure("check out the worktree's own branch, not main")
    if worktree.output("status", "--porcelain", "--untracked-files=no"):
        raise Failure("commit or discard the worktree's tracked changes before landing")
    checkout = Repository(main_checkout(worktree))
    for _ in range(ATTEMPTS):
        worktree.git("fetch", "-q", "origin", "main")
        target = worktree.output("rev-parse", "FETCH_HEAD")
        if worktree.git("rebase", "-q", target, check=False).returncode:
            worktree.git("rebase", "--abort", check=False)
            raise Failure(
                f"{name} conflicts with origin/main; rebase it by hand, then land again"
            )
        head = worktree.output("rev-parse", "HEAD")
        if head == target:
            raise Failure(f"{name} has no commits beyond origin/main")
        pushed = worktree.git(
            "push", "-q", "origin", f"{head}:refs/heads/main", check=False
        )
        if pushed.returncode == 0:
            with locked(checkout):
                update(checkout)
            return head
    raise Failure(f"origin main kept moving; gave up after {ATTEMPTS} attempts")


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--repo", type=Path, default=Path.cwd(), help="worktree to land"
    )
    args = parser.parse_args()
    try:
        worktree = Repository(Repository.toplevel(args.repo))
        head = land(worktree)
    except (Failure, subprocess.CalledProcessError) as error:
        print(f"land: {error}", file=sys.stderr)
        sys.exit(1)
    print(worktree.git("log", "--oneline", "-1", head).stdout, end="")


if __name__ == "__main__":
    main()
