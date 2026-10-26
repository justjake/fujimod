import os
import subprocess
from pathlib import Path

import pytest
from install_hooks import install


def git(*arguments: str, cwd: Path, env: dict[str, str] | None = None) -> str:
    return subprocess.run(
        ["git", *arguments],
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        check=True,
    ).stdout


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    path = tmp_path / "repo"
    git("init", "-q", "-b", "main", str(path), cwd=tmp_path)
    for name in ("user.name", "user.email"):
        git("config", name, "Test", cwd=path)
    (path / "a.txt").write_text("a\n")
    git("add", ".", cwd=path)
    git("commit", "-qm", "base", cwd=path)
    install(path)
    return path


def worktree_count(repo: Path) -> int:
    return git("worktree", "list", "--porcelain", cwd=repo).count("worktree ")


def test_unapproved_worktree_is_removed(repo: Path, tmp_path: Path) -> None:
    env = {k: v for k, v in os.environ.items() if k != "FUJIMOD_WORKTREE_APPROVED"}
    result = subprocess.run(
        ["git", "worktree", "add", "-q", "--detach", str(tmp_path / "extra")],
        cwd=repo,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode != 0
    assert "New worktrees are refused" in result.stderr
    assert worktree_count(repo) == 1
    assert not (tmp_path / "extra").exists()


def test_approved_worktree_is_kept_and_logged(repo: Path, tmp_path: Path) -> None:
    env = dict(os.environ, FUJIMOD_WORKTREE_APPROVED="owner OK: bisect")
    git("worktree", "add", "-q", "--detach", str(tmp_path / "extra"), cwd=repo, env=env)
    assert worktree_count(repo) == 2
    log = (repo / ".git" / "fujimod-worktrees.log").read_text()
    assert str((tmp_path / "extra").resolve()) in log
    assert log.rstrip().endswith("owner OK: bisect")


def test_ordinary_checkouts_are_unaffected(repo: Path) -> None:
    git("checkout", "-q", "-b", "other", cwd=repo)
    git("checkout", "-q", "main", cwd=repo)
    assert worktree_count(repo) == 1
