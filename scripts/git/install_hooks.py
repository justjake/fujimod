"""Install this directory's Git hooks into a checkout's common Git directory.

python scripts/git/install_hooks.py [--repo PATH]
"""

import argparse
import shutil
import subprocess
from pathlib import Path

HOOKS = Path(__file__).resolve().parent / "hooks"


class Arguments(argparse.Namespace):
    repo: Path


def install(repo: Path) -> list[Path]:
    hooks = subprocess.run(
        ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
        cwd=repo,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    installed = []
    for hook in sorted(HOOKS.iterdir()):
        target = Path(hooks) / "hooks" / hook.name
        shutil.copyfile(hook, target)
        target.chmod(0o755)
        installed.append(target)
    return installed


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    args = parser.parse_args(namespace=Arguments())
    for path in install(args.repo):
        print(f"Installed {path}")


if __name__ == "__main__":
    main()
