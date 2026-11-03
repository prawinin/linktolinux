# Building LinkToLinux

The desktop uses Tauri, Rust and React. The Android companion uses Kotlin and Android platform APIs.

## Linux

Install Node.js 22.12+, Rust and the [Tauri Linux prerequisites](https://v2.tauri.app/start/prerequisites/#linux).

Place the [companion APK](https://github.com/prawinin/linktolinux/releases/download/v1.0.0/LinkToLinux-Companion.apk) at `release/LinkToLinux-Companion.apk`, then run:

```sh
./scripts/build.sh
./launch.sh
```

The build writes the executable and installer to `release/`. Set `BUILD_JOBS` to limit workers; the default is 4, with a maximum of 6.

## Android

Install JDK 17, Android SDK platform 34 and build tools 34. Set `JAVA_HOME` and `ANDROID_HOME`, or configure the SDK in `android-companion/local.properties`.

For a debug APK:

```sh
cd android-companion
./gradlew --no-daemon --max-workers=3 assembleDebug
```

For a signed release, supply the signing variables listed in [.env.example](.env.example), or use a private `android-companion/signing/signing.properties` file with `storeFile`, `storePassword`, `keyAlias` and `keyPassword`. Relative keystore paths resolve from `android-companion/`.

From the repository root:

```sh
./scripts/build.sh --android
```

Android updates must be signed with the same key as the installed app. Keep signing files private.

## Tests

Build the desktop before running the installer tests.

```sh
npm test
npm run build
python3 -m unittest discover -s tests -p 'test_*.py'
cargo test --locked --manifest-path src-tauri/Cargo.toml -j 4
cargo clippy --locked --manifest-path src-tauri/Cargo.toml -j 4 --no-deps -- -D warnings
```

Android tests and lint:

```sh
cd android-companion
./gradlew --no-daemon --max-workers=3 testDebugUnitTest lintDebug
```

## Dependency licenses

After changing dependencies, regenerate the license inventory with frontend packages installed:

```sh
cargo metadata --locked --format-version 1 --filter-platform x86_64-unknown-linux-gnu --manifest-path src-tauri/Cargo.toml > /tmp/linktolinux-dependencies.json
python3 scripts/collect-licenses.py --cargo-metadata /tmp/linktolinux-dependencies.json
```

The collector requires a license text for each dependency. Upstream copies are kept in `LICENSES/upstream/`; Android notices are bundled in `android-companion/app/src/main/assets/`.
