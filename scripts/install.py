#!/usr/bin/env python3
"""Package a local Linux release and run the standalone installer."""
from __future__ import annotations

from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    try:
        result = subprocess.run([sys.executable, str(ROOT / "scripts/package-linux.py")], capture_output=True, text=True, timeout=120, check=False)
        if result.returncode:
            print(result.stderr.strip(), file=sys.stderr)
            return result.returncode
        artifact = Path(result.stdout.strip())
        if artifact.parent != ROOT / "release" or artifact.suffix != ".run" or not artifact.is_file():
            print("The package builder did not produce a release installer.", file=sys.stderr)
            return 1
        return subprocess.run([str(artifact), *sys.argv[1:]], timeout=600, check=False).returncode
    except (OSError, subprocess.TimeoutExpired) as error:
        print(f"Cannot install LinkToLinux: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
