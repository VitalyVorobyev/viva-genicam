#!/usr/bin/env bash
#
# Drive viva_gentl.cti from C against viva-fake-gige: build the .cti, compile
# the harness against the EMVA's GenTL.h, start the fake, list, open, read
# every port's XML, acquire frames and check their pixels — then validate
# the XMLs the ports served against the GenApi 1.1 schema, and, with
# HARVESTERS_PYTHON set, acquire again through Harvesters
# (scripts/gentl-harvesters.py), a consumer we did not write.
#
# Usage:
#   scripts/gentl-harness.sh [frames]        # default 10
#   HARVESTERS_PYTHON=.venv/bin/python scripts/gentl-harness.sh
#
# Needs a C compiler (cc), curl and unzip, and xmllint for the schema check;
# a step whose tool is missing is skipped, and says so — except under CI
# (CI=true, as GitHub Actions sets), where it fails. HARVESTERS_PYTHON is a
# Python with harvesters and numpy installed. GENTL_EMVA_DIR is where the
# EMVA's header and schema are kept (default target/gentl/emva). UDP 3956
# must be free: the fake binds it. POSIX only; CI runs it on Linux.

set -euo pipefail

FRAMES="${1:-10}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

WORK="target/gentl/harness"
mkdir -p "$WORK/xml"

CTI="$(scripts/build-cti.sh target/gentl)"
EMVA="$(scripts/fetch-gentl-header.sh "${GENTL_EMVA_DIR:-target/gentl/emva}")"

# A step that cannot run is skipped locally and fails under CI.
skipped() {
  echo "$1" >&2
  if [ "${CI:-}" = "true" ]; then
    exit 1
  fi
}

cc -std=c99 -Wall -Wextra -Werror -I"$EMVA" \
  crates/viva-gentl/harness/gentl_harness.c -o "$WORK/gentl_harness" -ldl

cargo build --release --locked -p viva-fake-gige >&2
target/release/viva-fake-gige --width 64 --height 48 --pixel-format mono16 --fps 30 \
  >"$WORK/fake.log" 2>&1 &
FAKE=$!
trap 'kill "$FAKE" 2>/dev/null || true' EXIT
sleep 1

# The fake's AcquisitionStart and AcquisitionStop registers
# (viva_fake_gige::REG_ACQ_START / REG_ACQ_STOP).
"$WORK/gentl_harness" "$CTI" "$WORK/xml" "$FRAMES" 0x20024 0x20028

if command -v xmllint >/dev/null 2>&1; then
  for xml in "$WORK"/xml/*.xml; do
    xmllint --noout --schema "$EMVA/GenApiSchema_Version_1_1.xsd" "$xml"
  done
else
  skipped "xmllint not found: the XMLs were not validated against the schema"
fi

if [ -n "${HARVESTERS_PYTHON:-}" ]; then
  "$HARVESTERS_PYTHON" -I scripts/gentl-harvesters.py "$CTI" "$FRAMES"
else
  skipped "HARVESTERS_PYTHON not set: Harvesters was not run"
fi
