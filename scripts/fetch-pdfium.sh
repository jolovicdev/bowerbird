#!/usr/bin/env bash
# Downloads the prebuilt PDFium library into vendor/pdfium, or with
# `android`, into the Android project.
# The build number must match the pdfium_* feature that pdfium-render enables
# (pdfium-render 0.9.4 defaults to pdfium_7881).
set -euo pipefail

BUILD=7881
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Downloads PDFium for platform $1 and unpacks it into $2, once the archive
# matches the SHA-256 recorded here for this build.
fetch() {
  local sha256 archive sum
  case "$1" in
    android-arm64) sha256=16d23bb86c4188d59326dc509938f59df3c417ecfdd3d2ca2f160dc5bd49a839 ;;
    linux-x64) sha256=1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d ;;
    linux-arm64) sha256=ee7f7b7d5468958336a818c1cd580bdd20972846b7377b13f9a923d92d1d4674 ;;
    mac-arm64) sha256=52e94ca5aa8847934330daf3f8150c190682c5ca93831468794f8b90d4392e40 ;;
    mac-x64) sha256=6dedf83990e0e3d6b7c93c9e7589c5a126b0ae14b7464d76120cff7a26afb18b ;;
  esac
  archive="$(mktemp)"
  curl -fsSL -o "$archive" \
    "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F${BUILD}/pdfium-$1.tgz"
  # macOS has shasum but not always sha256sum; both check "hash  file" lines.
  if command -v sha256sum > /dev/null; then sum=(sha256sum); else sum=(shasum -a 256); fi
  echo "$sha256  $archive" | "${sum[@]}" --status -c || {
    echo "pdfium-$1.tgz doesn't match its recorded SHA-256" >&2
    exit 1
  }
  tar xzf "$archive" -C "$2"
  rm -f "$archive"
}

# `fetch-pdfium.sh android`: the library for 64-bit ARM phones, placed where
# the Android build packs it beside the app's own native library, and its
# licenses among the app's assets.
if [ "${1:-}" = android ]; then
  JNI="$ROOT/crates/desktop/gen/android/app/src/main/jniLibs/arm64-v8a"
  NOTICES="$ROOT/crates/desktop/gen/android/app/src/main/assets/notices/pdfium"
  TMP="$(mktemp -d)"
  fetch android-arm64 "$TMP"
  mkdir -p "$JNI" "$NOTICES"
  cp "$TMP/lib/libpdfium.so" "$JNI/"
  cp -r "$TMP/LICENSE" "$TMP/licenses" "$NOTICES/"
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
fetch "$PLATFORM" "$DEST"
echo "PDFium ${BUILD} (${PLATFORM}) installed in ${DEST}/lib"
