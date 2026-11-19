import json
import os
import stat
from pathlib import Path

import pytest
from rebase import pull_request_base
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
