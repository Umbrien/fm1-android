# Reproducible local build

## Daily development

Run commands from the repository root. Shell scripts prepare the native library
and firmware assets, then use Gradle for the Material 3 Kotlin shell. Android
Studio can open `Android/` and provide the SDK, editor and Device Manager.
Run `Android/build.sh --core` at least once to prepare native inputs before
building from the IDE.
Start an **arm64** virtual device in Device Manager before installing the APK.

For the checkout configured during local setup, load its ignored environment:

```sh
source .tools/env.sh
./Android/build.sh --core
adb devices -l
adb install -r Android/build/FM1-android-arm64.apk
adb shell am start -n org.fm1.android/.MainActivity
```

`.tools/env.sh` is machine-local and is not supplied by a fresh clone. It selects
the local `.tools/android-sdk`, NDK 27.2.12479018, Java 21, LLVM 20 and `.venv`.
For another checkout, follow the prerequisites below and configure the environment
explicitly (adjust the SDK path if needed):

```sh
source .venv/bin/activate
export JAVA_HOME="$(/usr/libexec/java_home -v 21)"
export ANDROID_HOME="$HOME/Library/Android/sdk"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.2.12479018"
export FM1_NATIVE_LLVM_BIN="/opt/homebrew/opt/llvm@20/bin"
export PATH="$JAVA_HOME/bin:$ANDROID_HOME/platform-tools:$PATH"
```

For Kotlin/JNI-only changes, `./Android/build.sh` reuses the existing native
archive. Rebuild with `--core` after C/Rust engine changes, then reinstall with
`adb install -r` and launch again. The signed APK is
`Android/build/FM1-android-arm64.apk`.

Run `./Android/test.sh` after installation on the test emulator. Its complete
output is saved to `Android/build/test/result.txt`. On 2026-10-21, the standalone
build passed all ten default cases on an arm64 Android 17 emulator (API 37);
external exchange fixtures and physical-device validation were not included.

## Tools

Validated on Apple Silicon macOS with Java 19 and 21, Python 3.12+, Rust, host LLVM 20,
Android SDK platform 35/build-tools 35.0.0 and NDK 27.2.12479018. Use Java 21.
The UI uses Material Components 1.13.0 and its AndroidX dependencies, resolved
from Google Maven and Maven Central. The native engines remain unchanged.

Install the SDK command-line tools normally, then use the installed sdkmanager:

```sh
sdkmanager 'platforms;android-35' 'build-tools;35.0.0' 'ndk;27.2.12479018'
rustup target add aarch64-linux-android
```

Set ANDROID_HOME to the SDK root and ANDROID_NDK_HOME to that exact NDK. NDK host
binaries are discovered under toolchains/llvm/prebuilt rather than assumed to be
Darwin. Linux compilation is configured but has not been tested. Windows is not
validated by these Bash scripts.

Set FM1_NATIVE_LLVM_BIN to a matching clang and llvm-link from LLVM 20. Multi-unit
firmware is combined before per-instance state rewriting, so the linker and IR
compiler must match. The NDK clang handles other translation units. Exact source
and resource fingerprints are checked before native compilation.

## Android UI build

The checked-in wrapper pins Gradle 8.13 with a distribution SHA256. The Android
project pins AGP 8.11.1, Kotlin 2.0.21 and Material Components 1.13.0. Gradle handles
AndroidX dependencies, manifest/resource merging, Kotlin compilation and DEX.
The shell scripts keep Gradle's cache under ignored `.tools/gradle-home/` and
Android user settings under `.tools/android-user/`. First build needs network
access; subsequent builds reuse the downloaded dependencies. `scripts/kotlin-toolchain.sh`
is retained for historical standalone compiler usage, but is not used by these builds.

`Theme.FM1` inherits Material 3 DayNight. DynamicColors applies the system palette
on supported Android 12+ devices, with the standard Material palette as fallback.
Synth rendering and finish palettes stay in `PanelView`; they do not theme the shell.

## Prepare sources and assets

Use an active Python virtualenv with Pillow 13.0.0 supporting RAQM and fonttools.
The family lock records the validated Pillow/FreeType/RAQM versions. Generated
font/image bytes are pinned; another rendering-library version may correctly
fail verification. Restore the recorded build environment rather than changing
expected hashes to accept a different asset.

On Apple Silicon, the prebuilt Pillow 13.0.0 wheel can produce different font
bytes even with the recorded FreeType and RAQM versions. A source build against
Homebrew FreeType 2.14.3 and libraqm 0.11.0 reproduces the locked resources:

```sh
brew install python@3.12 llvm@20 libraqm
python3.12 -m venv .venv
source .venv/bin/activate
python -m pip install --no-binary=pillow 'Pillow==13.0.0' fonttools
export FM1_NATIVE_LLVM_BIN="/opt/homebrew/opt/llvm@20/bin"
```

Check the installed FreeType version before generation; package-manager updates
may install a newer version. Keep the existing resource hashes as the acceptance
check. The expansion source lock excludes Python bytecode caches, whose bytes
depend on the interpreter and absolute checkout path.

```sh
python scripts/prepare.py
```

This explicit command executes the following existing source preparers, then
fetches package records from Core/native/packages.lock.json:

```sh
python Core/native/prepare.py "$PWD/Research/NativeSloop/upstream-sloop-2.8"
python Core/native/clipper_prepare.py "$PWD/Research/NativeClipper/upstream-clipper"
python Core/native/additions_prepare.py "$PWD/Research/NativeAdditions"
python Core/native/prepare_family.py "$PWD/Research/NativeFamily"
python Core/native/official_preview_prepare.py "$PWD/Research/NativeOfficial/upstream-fm1-open"
python Core/native/skiff_prepare.py "$PWD/Research/NativeSkiff/upstream-skiff"
python Core/native/expansion_prepare.py "$PWD/Research/NativeExpansion"
```

Source tags are checked against exact commits; notice/input/resource hashes
are verified. Existing checkouts are never reset. To advance a clean existing
Skiff cache from the previous pin to v1.4, fetch and select its pinned commit
before running preparation (preserve any local source edits first):

```sh
git -C Research/NativeSkiff/upstream-skiff fetch --depth 1 origin tag v1.4
git -C Research/NativeSkiff/upstream-skiff switch --detach c01fa1ed619266d770ea6552b153612e1b77d0a9
```

Release packages require their
exact size and SHA256. Existing mismatching packages are kept and cause failure.
Downloaded source and derived resources retain upstream component terms.

The ignored cache layout is:

```text
Research/NativeSloop/upstream-sloop-2.8/
Research/NativeClipper/{upstream-clipper,clipper-1.1-experimental.fwsc}
Research/Stock/FM-1_015.fwsc
Research/FM1VA/FM-1+VA-102.fwsc
Research/NativeFamily/{felucca,x0x,melodee}/
Research/NativeFamily/{felucca-1.6.1,x0x-1.1.0,melodee-1.4.3}.fwsc
Research/NativeOfficial/upstream-fm1-open/
Research/NativeOfficial/FM1-Open-V13.fwsc
Research/NativeSkiff/upstream-skiff/
Research/NativeSkiff/skiff-1.4.fwsc
Research/NativeExpansion/{fomni,jangada}/
Research/NativeExpansion/{fomni-0.4,jangada-1.0.1}.fwsc
```

SLOOP's exact package lives in its pinned checkout's docs/firmware directory.
A local ignored Research symlink may reuse an existing prepared cache; no host
path or symlink belongs in Git. The source/notice/resource checks still apply.

## Compile, sign and install

```sh
./Android/build.sh --core
adb install -r Android/build/FM1-android-arm64.apk
```

The --core option prepares the Rust static archive and all twelve native engines.
Later Kotlin/JNI-only iterations may omit --core to reuse that archive. APK assets
are checked against packages.lock.json before packaging. Native ELF segments
use 16 KiB alignment. This is an arm64-only APK with minSdk 26 and targetSdk 35.

Android/signing.sh creates an ignored local development key and a random local
password file, both mode 0600. Neither is published. Preserve both for updates;
a new clone creates a different signer. The signed output, intermediate sources,
archives and all package assets remain under ignored build/cache directories.

## Tests

Use a dedicated emulator with synthetic test projects. Instrumentation mutates
that app's test state and private project library; do not point it at a phone
containing unrelated owner work.

```sh
./Android/test.sh
```

For a shorter UI-only pass (Material theme, multitouch/lifecycle and reset), use
`FM1_UI_ONLY=true ./Android/test.sh`. Run it after changing the emulator's system
light/dark setting to validate both modes.

The app must already be built/installed. The script wakes the dedicated emulator
and dismisses its keyguard before injecting real touch events. Default tests need only the exact
engine package assets already in the app and exercise sixteen core/UI cases, including
factory reset persistence for all eleven firmware entries. The
receipt explicitly records that external exchange fixtures were not supplied.
Optional cross-platform testing uses a caller-provided directory containing
swift-generated.fm1project, swift-generated.fm1backup and ios-simulator.fm1project:

```sh
FM1_EXCHANGE_FIXTURES=/path/to/local/synthetic-fixtures ./Android/test.sh
```

Those fixtures are never part of the public repository or consumer APK.
The test package shares the app's local signer and uses standard Android SDK
Instrumentation. The script exits nonzero if its final JSON receipt is absent
or reports failures.

Host Rust regression may also be run with all seven FM1_NATIVE_* environment
variables from Android/build.sh pointing at the prepared cache:

```sh
cargo test --workspace --release
```

Without those environment variables, native-only test cases are conditional and
are not evidence for nine-native-engine compatibility.

Set `FM1_STOCK_PACKAGE="$PWD/Research/Stock/FM-1_015.fwsc"` for the host
stock V15 boot/sound/flash test. The host default uses the interpreter; Android enables the ARM64 JIT.

Set `FM1_VA_PACKAGE="$PWD/Research/FM1VA/FM-1+VA-102.fwsc"` for the
FM-1+VA host interpreter boot/sound/flash test. Its exact package is fetched by
`scripts/prepare.py`; the app does not download it at runtime.

Compiler differential checks (Apple Silicon host, original local packages):

```sh
FM1_COMPILER_PACKAGES="$PWD/Research/Stock/FM-1_015.fwsc:$PWD/Research/FM1VA/FM-1+VA-102.fwsc:$PWD/Research/NativeFamily/felucca-1.6.1.fwsc:$PWD/Research/NativeFamily/x0x-1.1.0.fwsc" cargo test -p fm1-ios-core --release compiler_tests -- --nocapture
FM1_COMPILER_ONLY=true ./Android/test.sh
```

The device compiler case compares compiled execution against the interpreter and
exercises package import, state isolation, restart, reset and failed-import recovery.
It requires an installed APK from the current build and a dedicated arm64 emulator.
