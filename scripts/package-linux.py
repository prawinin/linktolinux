#!/usr/bin/env python3
"""Build a deterministic offline installer from the Linux release artifacts."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import struct
import subprocess
import sys
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parent.parent
APP_ID = "dev.linktolinux.app"
HEADER = b'''#!/bin/sh
set -eu
if ! command -v python3 >/dev/null 2>&1; then
    printf '%s\\n' 'LinkToLinux installation requires Python 3.10 or newer.' >&2
    exit 1
fi
exec python3 -c '
import sys, zipfile
if sys.version_info < (3, 10):
    sys.stderr.write("LinkToLinux installation requires Python 3.10 or newer.\\n")
    sys.exit(1)
archive_path = sys.argv.pop(1)
sys.argv[0] = archive_path
try:
    with zipfile.ZipFile(archive_path) as archive:
        entry = archive.getinfo("__main__.py")
        if entry.file_size > 1048576:
            raise ValueError("Installer entry point is too large")
        source = archive.read(entry)
    exec(compile(source, archive_path + "!/installer.py", "exec"), {"__name__": "__main__", "__file__": archive_path})
except (OSError, ValueError, zipfile.BadZipFile) as error:
    sys.stderr.write("LinkToLinux installer could not be opened: " + str(error) + "\\n")
    sys.exit(1)
' "$0" "$@"
# LinkToLinux payload follows.
'''


def read_regular(path: Path) -> bytes:
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"Required release file is missing or is not regular: {path.name}")
    if path.stat().st_size > 64 * 1024 * 1024:
        raise ValueError(f"Release file is too large: {path.name}")
    return path.read_bytes()


def binary_architecture(binary: bytes) -> str:
    if len(binary) < 64 or binary[:6] != b"\x7fELF\x02\x01":
        raise ValueError("The Linux release must be a 64-bit little-endian ELF executable")
    architecture = {62: "x86_64", 183: "aarch64"}.get(struct.unpack_from("<H", binary, 18)[0])
    if architecture is None:
        raise ValueError("Unsupported Linux executable architecture")
    return architecture


def minimum_glibc(binary: Path) -> str:
    try:
        result = subprocess.run(["readelf", "--version-info", str(binary)], capture_output=True, text=True, check=True, timeout=15)
    except (OSError, subprocess.SubprocessError) as error:
        raise ValueError("GNU readelf is required to inspect the Linux runtime requirements") from error
    versions = [tuple(int(part) for part in value.split(".")) for value, flags in
                re.findall(r"Name: GLIBC_([0-9]+(?:\.[0-9]+)+)\s+Flags:\s+(\S+)", result.stdout)
                if flags != "WEAK"]
    if not versions:
        raise ValueError("The Linux executable does not declare its glibc requirements")
    return ".".join(str(part) for part in max(versions))


def installation_notes(version: str, architecture: str, libc: str) -> bytes:
    return f"""LinkToLinux {version}

INSTALL
Run the .run installer, then open LinkToLinux from your applications menu.
For a terminal installation, add --yes --no-gui.

CONNECT
Install the Android companion using the QR code in Connection. Put both
devices on the same Wi-Fi, then enter the desktop address, port and pairing
code shown on Linux. Enable Remember this desktop to reconnect automatically.

REQUIREMENTS
Linux {architecture}, glibc {libc}+, GTK 3 and WebKitGTK 4.1.
Installation and removal require Python 3.10+.

Camera preview uses ffmpeg. Virtual webcam output requires v4l2loopback.
Microphone sharing uses PipeWire, pactl, pw-cat and ffmpeg. Clipboard sync
uses wl-clipboard on Wayland or xclip on X11. ADB tools require adb.

Start camera or microphone sharing while the Android companion is visible.
Use its notification to send phone clipboard text while using another app.

UPDATES AND REMOVAL
Close the app and run a newer installer to update. To remove it, choose
Uninstall from the application-menu actions. Your preferences, pairing and
received files are kept.

PRIVACY
The local connection is authenticated but not encrypted. Use trusted private
Wi-Fi and do not expose the listener to the internet.

LICENSE
Free for personal, non-commercial use. See LICENSE.txt and
THIRD_PARTY_NOTICES.md for the full terms and third-party licenses.
""".encode("utf-8")


def add_entry(archive: zipfile.ZipFile, name: str, data: bytes, mode: int) -> None:
    entry = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
    entry.create_system = 3
    entry.external_attr = (stat.S_IFREG | mode) << 16
    entry.compress_type = zipfile.ZIP_DEFLATED
    archive.writestr(entry, data, compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)


def build_installer(output: Path | None = None) -> Path:
    config = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text(encoding="utf-8"))
    version = config.get("version")
    if config.get("identifier") != APP_ID or not isinstance(version, str) or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.-]+)?", version):
        raise ValueError("Invalid application identity or release version")
    binary_path = ROOT / "release/linktolinux"
    binary = read_regular(binary_path)
    architecture = binary_architecture(binary)
    libc = minimum_glibc(binary_path)
    sources = {
        "LinkToLinux-Companion.apk": ROOT / "release/LinkToLinux-Companion.apk",
        "icon.svg": ROOT / "assets/icon.svg",
        "icons/32.png": ROOT / "src-tauri/icons/32x32.png",
        "icons/128.png": ROOT / "src-tauri/icons/128x128.png",
        "icons/256.png": ROOT / "src-tauri/icons/128x128@2x.png",
        "icons/512.png": ROOT / "src-tauri/icons/icon.png",
        "linktolinux.desktop": ROOT / "linktolinux.desktop",
        "THIRD_PARTY_NOTICES.md": ROOT / "THIRD_PARTY_NOTICES.md",
        "LICENSE.txt": ROOT / "LICENSE",
        "LICENSES/dependencies.txt": ROOT / "LICENSES/dependencies.txt",
        "LICENSES/scrcpy-Apache-2.0.txt": ROOT / "LICENSES/scrcpy-Apache-2.0.txt",
    }
    files = {name: read_regular(path) for name, path in sources.items()}
    files["linktolinux"] = binary
    files["INSTALL.txt"] = installation_notes(version, architecture, libc)
    records = {name: {"sha256": hashlib.sha256(data).hexdigest(), "size": len(data), "mode": 0o755 if name == "linktolinux" else 0o644}
               for name, data in files.items()}
    metadata = {"format": 1, "app_id": APP_ID, "name": "LinkToLinux", "version": version,
                "architecture": architecture, "minimum_glibc": libc, "files": records}
    installer_source = read_regular(ROOT / "packaging/linux/installer.py")
    destination = (output or ROOT / "release" / f"LinkToLinux-{version}-linux-{architecture}.run").expanduser().resolve()
    if destination.suffix != ".run":
        raise ValueError("The installer output filename must end in .run")
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(mode="w+b", prefix=".linktolinux-package-", dir=destination.parent, delete=False) as handle:
            temporary = Path(handle.name)
            handle.write(HEADER)
            with zipfile.ZipFile(handle, "w") as archive:
                add_entry(archive, "__main__.py", installer_source, 0o644)
                add_entry(archive, "manifest.json", (json.dumps(metadata, indent=2, sort_keys=True) + "\n").encode("utf-8"), 0o644)
                for name in sorted(files):
                    add_entry(archive, "payload/" + name, files[name], records[name]["mode"])
            handle.flush()
            os.fsync(handle.fileno())
        temporary.chmod(0o755)
        temporary.replace(destination)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    return destination


def release_checksums(installer: Path) -> None:
    directory = ROOT / "release"
    names = ["LinkToLinux-Companion.apk", installer.name]
    lines = [hashlib.sha256(read_regular(directory / name)).hexdigest() + "  " + name for name in sorted(names)]
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", prefix=".checksums-", dir=directory, delete=False) as handle:
            temporary = Path(handle.name)
            handle.write("\n".join(lines) + "\n")
            handle.flush()
            os.fsync(handle.fileno())
        temporary.chmod(0o644)
        temporary.replace(directory / "SHA256SUMS")
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="Destination .run file (defaults to release/)")
    args = parser.parse_args()
    try:
        artifact = build_installer(args.output)
        if args.output is None:
            release_checksums(artifact)
        print(artifact)
        return 0
    except (OSError, ValueError, zipfile.BadZipFile) as error:
        print(f"Cannot package LinkToLinux: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
