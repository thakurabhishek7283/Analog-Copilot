#!/usr/bin/env bash
# Download a pinned ngspice artifact into .cache/ and verify its sha256.
# usage: fetch.sh [src|win64]   (default: src). Prints the archive path.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=pin.env
. "$here/pin.env"

case "${1:-src}" in
  src)   file=$NGSPICE_SRC;   sha=$NGSPICE_SRC_SHA256 ;;
  win64) file=$NGSPICE_WIN64; sha=$NGSPICE_WIN64_SHA256 ;;
  *) echo "usage: $0 [src|win64]" >&2; exit 2 ;;
esac

mkdir -p "$here/.cache"
out="$here/.cache/$file"
if [ ! -f "$out" ] || ! echo "$sha  $out" | sha256sum -c --status; then
  echo "fetching $file" >&2
  curl -sSfL --retry 3 -o "$out.part" "$NGSPICE_BASE_URL/$file"
  mv "$out.part" "$out"
fi
if ! echo "$sha  $out" | sha256sum -c --status; then
  echo "sha256 mismatch for $file (expected $sha)" >&2
  rm -f "$out"
  exit 1
fi
echo "$out"
