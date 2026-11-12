"""Install Git hooks into a checkout's common Git directory.

python scripts/git/install_hooks.py [--repo PATH] [--hooks PATH]
"""

import argparse
import shutil
import subprocess
from pathlib import Path

HOOKS = Path(__file__).resolve().parent / "hooks"


class Arguments(argparse.Namespace):
    repo: Path
    hooks: Path


def install(repo: Path, hooks: Path = HOOKS) -> list[Path]:
    common = subprocess.run(
        ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
        cwd=repo,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    installed = []
    for hook in sorted(hooks.iterdir()):
        target = Path(common) / "hooks" / hook.name
        shutil.copyfile(hook, target)
        target.chmod(0o755)
        installed.append(target)
    return installed


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--hooks", type=Path, default=HOOKS)
    args = parser.parse_args(namespace=Arguments())
    for path in install(args.repo, args.hooks):
        print(f"Installed {path}")


if __name__ == "__main__":
    main()
