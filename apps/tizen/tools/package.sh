#!/usr/bin/env bash
# Build and sign the Tizen web app with a per-TV Samsung certificate profile.
#
#   PROFILE=my-tv-cert apps/tizen/tools/package.sh /tmp/peregrine-tv
#
# The output directory must not contain a space: the install step passes its .wgt path to
# `tizen install -n`. The profile holds the author certificate and the distributor certificate
# tied to one TV's DUID; neither is in this repository.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${1:-$ROOT/build}"
PROFILE="${PROFILE:-}"
TIZEN="${TIZEN:-$HOME/tizen-studio/tools/ide/bin/tizen}"

if [ -z "$PROFILE" ]; then
  echo "Set PROFILE to a Samsung certificate profile (tizen security-profiles list)." >&2
  exit 1
fi
if [ ! -x "$TIZEN" ]; then
  TIZEN="$(command -v tizen || true)"
fi
[ -n "$TIZEN" ] || { echo "tizen CLI not found; install Tizen Studio with the TV extension." >&2; exit 1; }
case "$OUT" in *" "*) echo "the output path must not contain a space: $OUT" >&2; exit 1;; esac

mkdir -p "$OUT"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

# Only the runtime files are packaged; the tests and this README stay in the repository.
cp "$ROOT/config.xml" "$ROOT/index.html" "$ROOT/main.js" "$ROOT/config.js" \
   "$ROOT/server-url.js" "$ROOT/style.css" "$STAGE/"
cp -R "$ROOT/icons" "$STAGE/icons"

"$TIZEN" package -t wgt -s "$PROFILE" -o "$OUT" -- "$STAGE"
echo "package: $OUT/Peregrine.wgt"
