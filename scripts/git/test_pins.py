"""Rebase pin mapping: submodule pins follow a rebased submodule."""
import os
import subprocess
from pathlib import Path

import pytest
from commit import Failure, Repository
from sync import replay, resolve_pinned_submodules


def run(*arguments: str, cwd: Path, env: dict[str, str] | None = None) -> str:
    return subprocess.run(
        arguments, cwd=cwd, check=True, capture_output=True, text=True,
        env=None if env is None else {**os.environ, **env},
    ).stdout


def clone(remote: Path, path: Path) -> Path:
    run("git", "clone", "-q", str(remote), str(path), cwd=path.parent)
    for name in ("user.name", "user.email"):
        run("git", "config", name, "Test", cwd=path)
    return path


def head_of(path: Path) -> str:
    return run("git", "rev-parse", "HEAD", cwd=path).strip()


def pin_repositories(tmp_path: Path, resolved: bool = False) -> dict[str, object]:
    """A superproject pinning a submodule whose topic commit has a rebased twin.

    ``resolved``: the twin's change differs, as a replay that resolved a
    conflict leaves it, keeping only the subject, author and author time."""
    library = tmp_path / "library.git"
    run("git", "init", "-q", "--bare", "-b", "main", str(library), cwd=tmp_path)
    source = clone(library, tmp_path / "library")
    (source / "v.txt").write_text("base\n")
    run("git", "add", "v.txt", cwd=source)
    run("git", "commit", "-qm", "base", cwd=source)
    base = head_of(source)
    run("git", "push", "-q", "origin", "main", cwd=source)
    run("git", "switch", "-q", "-c", "topic", cwd=source)
    (source / "a.txt").write_text("topic\n")
    run("git", "add", "a.txt", cwd=source)
    run("git", "commit", "-qm", "topic change", cwd=source)
    pinned = head_of(source)
    run("git", "push", "-q", "origin", "topic", cwd=source)
    run("git", "switch", "-q", "main", cwd=source)
    (source / "b.txt").write_text("main\n")
    run("git", "add", "b.txt", cwd=source)
    run("git", "commit", "-qm", "main change", cwd=source)
    library_main = head_of(source)
    run("git", "push", "-q", "origin", "main", cwd=source)
    run("git", "switch", "-q", "-c", "topic-rebased", "main", cwd=source)
    (source / "a.txt").write_text("topic resolved\n" if resolved else "topic\n")
    run("git", "add", "a.txt", cwd=source)
    when = run("git", "log", "-1", "--format=%ad", "--date=raw", pinned, cwd=source).strip()
    run("git", "commit", "-qm", "topic change", cwd=source, env={"GIT_AUTHOR_DATE": when})
    rebased = head_of(source)
    assert rebased != pinned
    run("git", "push", "-q", "origin", "topic-rebased", cwd=source)

    remote = tmp_path / "super.git"
    run("git", "init", "-q", "--bare", "-b", "main", str(remote), cwd=tmp_path)
    other = clone(remote, tmp_path / "super-other")
    allow = ("-c", "protocol.file.allow=always")
    run("git", *allow, "submodule", "add", "-q", str(library), "lib", cwd=other)
    run("git", "-C", "lib", "checkout", "-q", "--detach", base, cwd=other)
    run("git", "add", "lib", cwd=other)
    run("git", "commit", "-qm", "add lib", cwd=other)
    run("git", "push", "-q", "origin", "main", cwd=other)
    base_commit = head_of(other)
    shared = clone(remote, tmp_path / "shared")
    run("git", *allow, "submodule", "update", "--init", "-q", cwd=shared)

    run("git", "switch", "-q", "-c", "topic", cwd=shared)
    run("git", "-C", "lib", "fetch", "-q", "origin", cwd=shared)
    run("git", "-C", "lib", "checkout", "-q", "--detach", pinned, cwd=shared)
    run("git", "add", "lib", cwd=shared)
    run("git", "commit", "-qm", "pin topic", cwd=shared)
    pin_commit = head_of(shared)
    (shared / "work.txt").write_text("work\n")
    run("git", "add", "work.txt", cwd=shared)
    run("git", "commit", "-qm", "work", cwd=shared)
    work_commit = head_of(shared)

    run("git", "-C", "lib", "fetch", "-q", "origin", "main", cwd=other)
    run("git", "-C", "lib", "checkout", "-q", "--detach", library_main, cwd=other)
    run("git", "add", "lib", cwd=other)
    (other / "upstream.txt").write_text("upstream\n")
    run("git", "add", "upstream.txt", cwd=other)
    run("git", "commit", "-qm", "upstream", cwd=other)
    run("git", "push", "-q", "origin", "main", cwd=other)
    run("git", "fetch", "-q", "origin", "main", cwd=shared)

    run("git", "-C", "lib", "fetch", "-q", "origin", "topic-rebased", cwd=shared)
    run(
        "git",
        "-C",
        "lib",
        "checkout",
        "-q",
        "-B",
        "topic",
        "origin/topic-rebased",
        cwd=shared,
    )
    return dict(
        shared=shared,
        rebased=rebased,
        main=library_main,
        pinned=pinned,
        pin_commit=pin_commit,
        work_commit=work_commit,
        new_base=head_of(other),
        base_commit=base_commit,
        base=base,
    )


def test_replay_maps_a_moved_pin_to_the_rebased_submodule_commit(
    tmp_path: Path,
) -> None:
    repos = pin_repositories(tmp_path)
    shared = repos["shared"]
    assert isinstance(shared, Path)
    before = head_of(shared)
    head, kept, dropped = replay(
        Repository(shared),
        repos["new_base"],
        [repos["pin_commit"], repos["work_commit"]],
    )
    assert (kept, dropped) == (2, 0)
    assert run("git", "rev-parse", f"{head}^^", cwd=shared).strip() == repos["new_base"]
    pin = run("git", "ls-tree", f"{head}^", "lib", cwd=shared).split()[2]
    assert pin == repos["rebased"]
    assert pin != repos["main"]
    run("git", "cat-file", "-e", f"{head}:work.txt", cwd=shared)
    # Replay never moves a checkout.
    assert head_of(shared) == before


def test_replay_maps_a_pin_whose_twin_resolved_a_conflict(tmp_path: Path) -> None:
    repos = pin_repositories(tmp_path, resolved=True)
    shared = repos["shared"]
    assert isinstance(shared, Path)
    head, kept, dropped = replay(
        Repository(shared), repos["new_base"], [repos["pin_commit"], repos["work_commit"]]
    )
    assert (kept, dropped) == (2, 0)
    assert run("git", "ls-tree", f"{head}^", "lib", cwd=shared).split()[2] == repos["rebased"]


def test_replay_names_a_pin_with_no_rebased_commit(tmp_path: Path) -> None:
    repos = pin_repositories(tmp_path)
    shared = repos["shared"]
    assert isinstance(shared, Path)
    unknown = "f" * 40
    run(
        "git",
        "update-index",
        "--add",
        "--cacheinfo",
        f"160000,{unknown},lib",
        cwd=shared,
    )
    run("git", "commit", "-qm", "pin unknown", cwd=shared)
    with pytest.raises(Failure, match=f"{unknown}.*patch id"):
        replay(Repository(shared), repos["new_base"], [head_of(shared)])


def test_replay_rewrites_a_clean_pin_move_to_the_rebased_commit(tmp_path: Path) -> None:
    repos = pin_repositories(tmp_path)
    shared = repos["shared"]
    assert isinstance(shared, Path)
    # The new base still pins the original commit, so this pin commit applies
    # cleanly; its pin still moves to the rebased submodule commit.
    head, kept, dropped = replay(
        Repository(shared), repos["base_commit"], [repos["pin_commit"]]
    )
    assert (kept, dropped) == (1, 0)
    assert run("git", "ls-tree", head, "lib", cwd=shared).split()[2] == repos["rebased"]


def test_replay_prefers_the_submodule_rebases_commit_map(tmp_path: Path) -> None:
    repos = pin_repositories(tmp_path)
    shared = repos["shared"]
    assert isinstance(shared, Path)
    # The recorded map wins over the patch id, which would pick the rebased twin.
    head, kept, dropped = replay(
        Repository(shared),
        repos["base_commit"],
        [repos["pin_commit"]],
        pin_maps={"lib": {repos["pinned"]: repos["main"]}},
    )
    assert (kept, dropped) == (1, 0)
    assert run("git", "ls-tree", head, "lib", cwd=shared).split()[2] == repos["main"]


def test_replay_keeps_the_new_base_pin_when_a_commit_leaves_it_alone(
    tmp_path: Path,
) -> None:
    repos = pin_repositories(tmp_path)
    shared = repos["shared"]
    assert isinstance(shared, Path)
    # ``work`` changes only work.txt; its gitlink still names the pin its
    # parent set, while the new base pins the submodule's main commit.
    pin_commit, note = repos["pin_commit"], repos["work_commit"]
    base_tree = run(
        "git", "rev-parse", f"{repos['new_base']}^{{tree}}", cwd=shared
    ).strip()
    tree = resolve_pinned_submodules(
        Repository(shared), repos["new_base"], note, pin_commit, [base_tree, "lib"], {}
    )
    assert run("git", "ls-tree", tree, "lib", cwd=shared).split()[2] == repos["main"]
