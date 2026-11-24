import json
import os
import stat
import subprocess
from pathlib import Path

import pytest
from rebase import pull_request_base, rebase_tree
from repository import Failure, Repository


def fake_gh(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, output: str, code: int = 0) -> None:
    gh = tmp_path / "bin" / "gh"
    gh.parent.mkdir()
    gh.write_text(f"#!/bin/sh\ncat <<'EOF'\n{output}\nEOF\nexit {code}\n")
    gh.chmod(gh.stat().st_mode | stat.S_IEXEC)
    monkeypatch.setenv("PATH", f"{gh.parent}{os.pathsep}{os.environ['PATH']}")


def test_the_base_is_the_open_pull_requests_base(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake_gh(tmp_path, monkeypatch, json.dumps([{"baseRefName": "someone/base", "number": 7}]))
    assert pull_request_base(Repository(tmp_path), "someone/stacked") == "someone/base"


def test_a_branch_without_a_pull_request_rebases_onto_main(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake_gh(tmp_path, monkeypatch, "[]")
    assert pull_request_base(Repository(tmp_path), "someone/new") == "main"


def test_an_unanswered_question_refuses_instead_of_guessing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake_gh(tmp_path, monkeypatch, "", code=1)
    with pytest.raises(Failure, match="--base"):
        pull_request_base(Repository(tmp_path), "someone/topic")


def command(*arguments: str, cwd: Path) -> str:
    return subprocess.run(
        arguments, cwd=cwd, check=True, capture_output=True, text=True
    ).stdout.strip()


def checkout(remote: Path, path: Path) -> Path:
    command("git", "clone", "-q", str(remote), str(path), cwd=path.parent)
    for name in ("user.name", "user.email"):
        command("git", "config", name, "Test", cwd=path)
    return path


def test_a_branch_submodule_rebases_before_its_superproject(
    tmp_path: Path,
) -> None:
    library_remote = tmp_path / "library.git"
    command("git", "init", "-q", "--bare", "-b", "main", str(library_remote), cwd=tmp_path)
    library = checkout(library_remote, tmp_path / "library")
    (library / "v.txt").write_text("base\n")
    command("git", "add", "v.txt", cwd=library)
    command("git", "commit", "-qm", "base", cwd=library)
    base = command("git", "rev-parse", "HEAD", cwd=library)
    command("git", "push", "-q", "origin", "main", cwd=library)
    command("git", "switch", "-q", "-c", "topic", cwd=library)
    (library / "a.txt").write_text("topic\n")
    command("git", "add", "a.txt", cwd=library)
    command("git", "commit", "-qm", "topic change", cwd=library)
    command("git", "push", "-q", "origin", "topic", cwd=library)
    command("git", "switch", "-q", "main", cwd=library)
    (library / "b.txt").write_text("main\n")
    command("git", "add", "b.txt", cwd=library)
    command("git", "commit", "-qm", "main change", cwd=library)
    library_main = command("git", "rev-parse", "HEAD", cwd=library)
    command("git", "push", "-q", "origin", "main", cwd=library)

    remote = tmp_path / "super.git"
    command("git", "init", "-q", "--bare", "-b", "main", str(remote), cwd=tmp_path)
    other = checkout(remote, tmp_path / "other")
    allow = ("-c", "protocol.file.allow=always")
    command("git", *allow, "submodule", "add", "-q", str(library_remote), "lib", cwd=other)
    command("git", "-C", "lib", "checkout", "-q", "--detach", base, cwd=other)
    command("git", "add", "lib", cwd=other)
    command("git", "commit", "-qm", "add lib", cwd=other)
    command("git", "push", "-q", "origin", "main", cwd=other)
    shared = checkout(remote, tmp_path / "shared")
    command("git", *allow, "submodule", "update", "--init", "-q", cwd=shared)

    command("git", "switch", "-q", "-c", "topic", cwd=shared)
    command("git", "-C", "lib", "fetch", "-q", "origin", cwd=shared)
    command("git", "-C", "lib", "checkout", "-q", "-B", "topic", "origin/topic", cwd=shared)
    command("git", "add", "lib", cwd=shared)
    command("git", "commit", "-qm", "pin topic", cwd=shared)
    (shared / "work.txt").write_text("work\n")
    command("git", "add", "work.txt", cwd=shared)
    command("git", "commit", "-qm", "work", cwd=shared)

    command("git", "-C", "lib", "fetch", "-q", "origin", "main", cwd=other)
    command("git", "-C", "lib", "checkout", "-q", "--detach", library_main, cwd=other)
    command("git", "add", "lib", cwd=other)
    (other / "upstream.txt").write_text("upstream\n")
    command("git", "add", "upstream.txt", cwd=other)
    command("git", "commit", "-qm", "upstream", cwd=other)
    command("git", "push", "-q", "origin", "main", cwd=other)
    new_base = command("git", "rev-parse", "HEAD", cwd=other)

    rebase_tree(Repository(shared), "main", set(), {})

    library_head = command("git", "rev-parse", "HEAD", cwd=shared / "lib")
    assert command("git", "rev-parse", "HEAD^", cwd=shared / "lib") == library_main
    assert (shared / "lib" / "a.txt").read_text() == "topic\n"
    super_head = command("git", "rev-parse", "HEAD", cwd=shared)
    assert command("git", "rev-parse", "HEAD~2", cwd=shared) == new_base
    assert command("git", "ls-tree", super_head, "lib", cwd=shared).split()[2] == library_head
    assert (shared / "work.txt").read_text() == "work\n"
    assert command("git", "rev-parse", "topic", cwd=remote) == super_head
    assert command("git", "rev-parse", "topic", cwd=library_remote) == library_head
    assert command("git", "status", "--porcelain", cwd=shared) == ""
