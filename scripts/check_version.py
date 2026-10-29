#!/usr/bin/env python3
"""On a PR, the workspace version must be higher than the base branch's."""
import re
import subprocess
import sys


def version(text: str) -> tuple[int, ...]:
    m = re.search(r'\[workspace\.package\][^\[]*?version\s*=\s*"(\d+)\.(\d+)\.(\d+)', text, re.S)
    if not m:
        sys.exit("no [workspace.package] version")
    return tuple(int(x) for x in m.groups())


def main() -> None:
    base = sys.argv[1] if len(sys.argv) > 1 else "origin/main"
    head = version(open("Cargo.toml", encoding="utf-8").read())
    base_text = subprocess.run(["git", "show", f"{base}:Cargo.toml"], check=True,
                               capture_output=True, text=True).stdout
    if head <= version(base_text):
        sys.exit(f"version {head} is not above {base}'s {version(base_text)}")
    print(f"version {head} above {base}")


if __name__ == "__main__":
    main()
