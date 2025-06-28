#!/usr/bin/env bash
# Native ngspice of the pinned version -> dist/native/bin/ngspice (ngspice.exe on Windows).
# Used by the simulation tests now and by the sim_runner image later.
#
# Linux/macOS: built from the pinned source tarball with the same feature set as the WASM
# build (build-wasm.sh), so both engines run identical code paths.
# Windows (Git Bash/MSYS): unpacks the official build of the same release instead (needs
# py7zr in the repo .venv); it is for local development only, never deployed.
set -euo pipefail
shopt -s nullglob
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=pin.env
. "$here/pin.env"
dist="$here/dist/native"
key="$NGSPICE_VERSION-$(cat "$0" "$here"/patches/*.patch | sha256sum | cut -c1-16)"

if [ -f "$dist/.key" ] && [ "$(cat "$dist/.key")" = "$key" ]; then
  echo "$dist/bin"
  exit 0
fi
rm -rf "$dist" "$here/build/native"
mkdir -p "$dist" "$here/build/native"

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    archive="$(bash "$here/fetch.sh" win64)"
    "$here/../../.venv/Scripts/python.exe" -c \
      "import py7zr, sys; py7zr.SevenZipFile(sys.argv[1]).extractall(sys.argv[2])" \
      "$archive" "$here/build/native"
    cp -r "$here/build/native/Spice64"/. "$dist/"
    cp "$dist/bin/ngspice_con.exe" "$dist/bin/ngspice.exe"   # console build under the usual name
    ;;
  *)
    tarball="$(bash "$here/fetch.sh" src)"
    tar -xzf "$tarball" -C "$here/build/native"
    cd "$here/build/native/ngspice-$NGSPICE_VERSION"
    for p in "$here"/patches/*.patch; do patch -p1 -s < "$p"; done
    log="$here/build/native/build.log"
    { ./configure --prefix="$dist" --disable-xspice --disable-osdi --disable-openmp --disable-klu \
        --with-readline=no --with-fftw3=no --without-x --disable-dependency-tracking CFLAGS="-O2" &&
      make -j"$(nproc 2>/dev/null || sysctl -n hw.ncpu)" &&
      make install; } > "$log" 2>&1 || { grep -E 'error|Error' "$log" | head -20; echo "see $log" >&2; exit 1; }
    ;;
esac

echo "$key" > "$dist/.key"
echo "$dist/bin"
