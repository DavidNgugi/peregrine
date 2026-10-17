#!/usr/bin/env bash
# Builds the LGPL ffmpeg and ffprobe macOS ships, because no redistributable build exists.
#
# Usage: build-ffmpeg-macos.sh [arm64|x86_64]
#
# The configure line is the recorded one from ADR 0025: no --enable-gpl, no --enable-nonfree,
# --enable-version3 accepted, and --disable-autodetect so a runner's Homebrew cannot leak into the
# binary. The tag and commit are pinned; a mismatch stops the release.
set -euo pipefail

VERSION="n9.0.2"
COMMIT="946fcce07b6dcd0331c8cc609192aeff5e1924f8"
ARCH="${1:-$(uname -m)}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE="${TMPDIR:-/tmp}/peregrine-ffmpeg-${VERSION}-${ARCH}"
OUT="${ROOT}/dist/sidecars/${ARCH}"

case "${ARCH}" in
  arm64)   MIN_MACOS="14.0"; CROSS=() ;;
  x86_64)  MIN_MACOS="13.0"; CROSS=(--arch=x86_64 --disable-x86asm --extra-cflags="-arch x86_64" --extra-ldflags="-arch x86_64") ;;
  *) echo "unsupported arch: ${ARCH}" >&2; exit 1 ;;
esac

rm -rf "${SOURCE}"
git clone --depth 1 --branch "${VERSION}" https://github.com/FFmpeg/FFmpeg "${SOURCE}"
HEAD="$(git -C "${SOURCE}" rev-parse HEAD)"
if [ "${HEAD}" != "${COMMIT}" ]; then
  echo "ffmpeg: ${VERSION} resolves to ${HEAD}, the pin says ${COMMIT}" >&2
  exit 1
fi

cd "${SOURCE}"
./configure \
  --prefix="${SOURCE}/out" \
  --disable-gpl --disable-nonfree --enable-version3 \
  --disable-doc --disable-debug --disable-shared --enable-static --disable-autodetect \
  --enable-videotoolbox --enable-audiotoolbox --enable-avfoundation \
  "${CROSS[@]+"${CROSS[@]}"}" \
  --extra-cflags="-mmacosx-version-min=${MIN_MACOS}" \
  --extra-ldflags="-mmacosx-version-min=${MIN_MACOS}"

make -j"$(sysctl -n hw.ncpu 2>/dev/null || echo 4)" ffmpeg ffprobe

mkdir -p "${OUT}"
install -m 0755 ffmpeg ffprobe "${OUT}/"
echo "ffmpeg: wrote ${OUT}/ffmpeg and ${OUT}/ffprobe (${VERSION}, ${COMMIT}, ${ARCH})"
