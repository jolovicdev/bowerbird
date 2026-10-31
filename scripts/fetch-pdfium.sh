#!/usr/bin/env bash
# Downloads the prebuilt PDFium library into vendor/pdfium, or with
# `android`, into the Android project.
# The build number must match the pdfium_* feature that pdfium-render enables
# (pdfium-render 0.9.4 defaults to pdfium_7881).
set -euo pipefail

BUILD=7881
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# `fetch-pdfium.sh android`: the library for 64-bit ARM phones, placed where
# the Android build packs it beside the app's own native library.
if [ "${1:-}" = android ]; then
  JNI="$ROOT/crates/desktop/gen/android/app/src/main/jniLibs/arm64-v8a"
  TMP="$(mktemp -d)"
  curl -fsSL "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F${BUILD}/pdfium-android-arm64.tgz" \
    | tar xz -C "$TMP"
  mkdir -p "$JNI"
  cp "$TMP/lib/libpdfium.so" "$JNI/"
  rm -rf "$TMP"
  echo "PDFium ${BUILD} (android-arm64) installed in ${JNI}"
  exit 0
fi

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) PLATFORM=linux-x64 ;;
  Linux-aarch64) PLATFORM=linux-arm64 ;;
  Darwin-arm64) PLATFORM=mac-arm64 ;;
  Darwin-x86_64) PLATFORM=mac-x64 ;;
  *) echo "No PDFium build mapped for $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

DEST="$ROOT/vendor/pdfium"
rm -rf "$DEST"
mkdir -p "$DEST"
curl -fsSL "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F${BUILD}/pdfium-${PLATFORM}.tgz" \
  | tar xz -C "$DEST"
echo "PDFium ${BUILD} (${PLATFORM}) installed in ${DEST}/lib"
