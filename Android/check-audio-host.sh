#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/kotlin-toolchain.sh
OUT="$PWD/Android/build/audio-host-checks.jar"
mkdir -p "$(dirname "$OUT")"
"$KOTLIN_HOME/bin/kotlinc" \
 Android/app/src/main/kotlin/org/fm1/android/AudioTiming.kt \
 Android/app/src/main/kotlin/org/fm1/android/PcmWriteCursor.kt \
 Android/app/src/main/kotlin/org/fm1/android/PcmPlaybackDeadline.kt \
 Android/app/src/main/kotlin/org/fm1/android/OrderedPersistence.kt \
 Android/host-tests/AudioHostChecks.kt -include-runtime -d "$OUT"
java -jar "$OUT"
