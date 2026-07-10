#!/usr/bin/env bash
# Stage the AppLoad bundle in dist/paperterm, ready for remagic publish.
set -euo pipefail
cd "$(dirname "$0")"

BIN=target/aarch64-unknown-linux-gnu/release/paperterm
[ -f "$BIN" ] || {
  echo "build first: cargo build --release --target aarch64-unknown-linux-gnu" >&2
  exit 1
}

rm -rf dist/paperterm
mkdir -p dist/paperterm
install -m 755 "$BIN" dist/paperterm/paperterm
install -m 644 external.manifest.json icon.png dist/paperterm/
install -m 644 LICENSE fonts/LICENSE.spleen dist/paperterm/

echo "staged: $(du -sh dist/paperterm | cut -f1) in dist/paperterm/"
echo "install with: remagic install ./dist/paperterm"
