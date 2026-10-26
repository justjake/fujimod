import os
import subprocess
import threading
import time
from pathlib import Path

import pytest
from commit import Failure, Repository


def run(*arguments: str, cwd: Path) -> str:
    return subprocess.run(
        arguments, cwd=cwd, check=True, capture_output=True, text=True
    ).stdout


def clone(remote: Path, path: Path) -> Path:
    run("git", "clone", "-q", str(remote), str(path), cwd=path.parent)
    for name in ("user.name", "user.email"):
        run("git", "config", name, "Test", cwd=path)
    return path


@pytest.fixture
def repos(tmp_path: Path) -> tuple[Path, Path]:
    remote = tmp_path / "remote.git"
    run("git", "init", "-q", "--bare", "-b", "main", str(remote), cwd=tmp_path)
    checkout = clone(remote, tmp_path / "checkout")
    (checkout / "a.txt").write_text("one\ntwo\nthree\n")
    (checkout / "b.txt").write_text("b\n")
    run("git", "add", ".", cwd=checkout)
    run("git", "commit", "-qm", "base", cwd=checkout)
    run("git", "push", "-q", "origin", "main", cwd=checkout)
    return remote, checkout


def test_commit_takes_only_named_paths_and_pushes(repos: tuple[Path, Path]) -> None:
    remote, checkout = repos
    (checkout / "a.txt").write_text("one\ntwo\nthree\nfour\n")
    (checkout / "b.txt").write_text("someone else\n")
    run("git", "add", "b.txt", cwd=checkout)
    result = Repository(checkout).commit(["a.txt"], None, "Add four\n")
    assert run("git", "rev-parse", "main", cwd=remote).strip() == result
    files = run("git", "show", "--name-only", "--format=", result, cwd=checkout).split()
    assert files == ["a.txt"]
    # The shared index mirrors HEAD; the other edit stays on disk, unstaged.
    assert run("git", "status", "--porcelain", cwd=checkout).splitlines() == [
        " M b.txt"
    ]
    assert (checkout / "b.txt").read_text() == "someone else\n"


def test_patch_commits_chosen_hunks_and_keeps_the_rest(
    repos: tuple[Path, Path], tmp_path: Path
) -> None:
    _, checkout = repos
    (checkout / "a.txt").write_text("ONE\ntwo\nthree\n")
    patch = tmp_path / "mine.diff"
    patch.write_text(run("git", "diff", cwd=checkout))
    (checkout / "a.txt").write_text("ONE\ntwo\nthree\nother work\n")
    result = Repository(checkout).commit([], patch, "Capitalize\n")
    assert run("git", "show", f"{result}:a.txt", cwd=checkout) == "ONE\ntwo\nthree\n"
    assert (checkout / "a.txt").read_text() == "ONE\ntwo\nthree\nother work\n"
    assert run("git", "diff", cwd=checkout).count("+other work") == 1


def test_paths_that_name_patched_files_are_refused(
    repos: tuple[Path, Path], tmp_path: Path
) -> None:
    remote, checkout = repos
    before = run("git", "rev-parse", "main", cwd=remote)
    (checkout / "a.txt").write_text("ONE\ntwo\nthree\n")
    patch = tmp_path / "mine.diff"
    patch.write_text(run("git", "diff", cwd=checkout))
    (checkout / "a.txt").write_text("ONE\ntwo\nthree\nother work\n")
    for path in ["a.txt", "."]:
        with pytest.raises(Failure, match="a.txt"):
            Repository(checkout).commit([path], patch, "Capitalize\n")
    assert run("git", "rev-parse", "main", cwd=remote) == before


def test_commit_rebuilds_on_a_newer_origin(
    repos: tuple[Path, Path], tmp_path: Path
) -> None:
    remote, checkout = repos
    other = clone(remote, tmp_path / "other")
    (other / "c.txt").write_text("c\n")
    run("git", "add", "c.txt", cwd=other)
    run("git", "commit", "-qm", "other", cwd=other)
    run("git", "push", "-q", "origin", "main", cwd=other)
    (checkout / "b.txt").write_text("mine\n")
    result = Repository(checkout).commit(["b.txt"], None, "Mine\n")
    parent = run("git", "rev-parse", f"{result}^", cwd=checkout).strip()
    assert parent == run("git", "rev-parse", "main", cwd=other).strip()
    assert (checkout / "c.txt").read_text() == "c\n"


def test_nothing_to_commit_fails(repos: tuple[Path, Path]) -> None:
    _, checkout = repos
    with pytest.raises(Failure, match="nothing to commit"):
        Repository(checkout).commit(["a.txt"], None, "Nothing\n")


def test_detached_head_is_refused(repos: tuple[Path, Path]) -> None:
    _, checkout = repos
    run("git", "checkout", "-q", "--detach", cwd=checkout)
    (checkout / "a.txt").write_text("detached\n")
    with pytest.raises(Failure, match="detached"):
        Repository(checkout).commit(["a.txt"], None, "Detached\n")


def test_commit_waits_for_a_busy_index(repos: tuple[Path, Path]) -> None:
    remote, checkout = repos
    lock = checkout / ".git" / "index.lock"
    lock.write_text("")
    threading.Timer(1.0, lock.unlink).start()
    (checkout / "a.txt").write_text("waited\n")
    result = Repository(checkout).commit(["a.txt"], None, "Wait\n")
    assert run("git", "rev-parse", "main", cwd=remote).strip() == result
    assert run("git", "status", "--porcelain", cwd=checkout) == ""


def test_a_stale_index_lock_is_removed(repos: tuple[Path, Path]) -> None:
    remote, checkout = repos
    lock = checkout / ".git" / "index.lock"
    lock.write_text("")
    old = time.time() - 120
    os.utime(lock, (old, old))
    (checkout / "a.txt").write_text("after a crash\n")
    result = Repository(checkout).commit(["a.txt"], None, "Recover\n")
    assert run("git", "rev-parse", "main", cwd=remote).strip() == result
    assert not lock.exists()


def test_ignored_files_need_the_flag(repos: tuple[Path, Path]) -> None:
    _, checkout = repos
    (checkout / ".gitignore").write_text("runs/*/\n")
    Repository(checkout).commit([".gitignore"], None, "Ignore runs\n")
    (checkout / "runs/one").mkdir(parents=True)
    (checkout / "runs/one/input.json").write_text("{}\n")
    (checkout / "runs/one/frame.bin").write_text("data\n")
    with pytest.raises(Failure, match="ignored"):
        Repository(checkout).commit(["runs/one/input.json"], None, "Input\n")
    result = Repository(checkout).commit(
        ["runs/one/input.json"], None, "Input\n", ignored=True
    )
    files = run("git", "show", "--name-only", "--format=", result, cwd=checkout)
    assert files.split() == ["runs/one/input.json"]


def test_logs_and_bulk_additions_need_allow_bulk(repos: tuple[Path, Path]) -> None:
    _, checkout = repos
    (checkout / "run.log").write_text("output\n")
    with pytest.raises(Failure, match="logs or movies"):
        Repository(checkout).commit(["run.log"], None, "Log\n")
    (checkout / "out").mkdir()
    for number in range(500):
        (checkout / "out" / f"{number}.txt").write_text(f"{number}\n")
    with pytest.raises(Failure, match="500 files"):
        Repository(checkout).commit(["out"], None, "Output\n")
    Repository(checkout).commit(["out"], None, "Import\n", allow_bulk=True)


def test_untrack_keeps_the_file_on_disk(repos: tuple[Path, Path]) -> None:
    _, checkout = repos
    (checkout / ".gitignore").write_text("b.txt\n")
    result = Repository(checkout).commit(
        [".gitignore"], None, "Untrack b\n", untrack=["b.txt"]
    )
    files = run("git", "ls-tree", "--name-only", result, cwd=checkout).split()
    assert "b.txt" not in files
    assert (checkout / "b.txt").read_text() == "b\n"
    assert run("git", "status", "--porcelain", cwd=checkout) == ""
