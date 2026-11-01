#!/usr/bin/env bash
# Cross-builds the Windows app from Linux with cargo-xwin into a portable
# folder, dist/bowerbird-windows: Bowerbird.exe, DirectML.dll (onnxruntime's
# GPU backend, loaded at run time), pdfium\pdfium.dll, and the licenses.
# Needs cargo-xwin, clang, lld and llvm.
set -euo pipefail

BUILD=7881
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET=x86_64-pc-windows-msvc
export CARGO_TARGET_DIR="$ROOT/target/xwin"
# The Windows SDK and C runtime this build is known to link with. xwin
# otherwise takes the newest in Microsoft's manifest, so a release there
# could change the build.
export XWIN_SDK_VERSION="${XWIN_SDK_VERSION:-10.0.26100}"
export XWIN_CRT_VERSION="${XWIN_CRT_VERSION:-14.44.17.14}"
OUT="$ROOT/dist/bowerbird-windows"

# tauri.conf.json lists vendor/pdfium/lib as a resource, which must exist to build.
[ -d "$ROOT/vendor/pdfium/lib" ] || "$ROOT/scripts/fetch-pdfium.sh"

cd "$ROOT"
# onnxruntime links DirectML.lib and PathCch.lib by those names, but xwin
# only writes lower- and upper-case copies, and Linux paths are case-sensitive.
cargo xwin env --target "$TARGET" > /dev/null
SDK_LIB="${XDG_CACHE_HOME:-$HOME/.cache}/cargo-xwin/xwin/sdk/lib/um/x86_64"
for lib in DirectML PathCch; do
  ln -sf "$(echo "$lib" | tr '[:upper:]' '[:lower:]').lib" "$SDK_LIB/$lib.lib"
done

ui/node_modules/.bin/tauri build --config crates/desktop/tauri.conf.json \
  --runner cargo-xwin --target "$TARGET" --no-bundle "$@"

rm -rf "$OUT"
mkdir -p "$OUT/pdfium"
cp "$CARGO_TARGET_DIR/$TARGET/release/bowerbird-desktop.exe" "$OUT/Bowerbird.exe"
# ort downloads onnxruntime per target into its cache; DirectML.dll ships beside it.
DML="$(find "${XDG_CACHE_HOME:-$HOME/.cache}" -path "*/dfbin/$TARGET/*" -name DirectML.dll | head -1)"
[ -n "$DML" ] || { echo "DirectML.dll not found in the ort cache" >&2; exit 1; }
cp "$DML" "$OUT/"
TMP="$(mktemp -d)"
curl -fsSL "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F${BUILD}/pdfium-win-x64.tgz" \
  | tar xz -C "$TMP"
cp "$TMP/bin/pdfium.dll" "$TMP/LICENSE" "$OUT/pdfium/"
cp -r "$TMP/licenses" "$OUT/pdfium/"
rm -rf "$TMP"
cp -r LICENSE THIRD_PARTY_NOTICES.md licenses "$OUT/"
echo "Windows build in $OUT"
