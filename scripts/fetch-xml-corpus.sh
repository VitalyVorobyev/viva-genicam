#!/usr/bin/env bash
#
# Fetch a corpus of real-camera GenICam XML descriptions for conformance testing.
#
# The corpus is NOT committed. These documents are vendor copyright, published
# for interoperability by third-party projects; we fetch them on demand rather
# than redistribute them. The download directory is gitignored.
#
# Usage:
#   scripts/fetch-xml-corpus.sh [target-dir]
#
# Then:
#   cargo test -p viva-genapi-xml --test vendor_corpus -- --nocapture
#
# The corpus test is a no-op when the directory is absent, so CI and a fresh
# clone stay green without it.

set -euo pipefail

TARGET="${1:-fixtures/vendor-xml}"

# Real device descriptions collected by the EPICS areaDetector project, covering
# AVT, Basler, Baumer, FLIR, JAI, PCO, Point Grey, Photonic Science, Prosilica,
# SVS, Sony and The Imaging Source. Point Grey / FLIR entries matter most: their
# CDATA-wrapped formulas are what broke issue #45.
AREADETECTOR_REPO="areaDetector/aravisGigE"
AREADETECTOR_PATH="etc/genicam"

# The GenICam standard's own conformance document plus two aravis device
# descriptions. Constant-formula SwissKnives and the `R` access mode live here.
ARAVIS_REPO="AravisProject/aravis"
ARAVIS_FILES=(
  "tests/data/genicam.xml"
  "src/arv-fake-camera.xml"
  "src/arv-v4l2.xml"
)

if ! command -v gh >/dev/null 2>&1; then
  echo "error: this script needs the GitHub CLI (gh) on PATH" >&2
  exit 1
fi

if ! command -v unzip >/dev/null 2>&1; then
  echo "error: this script needs unzip on PATH (vendors ship zipped XML)" >&2
  exit 1
fi

mkdir -p "$TARGET"
echo "Fetching GenICam XML corpus into $TARGET"

echo "  $AREADETECTOR_REPO/$AREADETECTOR_PATH"
gh api "repos/$AREADETECTOR_REPO/contents/$AREADETECTOR_PATH" \
  --jq '.[] | select(.name | endswith(".xml")) | "\(.name)\t\(.download_url)"' |
  while IFS=$'\t' read -r name url; do
    curl -fsSL -o "$TARGET/$name" "$url"
    echo "    $name"
  done

echo "  $ARAVIS_REPO"
for path in "${ARAVIS_FILES[@]}"; do
  name="aravis_$(basename "$path")"
  url="https://raw.githubusercontent.com/$ARAVIS_REPO/main/$path"
  curl -fsSL -o "$TARGET/$name" "$url"
  echo "    $name"
done

# Documents contributed by users on the issue tracker, hosted as GitHub issue
# attachments or as a gist. Each one is a camera we could not open until it was
# reported, or a device class we had never seen; keeping it here is what stops
# that bug from coming back.
#
# The DMK 33GP2000e is also the only document in the corpus that begins with a
# UTF-8 byte-order mark (#122). Every other one starts at `<`, which is why a
# BOM went unnoticed until a user hit it.
#
# Gist URLs are pinned to a revision SHA. The bare `/raw` form follows the
# latest revision, so an edit upstream would silently change what the corpus
# tests run against.
#   name<TAB>url<TAB>issue
USER_CONTRIBUTED=$(
  cat <<'EOF'
Hikrobot_MV-CS050-10GC.xml	https://github.com/user-attachments/files/30513169/xml.raw.xml	35
MicroEpsilon_scanCONTROL_850050.xml	https://gist.githubusercontent.com/Katze719/f1116fda94ff1f424fc7bf5955c86952/raw/9e3153952c8fab3b64e60268c4a6119fcc70f39d/scancontrol-device.xml	93
TIS_DMK_33GP2000e.xml	https://github.com/user-attachments/files/31440543/camera_dmk_33GP2000e.xml	122
EOF
)

echo "  user-contributed (issue attachments)"
while IFS=$'\t' read -r name url issue; do
  [ -z "$name" ] && continue
  if curl -fsSL -o "$TARGET/$name" "$url"; then
    echo "    $name (#$issue)"
  else
    echo "    warning: could not fetch $name (#$issue)" >&2
  fi
done <<<"$USER_CONTRIBUTED"

# Same, but the vendor tool exports the description as a ZIP. The member name is
# a firmware build number and carries no model information, so we rename on
# extraction.
#   name<TAB>url<TAB>member<TAB>issue
USER_CONTRIBUTED_ZIP=$(
  cat <<'EOF'
FLIR_BFLY_PGE_13E4C.xml	https://github.com/user-attachments/files/30514270/Blackfly.BFLY-PGE-13E4C_19104035_GenICam.zip	GRS_GEV_v003_380122.xml	45
FLIR_BFLY_PGE_31S4C.xml	https://github.com/user-attachments/files/30514276/Blackfly.BFLY-PGE-31S4C_20274171_GenICam.zip	GRS_GEV_v003_292525.xml	45
FLIR_BFS_PGE_63S4C.xml	https://github.com/user-attachments/files/30514275/BFS-PGE-63S4C_0188B042_GENICAM.zip	public_camxml.xml	45
FLIR_BFS_PGE_70S7C.xml	https://github.com/user-attachments/files/30514267/BFS-PGE-70S7C_0188D99D_GENICAM.zip	public_camxml.xml	45
FLIR_BFS_PGE_31S4C_C.xml	https://github.com/user-attachments/files/30541856/bfs-pge-31s4c.xml.zip	bfs-pge-31s4c.xml	45
EOF
)

echo "  user-contributed (zipped issue attachments)"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
while IFS=$'\t' read -r name url member issue; do
  [ -z "$name" ] && continue
  if curl -fsSL -o "$tmp/archive.zip" "$url" &&
    unzip -o -q -j "$tmp/archive.zip" "$member" -d "$tmp" &&
    mv "$tmp/$member" "$TARGET/$name"; then
    echo "    $name (#$issue)"
  else
    echo "    warning: could not fetch $name (#$issue)" >&2
  fi
done <<<"$USER_CONTRIBUTED_ZIP"

count=$(find "$TARGET" -name '*.xml' | wc -l | tr -d ' ')
echo "Done: $count XML documents in $TARGET"
