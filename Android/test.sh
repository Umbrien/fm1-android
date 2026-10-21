#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
export GRADLE_USER_HOME="${GRADLE_USER_HOME:-$PWD/.tools/gradle-home}"
export ANDROID_USER_HOME="${ANDROID_USER_HOME:-$PWD/.tools/android-user}"
mkdir -p "$ANDROID_USER_HOME"
# The deprecated preference-root variable must not conflict with ANDROID_USER_HOME.
unset ANDROID_SDK_HOME
SDK="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
OUT="$PWD/Android/build/test"
mkdir -p "$OUT"
rm -rf "$OUT/assets"
mkdir -p "$OUT/assets"
EXCHANGE=false
if [[ -n "${FM1_EXCHANGE_FIXTURES:-}" ]]; then
 mkdir -p "$OUT/assets/exchange"
 for name in swift-generated.fm1project swift-generated.fm1backup ios-simulator.fm1project; do
  cp "$FM1_EXCHANGE_FIXTURES/$name" "$OUT/assets/exchange/$name"
 done
 EXCHANGE=true
fi
"$PWD/Android/gradlew" -p "$PWD/Android" --console=plain assembleDebugAndroidTest
"$PWD/Android/signing.sh" "$PWD/Android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk" "$OUT/tests.apk"
"$SDK/platform-tools/adb" shell input keyevent KEYCODE_WAKEUP
"$SDK/platform-tools/adb" shell wm dismiss-keyguard
"$SDK/platform-tools/adb" install -r "$OUT/tests.apk"
"$SDK/platform-tools/adb" shell am instrument -w -e exchange "$EXCHANGE" -e uiOnly "${FM1_UI_ONLY:-false}" org.fm1.android.tests/org.fm1.android.EngineInstrumentation | tee "$OUT/result.txt"
python3 - "$OUT/result.txt" <<'PYRESULT'
import json,pathlib,re,sys
text=pathlib.Path(sys.argv[1]).read_text();matches=list(re.finditer(r'\{\s*"platform"\s*:',text))
if not matches:sys.exit("No completed instrumentation receipt")
receipt=json.JSONDecoder().raw_decode(text[matches[-1].start():])[0]
if receipt.get("failures")!=0:sys.exit("Android instrumentation failed")
PYRESULT
