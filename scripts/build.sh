#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
if [[ $# -gt 1 || ($# -eq 1 && "$1" != "--android") ]]; then
    printf '%s\n' 'Usage: ./scripts/build.sh [--android]' >&2
    exit 2
fi
JOBS="${BUILD_JOBS:-4}"
if [[ ! "$JOBS" =~ ^[1-6]$ ]]; then
    printf '%s\n' 'BUILD_JOBS must be between 1 and 6.' >&2
    exit 2
fi
if ! command -v cargo >/dev/null && [[ -x "$HOME/.cargo/bin/cargo" ]]; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi
npm ci --no-audit --no-fund
npm run build
if [[ ! -v CARGO_ENCODED_RUSTFLAGS ]]; then
    CARGO_ENCODED_RUSTFLAGS="$(python3 -c 'import os; print(chr(31).join(os.environ.get("RUSTFLAGS", "").split()), end="")')"
fi
FLAG_SEPARATOR=$'\x1f'
for MAPPING in "$HOME=user" "$ROOT=linktolinux" "${CARGO_HOME:-$HOME/.cargo}=rust-dependencies"; do
    if [[ -n "$CARGO_ENCODED_RUSTFLAGS" ]]; then CARGO_ENCODED_RUSTFLAGS+="$FLAG_SEPARATOR"; fi
    CARGO_ENCODED_RUSTFLAGS+="--remap-path-prefix=$MAPPING"
done
export CARGO_ENCODED_RUSTFLAGS
cargo build --locked --release --manifest-path src-tauri/Cargo.toml -j "$JOBS"
mkdir -p release
install -m755 src-tauri/target/release/linktolinux release/linktolinux.new
mv -f release/linktolinux.new release/linktolinux
install -Dm644 THIRD_PARTY_NOTICES.md release/THIRD_PARTY_NOTICES.md
install -Dm644 LICENSE release/LICENSE.txt
install -Dm644 LICENSES/dependencies.txt release/LICENSES/dependencies.txt
install -Dm644 LICENSES/scrcpy-Apache-2.0.txt release/LICENSES/scrcpy-Apache-2.0.txt
if [[ "${1:-}" == "--android" ]]; then
    (
        cd android-companion
        ./gradlew --no-daemon --max-workers="$JOBS" assembleRelease lintRelease
    )
    install -m644 android-companion/app/build/outputs/apk/release/app-release.apk release/LinkToLinux-Companion.apk.new
    mv -f release/LinkToLinux-Companion.apk.new release/LinkToLinux-Companion.apk
fi
python3 scripts/package-linux.py
printf '%s\n' 'Release artifacts are in release/.'
