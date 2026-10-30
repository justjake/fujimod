import subprocess
from pathlib import Path

import pytest
from commit import Failure, Repository
from sync import load_conflicts, resolve, sync


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
def repos(tmp_path: Path) -> tuple[Path, Path, Path]:
    """A remote, the shared checkout, and another machine that pushes to it."""
    remote = tmp_path / "remote.git"
    run("git", "init", "-q", "--bare", "-b", "main", str(remote), cwd=tmp_path)
    other = clone(remote, tmp_path / "other")
    (other / "a.txt").write_text("one\ntwo\nthree\nfour\nfive\n")
    (other / "b.txt").write_text("b\n")
    (other / "gone.txt").write_text("gone\n")
    run("git", "add", ".", cwd=other)
    run("git", "commit", "-qm", "base", cwd=other)
    run("git", "push", "-q", "origin", "main", cwd=other)
    return remote, clone(remote, tmp_path / "shared"), other


def push(other: Path, files: dict[str, str | None]) -> str:
    for name, content in files.items():
        if content is None:
            run("git", "rm", "-q", name, cwd=other)
        else:
            (other / name).write_text(content)
            run("git", "add", name, cwd=other)
    run("git", "commit", "-qm", "upstream", cwd=other)
    run("git", "push", "-q", "origin", "main", cwd=other)
    return run("git", "rev-parse", "HEAD", cwd=other).strip()


def test_sync_merges_upstream_changes_into_a_local_edit(
    repos: tuple[Path, Path, Path],
) -> None:
    _, shared, other = repos
    (shared / "a.txt").write_text("ONE\ntwo\nthree\nfour\nfive\n")
    head = push(
        other,
        {"a.txt": "one\ntwo\nthree\nfour\nFIVE\n", "b.txt": "B\n", "gone.txt": None},
    )
    result = sync(Repository(shared))
    assert run("git", "rev-parse", "HEAD", cwd=shared).strip() == head
    assert result.merged == ["a.txt"] and result.conflicts == []
    assert (shared / "a.txt").read_text() == "ONE\ntwo\nthree\nfour\nFIVE\n"
    assert (shared / "b.txt").read_text() == "B\n"
    assert not (shared / "gone.txt").exists()
    # Only the local edit remains, relative to the new HEAD.
    assert run("git", "status", "--porcelain", cwd=shared).splitlines() == [" M a.txt"]
    changes = [
        line
        for line in run("git", "diff", "-U0", cwd=shared).splitlines()[4:]
        if line[0] in "+-"
    ]
    assert changes == ["-one", "+ONE"]


def test_staged_edits_become_unstaged_and_are_merged(
    repos: tuple[Path, Path, Path],
) -> None:
    _, shared, other = repos
    (shared / "a.txt").write_text("ONE\ntwo\nthree\nfour\nfive\n")
    run("git", "add", "a.txt", cwd=shared)
    push(other, {"a.txt": "one\ntwo\nthree\nfour\nFIVE\n"})
    sync(Repository(shared))
    assert (shared / "a.txt").read_text() == "ONE\ntwo\nthree\nfour\nFIVE\n"
    assert run("git", "status", "--porcelain", cwd=shared).splitlines() == [" M a.txt"]


def test_stale_index_entries_never_revert_committed_work(
    repos: tuple[Path, Path, Path],
) -> None:
    _, shared, other = repos
    repository = Repository(shared)
    (shared / "b.txt").write_text("committed\n")
    repository.commit(["b.txt"], None, "Commit b\n")
    # An index left at the old content, as when another process held index.lock.
    old = run("git", "rev-parse", "HEAD^:b.txt", cwd=shared).strip()
    run("git", "update-index", "--cacheinfo", f"100644,{old},b.txt", cwd=shared)
    run("git", "pull", "-q", cwd=other)
    head = push(other, {"b.txt": "committed\nmore\n"})
    sync(repository)
    assert run("git", "rev-parse", "HEAD", cwd=shared).strip() == head
    assert (shared / "b.txt").read_text() == "committed\nmore\n"
    assert run("git", "status", "--porcelain", cwd=shared) == ""


def test_overlapping_edit_is_kept_recorded_and_refused_until_resolved(
    repos: tuple[Path, Path, Path],
) -> None:
    _, shared, other = repos
    (shared / "a.txt").write_text("one\nTWO local\nthree\nfour\nfive\n")
    head = push(
        other, {"a.txt": "one\nTWO upstream\nthree\nfour\nfive\n", "b.txt": "B\n"}
    )
    repository = Repository(shared)
    result = sync(repository)
    assert result.conflicts == ["a.txt"]
    assert run("git", "rev-parse", "HEAD", cwd=shared).strip() == head
    assert (shared / "a.txt").read_text() == "one\nTWO local\nthree\nfour\nfive\n"
    assert (shared / "b.txt").read_text() == "B\n"
    with pytest.raises(Failure, match="merge them by hand"):
        repository.commit(["a.txt"], None, "Edit\n")
    (shared / "a.txt").write_text("one\nTWO both\nthree\nfour\nfive\n")
    resolve(repository, ["a.txt"])
    assert load_conflicts(repository) == {}
    repository.commit(["a.txt"], None, "Edit\n")


def test_commit_succeeds_when_origin_changed_an_edited_file(
    repos: tuple[Path, Path, Path],
) -> None:
    remote, shared, other = repos
    (shared / "a.txt").write_text("ONE\ntwo\nthree\nfour\nfive\n")
    (shared / "b.txt").write_text("mine\n")
    push(other, {"a.txt": "one\ntwo\nthree\nfour\nFIVE\n"})
    result = Repository(shared).commit(["b.txt"], None, "Mine\n")
    assert run("git", "rev-parse", "main", cwd=remote).strip() == result
    assert run("git", "rev-parse", "HEAD", cwd=shared).strip() == result
    assert (shared / "a.txt").read_text() == "ONE\ntwo\nthree\nfour\nFIVE\n"


def test_untracked_file_where_origin_adds_one_is_a_conflict(
    repos: tuple[Path, Path, Path],
) -> None:
    _, shared, other = repos
    (shared / "new.txt").write_text("local\n")
    push(other, {"new.txt": "upstream\n"})
    result = sync(Repository(shared))
    assert result.conflicts == ["new.txt"]
    assert (shared / "new.txt").read_text() == "local\n"


def test_a_pinned_submodule_follows_the_new_pin_unless_it_holds_work(
    tmp_path: Path, repos: tuple[Path, Path, Path]
) -> None:
    remote, shared, other = repos
    library = tmp_path / "library.git"
    run("git", "init", "-q", "--bare", "-b", "main", str(library), cwd=tmp_path)
    source = clone(library, tmp_path / "library")
    pins = []
    for version in ("1", "2", "3"):
        (source / "v.txt").write_text(version)
        run("git", "add", "v.txt", cwd=source)
        run("git", "commit", "-qm", version, cwd=source)
        pins.append(run("git", "rev-parse", "HEAD", cwd=source).strip())
    run("git", "push", "-q", "origin", "main", cwd=source)
    allow = ("-c", "protocol.file.allow=always")

    def pin(commit: str) -> None:
        run("git", *allow, "submodule", "update", "--init", "-q", "lib", cwd=other)
        run("git", "-C", "lib", "checkout", "-q", "--detach", commit, cwd=other)
        run("git", "add", "lib", cwd=other)
        run("git", "commit", "-qm", "pin", cwd=other)
        run("git", "push", "-q", "origin", "main", cwd=other)

    run("git", *allow, "submodule", "add", "-q", str(library), "lib", cwd=other)
    run("git", "-C", "lib", "checkout", "-q", "--detach", pins[0], cwd=other)
    run("git", "add", "lib", cwd=other)
    run("git", "commit", "-qm", "add lib", cwd=other)
    run("git", "push", "-q", "origin", "main", cwd=other)
    run("git", "pull", "-q", cwd=shared)
    run("git", *allow, "submodule", "update", "--init", "-q", cwd=shared)

    pin(pins[1])
    sync(Repository(shared))
    assert run("git", "rev-parse", "HEAD", cwd=shared / "lib").strip() == pins[1]

    (shared / "lib" / "v.txt").write_text("local work")
    pin(pins[2])
    sync(Repository(shared))
    assert run("git", "rev-parse", "HEAD", cwd=shared / "lib").strip() == pins[1]
    assert (shared / "lib" / "v.txt").read_text() == "local work"
