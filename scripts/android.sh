#!/usr/bin/env bash
# Builds the Android app (64-bit ARM; onnxruntime ships no other Android
# build) as an APK signed with the debug key. Needs the Android SDK and NDK:
# ANDROID_HOME and NDK_HOME, or ~/Android/env.sh to set them.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[ -f "$HOME/Android/env.sh" ] && . "$HOME/Android/env.sh"
export NDK_HOME="${NDK_HOME:-$(ls -d "$ANDROID_HOME"/ndk/* | sort -V | tail -1)}"
SYSROOT="$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/sysroot"

# whisper.cpp is built with CMake and bound with bindgen; both need the NDK's
# headers and toolchain rather than the host's.
export BINDGEN_EXTRA_CLANG_ARGS_aarch64_linux_android="--sysroot=$SYSROOT --target=aarch64-linux-android24"
export CMAKE_TOOLCHAIN_FILE_aarch64_linux_android="$ROOT/scripts/android-arm64.toolchain.cmake"

[ -f "$ROOT/crates/desktop/gen/android/app/src/main/jniLibs/arm64-v8a/libpdfium.so" ] \
  || "$ROOT/scripts/fetch-pdfium.sh" android

cd "$ROOT"
ui/node_modules/.bin/tauri android build --target aarch64 --apk --ci "$@"
