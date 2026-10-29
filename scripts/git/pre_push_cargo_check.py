"""Check Rust workspace crates affected by commits pushed to the main branch."""

from __future__ import annotations

import json
import subprocess
import sys
from collections.abc import Iterable
from pathlib import Path, PurePosixPath

MAIN_REF = "refs/heads/main"
WASM_TARGET = "wasm32-unknown-unknown"
# The web app's production browser module is built by
# bindings/javascript/scripts/build.ts.
BROWSER_WASM_PACKAGE = "fujimod-javascript"
RUST_INPUT_NAMES = {
    "Cargo.lock",
    "Cargo.toml",
    "rust-toolchain",
    "rust-toolchain.toml",
}


def parse_updates(lines: Iterable[str]) -> list[tuple[str, str, str, str]]:
    updates = []
    for line in lines:
        fields = line.split()
        if len(fields) != 4:
            raise ValueError(f"invalid pre-push update: {line.rstrip()}")
        updates.append(tuple(fields))
    return updates


def _git_output(root: Path, *arguments: str) -> bytes:
    result = subprocess.run(
        ["git", *arguments],
        cwd=root,
        capture_output=True,
        check=False,
    )
    if result.returncode:
        detail = result.stderr.decode("utf-8", errors="replace").strip()
        raise RuntimeError(detail or f"git {' '.join(arguments)} failed")
    return result.stdout


def is_zero_oid(value: str) -> bool:
    return bool(value) and all(character == "0" for character in value)


def pushed_paths(root: Path, updates: Iterable[tuple[str, str, str, str]]) -> set[str]:
    paths = set()
    for _local_ref, local_sha, remote_ref, remote_sha in updates:
        if remote_ref != MAIN_REF or is_zero_oid(local_sha):
            continue
        revisions = ["rev-list", "--reverse", local_sha]
        if not is_zero_oid(remote_sha):
            revisions.append(f"^{remote_sha}")
        commits = _git_output(root, *revisions).decode("ascii").splitlines()
        for commit in commits:
            changed = _git_output(
                root,
                "diff-tree",
                "--root",
                "--no-commit-id",
                "-m",
                "--no-renames",
                "--name-only",
                "-r",
                "-z",
                commit,
            )
            paths.update(
                part.decode("utf-8", errors="surrogateescape")
                for part in changed.split(b"\0")
                if part
            )
    return paths


def is_rust_input(path: str) -> bool:
    candidate = PurePosixPath(path)
    return (
        candidate.suffix == ".rs"
        or candidate.name in RUST_INPUT_NAMES
        or bool(candidate.parts and candidate.parts[0] == ".cargo")
    )


def select_touched_crates(
    paths: Iterable[str],
    packages: Iterable[dict],
    workspace_root: Path,
) -> list[dict]:
    root = workspace_root.resolve()
    package_roots = []
    for package in packages:
        manifest = Path(package["manifest_path"]).resolve()
        try:
            relative_root = manifest.parent.relative_to(root)
        except ValueError:
            continue
        package_roots.append((relative_root, manifest.relative_to(root), package))

    selected = {}
    all_packages = {package["name"]: package for _, _, package in package_roots}
    for path in paths:
        if not is_rust_input(path):
            continue
        candidate = PurePosixPath(path)
        if candidate in {
            PurePosixPath("Cargo.toml"),
            PurePosixPath("Cargo.lock"),
            PurePosixPath("rust-toolchain"),
            PurePosixPath("rust-toolchain.toml"),
        } or (candidate.parts and candidate.parts[0] == ".cargo"):
            selected.update(all_packages)
            continue

        matches = [
            (len(package_root.parts), package)
            for package_root, manifest, package in package_roots
            if candidate == manifest or candidate.is_relative_to(package_root)
        ]
        if matches:
            _, package = max(matches, key=lambda match: match[0])
            selected[package["name"]] = package
    return [selected[name] for name in sorted(selected)]


def select_browser_wasm_packages(
    metadata: dict,
    touched_packages: Iterable[dict],
    workspace_root: Path,
) -> list[dict]:
    """Select touched workspace crates built for the production browser module."""
    workspace_members = set(metadata["workspace_members"])
    packages = {
        package["id"]: package
        for package in metadata["packages"]
        if package["id"] in workspace_members
    }
    browser_packages = [
        package
        for package in packages.values()
        if package["name"] == BROWSER_WASM_PACKAGE
        and Path(package["manifest_path"])
        .parent.resolve()
        .is_relative_to(workspace_root.resolve())
    ]
    if len(browser_packages) != 1:
        raise ValueError(
            f"expected one workspace package named {BROWSER_WASM_PACKAGE}, "
            f"found {len(browser_packages)}"
        )

    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    browser_id = browser_packages[0]["id"]
    if browser_id not in nodes:
        raise ValueError(
            f"Cargo metadata has no dependency node for {BROWSER_WASM_PACKAGE}"
        )

    closure = set()
    pending = [browser_id]
    while pending:
        package_id = pending.pop()
        if package_id in closure:
            continue
        node = nodes.get(package_id)
        if node is None:
            raise ValueError(f"Cargo metadata has no dependency node for {package_id}")
        closure.add(package_id)
        for dependency in node.get("deps", []):
            dependency_id = dependency["pkg"]
            if dependency_id not in workspace_members:
                continue
            kinds = dependency.get("dep_kinds", [])
            if kinds and not any(kind.get("kind") is None for kind in kinds):
                continue
            pending.append(dependency_id)

    touched_ids = {package["id"] for package in touched_packages}
    return sorted(
        (
            package
            for package_id, package in packages.items()
            if package_id in closure and package_id in touched_ids
        ),
        key=lambda package: package["name"],
    )


def workspace_metadata(root: Path) -> dict:
    result = subprocess.run(
        [
            "cargo",
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--filter-platform",
            WASM_TARGET,
        ],
        cwd=root,
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(result.stdout)


def cargo_check_command(
    packages: Iterable[dict], target: str | None = None
) -> list[str]:
    command = ["cargo", "check", "--locked"]
    if target:
        command.extend(("--target", target))
    for package in packages:
        command.extend(("--package", package["name"]))
    return command


def check_updates(root: Path, updates: Iterable[tuple[str, str, str, str]]) -> int:
    main_updates = [update for update in updates if update[2] == MAIN_REF]
    if not main_updates:
        return 0

    paths = pushed_paths(root, main_updates)
    if not any(is_rust_input(path) for path in paths):
        print("No Rust inputs changed; skipping cargo check.", flush=True)
        return 0

    metadata = workspace_metadata(root)
    packages = select_touched_crates(paths, metadata["packages"], root)
    if not packages:
        print(
            "No workspace crates were affected by the pushed Rust inputs.", flush=True
        )
        return 0

    names = ", ".join(package["name"] for package in packages)
    print(f"Checking native target for: {names}", flush=True)
    result = subprocess.run(cargo_check_command(packages), cwd=root, check=False)
    if result.returncode:
        return result.returncode

    wasm_packages = select_browser_wasm_packages(metadata, packages, root)
    if wasm_packages:
        names = ", ".join(package["name"] for package in wasm_packages)
        print(f"Checking {WASM_TARGET} for: {names}", flush=True)
        result = subprocess.run(
            cargo_check_command(wasm_packages, WASM_TARGET),
            cwd=root,
            check=False,
        )
        if result.returncode:
            return result.returncode
    return 0


def main() -> int:
    try:
        root_result = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            capture_output=True,
            text=True,
            check=True,
        )
        root = Path(root_result.stdout.strip())
        return check_updates(root, parse_updates(sys.stdin))
    except (
        OSError,
        RuntimeError,
        ValueError,
        subprocess.CalledProcessError,
        json.JSONDecodeError,
    ) as error:
        print(f"Rust push check failed: {error}", file=sys.stderr, flush=True)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
