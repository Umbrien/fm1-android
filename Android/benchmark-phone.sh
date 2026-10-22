#!/bin/bash
# Build and measure only the isolated benchmark package on an explicitly selected phone.
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ -z "${ANDROID_HOME:-}" && -f .tools/env.sh ]]; then
 source .tools/env.sh
fi
export GRADLE_USER_HOME="${GRADLE_USER_HOME:-$PWD/.tools/gradle-home}"
export ANDROID_USER_HOME="${ANDROID_USER_HOME:-$PWD/.tools/android-user}"
unset ANDROID_SDK_HOME
exec python3 Android/benchmark_phone.py "$@"
