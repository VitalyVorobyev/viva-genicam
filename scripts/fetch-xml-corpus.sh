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
# The scanCONTROL 850050 (a laser profile scanner) came without a bug report:
# it was asked for while merging the contributor's unrelated PFNC patch (#93).
# Its four <Register> nodes, FileAccessBuffer among them (the node #70's JAI
# also skipped), are evidence for backlog GA-09 and are why the same contributor
# asked for NodeMap::register_address in #92.
#
# The Manta G-125B at firmware 00.01.44.28514 is a newer revision of the
# documents fetched above; it was read from the maintainer's own camera with
# `viva-camctl xml` while testing the hardware plan (#215).
#
# The TRT009S is an event camera (Sony IMX636): its document has no
# PixelFormat node at all. It showed that a `<pLength>` on an `<IntReg>` was
# read past, so its event frame IDs, 2 or 8 bytes as the extended-ID bit says,
# were read as 4 (#138).
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
AVT_Manta_G125B_1.44.28514.xml	https://gist.githubusercontent.com/VitalyVorobyev/a458f692ab7944f3fb3b4684efab8453/raw/46c5189a402852361022c34580a9ad67de820070/AVT_Manta_G125B_1.44.28514.xml	215
Lucid_TRT009S.xml	https://github.com/user-attachments/files/34187527/trt009s.txt	138
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
#
# The first four FLIR documents are stand-ins attached to #45. The fifth,
# BFS-PGE-31S4C-C, is the #45 reporter's own camera, dumped with
# `viva-camctl xml` after 0.3.0 fixed the defect that made it unopenable.
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

# Same, but the attachment is a `viva-camctl report`. Every report embeds the
# camera's XML between its `## GenApi XML` and `## End of report` headings, so
# a report is enough: nobody has to send the document separately.
#
# The a2A4096-44g5m read `TriggerMode` from bootstrap register 0x000C because
# an `<Integer>`'s `<pIndex>` was taken as an address term (#210).
#   name<TAB>url<TAB>issue
USER_CONTRIBUTED_REPORT=$(
  cat <<'EOF'
Basler_a2A4096_44g5m.xml	https://github.com/user-attachments/files/33539563/viva-report.txt	210
EOF
)

echo "  user-contributed (viva-camctl reports)"
while IFS=$'\t' read -r name url issue; do
  [ -z "$name" ] && continue
  # Between the headings, without the blank lines around the document.
  if curl -fsSL -o "$tmp/report.txt" "$url" &&
    awk '/^## End of report/ { exit } found { print } /^## GenApi XML/ { found = 1 }' \
      "$tmp/report.txt" | sed -e '/./,$!d' | sed -e ':a' -e '/^\n*$/{$d;N;ba' -e '}' \
      >"$tmp/$name" &&
    [ -s "$tmp/$name" ] &&
    mv "$tmp/$name" "$TARGET/$name"; then
    echo "    $name (#$issue)"
  else
    echo "    warning: could not fetch $name (#$issue)" >&2
  fi
done <<<"$USER_CONTRIBUTED_REPORT"

count=$(find "$TARGET" -name '*.xml' | wc -l | tr -d ' ')
echo "Done: $count XML documents in $TARGET"
