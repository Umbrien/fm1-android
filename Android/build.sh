#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
export GRADLE_USER_HOME="${GRADLE_USER_HOME:-$ROOT/.tools/gradle-home}"
export ANDROID_USER_HOME="${ANDROID_USER_HOME:-$PWD/.tools/android-user}"
mkdir -p "$ANDROID_USER_HOME"
# The deprecated preference-root variable must not conflict with ANDROID_USER_HOME.
unset ANDROID_SDK_HOME
SDK="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
NDK="${ANDROID_NDK_HOME:-$SDK/ndk/27.2.12479018}"
TOOLS="$SDK/build-tools/35.0.0"
OUT="$ROOT/Android/build"
NDK_HOST="$(python3 - "$NDK" <<'PYHOST'
from pathlib import Path
import sys
hosts=list((Path(sys.argv[1])/"toolchains/llvm/prebuilt").glob("*"))
if len(hosts)!=1:sys.exit("Expected exactly one installed NDK host toolchain")
print(hosts[0])
PYHOST
)"
CLANG="$NDK_HOST/bin/aarch64-linux-android26-clang"
mkdir -p "$OUT/assets/engines" "$OUT/lib/arm64-v8a"
if [[ "${1:-}" == "--core" ]]; then
 export ANDROID_NDK_HOME="$NDK"
 export FM1_NATIVE_CLIPPER="$ROOT/Research/NativeClipper/upstream-clipper"
 export FM1_NATIVE_SLOOP="$ROOT/Research/NativeSloop/upstream-sloop-2.8"
 export FM1_NATIVE_ADDITIONS="$ROOT/Research/NativeAdditions"
 export FM1_NATIVE_FAMILY="$ROOT/Research/NativeFamily"
 export FM1_NATIVE_OFFICIAL_PREVIEW="$ROOT/Research/NativeOfficial/upstream-fm1-open"
 export FM1_NATIVE_SKIFF="$ROOT/Research/NativeSkiff/upstream-skiff"
 export FM1_NATIVE_EXPANSION="$ROOT/Research/NativeExpansion"
 cargo build -p fm1-ios-core --release --target aarch64-linux-android
fi
"$CLANG" -std=c11 -O2 -fPIC -shared -Wl,-z,max-page-size=16384 -Wl,--no-undefined -I"$ROOT/Android/app/src/main/cpp" "$ROOT/Android/app/src/main/cpp/bridge.c" "$ROOT/target/aarch64-linux-android/release/libfm1_ios_core.a" -lm -ldl -llog -o "$OUT/lib/arm64-v8a/libfm1android.so"
python3 - <<'PY'
import pathlib,shutil,json,hashlib
root=pathlib.Path.cwd()
for item in json.loads((root/'Core/native/packages.lock.json').read_text())['packages']:
 path=root/item['path'];data=path.read_bytes()
 if len(data)!=item['size'] or hashlib.sha256(data).hexdigest()!=item['sha256']:
  raise SystemExit('Exact firmware package check failed: '+item['id'])
 shutil.copy2(path,root/f"Android/build/assets/engines/{item['kind']}.fwsc")
notices=root/'Android/build/assets/notices';notices.mkdir(exist_ok=True)
for p in (root/'licenses').glob('*'):
 if p.is_file():shutil.copy2(p,notices/p.name)
shutil.copy2(root/'LICENSE',notices/'LICENSE');shutil.copy2(root/'NOTICE',notices/'NOTICE')
PY
# Gradle merges Material/AndroidX resources and compiles the Kotlin shell.
"$ROOT/Android/gradlew" -p "$ROOT/Android" --project-cache-dir "$ROOT/Android/.gradle" --console=plain assembleDebug
"$ROOT/Android/signing.sh" "$ROOT/Android/app/build/outputs/apk/debug/app-debug-unsigned.apk" "$OUT/FM1-android-arm64.apk"
"$TOOLS/apksigner" verify --verbose "$OUT/FM1-android-arm64.apk"
echo "Built: $OUT/FM1-android-arm64.apk"
