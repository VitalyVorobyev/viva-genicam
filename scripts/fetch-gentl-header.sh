#!/usr/bin/env bash
#
# Fetch the EMVA's GenTL.h and GenApi 1.1 schema for the GenTL harnesses.
#
# Neither is committed: GenTL.h is published "under the license of the EMVA
# GenICam Standard Group". These are test-consumer inputs, not MIT project
# sources or distribution assets; their original terms still apply (see
# docs/legal-review.md and ADR-0031). Both come from the EMVA package, checked against the
# SHA-256 the EMVA publishes on its download page.
#
# Usage:
#   scripts/fetch-gentl-header.sh [out-dir]       # default target/gentl/emva
#
# GENICAM_PACKAGE_ZIP=/path/to/GenICam_Package_2026.07.zip reuses a download.
# Writes <out-dir>/GenTL.h and <out-dir>/GenApiSchema_Version_1_1.xsd.

set -euo pipefail

OUT="${1:-target/gentl/emva}"
PACKAGE_URL="https://www.emva.org/wp-content/uploads/GenICam_Package_2026.07.zip"
PACKAGE_SHA256="e74b8d982c3e7697399b03c254b6e8c596eddf0279959eba7de18734e279b64f"

if [ -f "${OUT}/GenTL.h" ] && [ -f "${OUT}/GenApiSchema_Version_1_1.xsd" ]; then
  echo "${OUT}"
  exit 0
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

ZIP="${GENICAM_PACKAGE_ZIP:-}"
if [ -z "$ZIP" ]; then
  ZIP="${WORK}/package.zip"
  echo "downloading ${PACKAGE_URL} (about 135 MB)" >&2
  curl -fsSL --retry 3 -o "$ZIP" "$PACKAGE_URL"
fi

if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL="$(sha256sum "$ZIP" | cut -d' ' -f1)"
else
  ACTUAL="$(shasum -a 256 "$ZIP" | cut -d' ' -f1)"
fi
if [ "$ACTUAL" != "$PACKAGE_SHA256" ]; then
  echo "error: ${ZIP} has SHA-256 ${ACTUAL}, expected ${PACKAGE_SHA256}" >&2
  exit 1
fi

unzip -q -o "$ZIP" "GenTL/GenTL.h" "GenApi/GenApiSchema_Version_1_1.xsd_.zip" -d "$WORK"
unzip -q -o "${WORK}/GenApi/GenApiSchema_Version_1_1.xsd_.zip" -d "${WORK}/GenApi"

mkdir -p "$OUT"
cp "${WORK}/GenTL/GenTL.h" "${OUT}/GenTL.h"
cp "${WORK}/GenApi/GenApiSchema_Version_1_1.xsd" "${OUT}/GenApiSchema_Version_1_1.xsd"
echo "${OUT}"
