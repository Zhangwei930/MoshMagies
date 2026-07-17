#!/usr/bin/env bash
# Local packaging helper (mirrors CI release layout for MagiesTerminal).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${OUT_DIR:-$ROOT/out}"
mkdir -p "$OUT"
cd "$ROOT"
cargo build --release
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
cp target/release/mosh-client "$STAGE/mosh-client" 2>/dev/null \
  || cp target/release/mosh-client.exe "$STAGE/mosh-client.exe"
cd "$STAGE"
if [ -f mosh-client ]; then
  tar -czf "$OUT/mosh-client-local.tar.gz" mosh-client
else
  tar -czf "$OUT/mosh-client-local.tar.gz" mosh-client.exe
fi
(cd "$OUT" && shasum -a 256 mosh-client-local.tar.gz > mosh-client-local.tar.gz.sha256)
ls -la "$OUT"
