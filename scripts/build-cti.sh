#!/usr/bin/env bash
#
# Build the GenTL producer and name it as GenTL consumers look for it.
#
# A GenTL producer is a shared library with the extension .cti (GenTL 1.6
# §6.1): this builds the `viva-gentl` cdylib in release mode and copies it
# to `<out-dir>/viva_gentl.cti`. Point a consumer at the directory with
# GENICAM_GENTL64_PATH, or load the file directly.
#
# Usage:
#   scripts/build-cti.sh [out-dir] [cargo build options...]
#
#   scripts/build-cti.sh                       # -> target/gentl/viva_gentl.cti
#   scripts/build-cti.sh dist --target x86_64-pc-windows-msvc
#
# Prints the path of the .cti.

set -euo pipefail

OUT="${1:-target/gentl}"
shift || true

TARGET=""
args=("$@")
for ((i = 0; i < ${#args[@]}; i++)); do
  case "${args[$i]}" in
    --target) TARGET="${args[$((i + 1))]}" ;;
    --target=*) TARGET="${args[$i]#--target=}" ;;
  esac
done

cargo build --release --locked -p viva-gentl "$@" >&2

DIR="target/release"
[ -n "$TARGET" ] && DIR="target/${TARGET}/release"

case "${TARGET:-$(rustc -vV | sed -n 's/^host: //p')}" in
  *windows*) LIB="${DIR}/viva_gentl.dll" ;;
  *apple*) LIB="${DIR}/libviva_gentl.dylib" ;;
  *) LIB="${DIR}/libviva_gentl.so" ;;
esac

if [ ! -f "$LIB" ]; then
  echo "error: ${LIB} was not built" >&2
  exit 1
fi

mkdir -p "$OUT"
cp "$LIB" "${OUT}/viva_gentl.cti"
echo "${OUT}/viva_gentl.cti"
