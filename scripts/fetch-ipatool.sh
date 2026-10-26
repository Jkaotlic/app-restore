#!/usr/bin/env bash
# Fetch pinned ipatool for macOS sidecars. Idempotent. Usage: fetch-ipatool.sh [--dry-run]
set -euo pipefail
DRY=0; [ "${1:-}" = "--dry-run" ] && DRY=1
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LOCK="$ROOT/scripts/ipatool.lock"
OUT="$ROOT/src-tauri/binaries"
BASE="https://github.com/majd/ipatool/releases/download"

lock() { grep -E "^$1=" "$LOCK" | head -n1 | cut -d= -f2-; }
sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
VERSION="$(lock VERSION)"

# triple  lock-key
PAIRS="aarch64-apple-darwin:macos_arm64 x86_64-apple-darwin:macos_amd64"

ok_thin() { # file expected
  [ -f "$1" ] && [ "$(sha "$1")" = "$2" ]
}
ok_universal() {
  [ -f "$1" ] && archs="$(lipo -archs "$1" 2>/dev/null)" && [[ " $archs " == *" arm64 "* && " $archs " == *" x86_64 "* ]]
}

need=0
for p in $PAIRS; do
  t="${p%%:*}"; k="${p##*:}"
  ok_thin "$OUT/ipatool-$t" "$(lock "BIN_${k}_SHA256")" || need=1
done
ok_universal "$OUT/ipatool-universal-apple-darwin" || need=1
if [ "$need" = 0 ]; then echo "up to date"; exit 0; fi
if [ "$DRY" = 1 ]; then echo "dry-run: would download ipatool $VERSION into $OUT"; exit 0; fi

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
mkdir -p "$OUT"
for p in $PAIRS; do
  t="${p%%:*}"; k="${p##*:}"
  name="$(lock "ARCHIVE_${k}_NAME")"
  mkdir -p "$TMP/$k"
  curl -fsSL "$BASE/v$VERSION/$name" -o "$TMP/$k/$name"
  [ "$(sha "$TMP/$k/$name")" = "$(lock "ARCHIVE_${k}_SHA256")" ] || { echo "sha256 mismatch: $name" >&2; exit 1; }
  tar -xzf "$TMP/$k/$name" -C "$TMP/$k"
  bin="$TMP/$k/$(lock "BIN_${k}_PATH")"
  [ "$(sha "$bin")" = "$(lock "BIN_${k}_SHA256")" ] || { echo "sha256 mismatch: binary of $name" >&2; exit 1; }
  install -m 0755 "$bin" "$OUT/ipatool-$t"
done
lipo -create "$OUT/ipatool-aarch64-apple-darwin" "$OUT/ipatool-x86_64-apple-darwin" -output "$OUT/ipatool-universal-apple-darwin"
chmod 0755 "$OUT/ipatool-universal-apple-darwin"
echo "ipatool $VERSION installed into $OUT"
