import subprocess
from pathlib import Path

import pytest
from land import land
from repository import Failure, Repository


def run(*arguments: str, cwd: Path) -> str:
    return subprocess.run(
        arguments, cwd=cwd, check=True, capture_output=True, text=True
    ).stdout


def configure(path: Path) -> None:
    for name in ("user.name", "user.email"):
        run("git", "config", name, "Test", cwd=path)


@pytest.fixture
def repos(tmp_path: Path) -> tuple[Path, Path, Path]:
    remote = tmp_path / "remote.git"
    run("git", "init", "-q", "--bare", "-b", "main", str(remote), cwd=tmp_path)
    checkout = tmp_path / "checkout"
    run("git", "clone", "-q", str(remote), str(checkout), cwd=tmp_path)
    configure(checkout)
    (checkout / "a.txt").write_text("one\n")
    (checkout / "b.txt").write_text("b\n")
    run("git", "add", ".", cwd=checkout)
    run("git", "commit", "-qm", "base", cwd=checkout)
    run("git", "push", "-q", "origin", "main", cwd=checkout)
    worktree = tmp_path / "worktree"
    run(
        "git", "worktree", "add", "-q", "-b", "worktree/test", str(worktree),
        cwd=checkout,
    )
    return remote, checkout, worktree


def commit(path: Path, name: str, text: str) -> None:
    (path / name).write_text(text)
    run("git", "commit", "-qam", f"Edit {name}", cwd=path)


def test_land_rebases_onto_origin_pushes_and_syncs_the_main_checkout(
    repos: tuple[Path, Path, Path], tmp_path: Path
) -> None:
    remote, checkout, worktree = repos
    commit(worktree, "a.txt", "one\nworktree\n")
    # Someone else lands on origin meanwhile; the main checkout keeps an edit.
    other = tmp_path / "other"
    run("git", "clone", "-q", str(remote), str(other), cwd=tmp_path)
    configure(other)
    commit(other, "b.txt", "b\nother\n")
    run("git", "push", "-q", "origin", "main", cwd=other)
    (checkout / "c.txt").write_text("untracked work\n")
    head = land(Repository(worktree))
    assert run("git", "rev-parse", "main", cwd=remote).strip() == head
    assert run("git", "rev-parse", "HEAD", cwd=checkout).strip() == head
    assert (checkout / "a.txt").read_text() == "one\nworktree\n"
    assert (checkout / "b.txt").read_text() == "b\nother\n"
    assert (checkout / "c.txt").read_text() == "untracked work\n"


def test_land_refuses_uncommitted_work_and_the_main_checkout(
    repos: tuple[Path, Path, Path],
) -> None:
    remote, checkout, worktree = repos
    before = run("git", "rev-parse", "main", cwd=remote)
    (worktree / "a.txt").write_text("uncommitted\n")
    with pytest.raises(Failure, match="tracked changes"):
        land(Repository(worktree))
    with pytest.raises(Failure, match="mise run commit"):
        land(Repository(checkout))
    assert run("git", "rev-parse", "main", cwd=remote) == before


def test_land_aborts_a_conflicting_rebase(
    repos: tuple[Path, Path, Path], tmp_path: Path
) -> None:
    remote, _, worktree = repos
    commit(worktree, "a.txt", "worktree\n")
    other = tmp_path / "other"
    run("git", "clone", "-q", str(remote), str(other), cwd=tmp_path)
    configure(other)
    commit(other, "a.txt", "other\n")
    run("git", "push", "-q", "origin", "main", cwd=other)
    before = run("git", "rev-parse", "main", cwd=remote)
    with pytest.raises(Failure, match="conflicts with origin/main"):
        land(Repository(worktree))
    assert run("git", "rev-parse", "main", cwd=remote) == before
    assert (worktree / "a.txt").read_text() == "worktree\n"
    assert run("git", "status", "--porcelain", cwd=worktree) == ""
