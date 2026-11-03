#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export LINKTOLINUX_APK="${LINKTOLINUX_APK:-$ROOT/release/LinkToLinux-Companion.apk}"
export PATH="$HOME/.local/bin:$PATH"
if [[ -x "$ROOT/release/linktolinux" ]]; then
    exec "$ROOT/release/linktolinux" "$@"
elif [[ -x "$ROOT/src-tauri/target/release/linktolinux" ]]; then
    exec "$ROOT/src-tauri/target/release/linktolinux" "$@"
else
    printf '%s\n' 'Release binary not found. Run ./scripts/build.sh first.' >&2
    exit 1
fi
