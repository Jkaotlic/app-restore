#!/usr/bin/env bash
# Build the pinned, patched ipatool for macOS sidecars from source. Idempotent.
# Usage: fetch-ipatool.sh [--dry-run]
# Needs git, go 1.21+ (GOTOOLCHAIN fetches the pinned one, checked by the Go
# checksum database) and Xcode command line tools (cgo, lipo). Module downloads are
# checked against ipatool's go.sum.
set -euo pipefail
DRY=0; [ "${1:-}" = "--dry-run" ] && DRY=1
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LOCK="$ROOT/scripts/ipatool.lock"
PATCHES="$ROOT/scripts/ipatool-patches"
OUT="$ROOT/src-tauri/binaries"

lock() { grep -E "^$1=" "$LOCK" | head -n1 | cut -d= -f2-; }
REPO="$(lock SOURCE_REPO)"
COMMIT="$(lock SOURCE_COMMIT)"
GO_VERSION="$(lock GO_VERSION)"
BUILD_VERSION="$(lock BUILD_VERSION)"

# Everything that defines the build; the stamp next to the binaries records it.
recipe() {
  printf '%s\n' "$REPO" "$COMMIT" "$GO_VERSION" "$BUILD_VERSION"
  cat "$0" "$PATCHES"/*.patch
}
want="$(recipe | shasum -a 256 | cut -d' ' -f1)"
STAMP="$OUT/.ipatool-build"

has_archs() { # file
  [ -f "$1" ] && archs="$(lipo -archs "$1" 2>/dev/null)" && [[ " $archs " == *" arm64 "* && " $archs " == *" x86_64 "* ]]
}
if has_archs "$OUT/ipatool-universal-apple-darwin" \
  && [ -f "$OUT/ipatool-aarch64-apple-darwin" ] && [ -f "$OUT/ipatool-x86_64-apple-darwin" ] \
  && [ "$(cat "$STAMP" 2>/dev/null)" = "$want" ]; then
  echo "up to date ($BUILD_VERSION)"; exit 0
fi
if [ "$DRY" = 1 ]; then echo "dry-run: would build ipatool $BUILD_VERSION ($COMMIT + patches) into $OUT"; exit 0; fi

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
SRC="$TMP/ipatool"
git init -q "$SRC"
git -C "$SRC" fetch -q --depth 1 "$REPO" "$COMMIT"
git -C "$SRC" -c advice.detachedHead=false checkout -q FETCH_HEAD
[ "$(git -C "$SRC" rev-parse HEAD)" = "$COMMIT" ] || { echo "commit mismatch: wanted $COMMIT" >&2; exit 1; }
for p in "$PATCHES"/*.patch; do
  git -C "$SRC" apply --whitespace=nowarn "$p" || { echo "patch does not apply: $(basename "$p")" >&2; exit 1; }
done

export GOTOOLCHAIN="go$GO_VERSION" GOFLAGS="-mod=readonly -trimpath"
export CGO_ENABLED=1 CGO_CFLAGS="-mmacosx-version-min=10.15" CGO_LDFLAGS="-mmacosx-version-min=10.15"
LDFLAGS="-s -w -X github.com/majd/ipatool/v2/cmd.version=$BUILD_VERSION"
mkdir -p "$OUT"
# triple  GOARCH
for p in aarch64-apple-darwin:arm64 x86_64-apple-darwin:amd64; do
  t="${p%%:*}"; a="${p##*:}"
  (cd "$SRC" && GOOS=darwin GOARCH="$a" go build -ldflags="$LDFLAGS" -o "$TMP/ipatool-$t" .)
  [ "$(lipo -archs "$TMP/ipatool-$t")" = "$([ "$a" = amd64 ] && echo x86_64 || echo arm64)" ] || { echo "wrong arch: $t" >&2; exit 1; }
done
# version check runs the native slice only
native="$TMP/ipatool-$([ "$(uname -m)" = arm64 ] && echo aarch64 || echo x86_64)-apple-darwin"
"$native" --version | grep -qF "$BUILD_VERSION" || { echo "built binary reports a wrong version" >&2; exit 1; }

for t in aarch64-apple-darwin x86_64-apple-darwin; do install -m 0755 "$TMP/ipatool-$t" "$OUT/ipatool-$t"; done
lipo -create "$OUT/ipatool-aarch64-apple-darwin" "$OUT/ipatool-x86_64-apple-darwin" -output "$OUT/ipatool-universal-apple-darwin"
chmod 0755 "$OUT/ipatool-universal-apple-darwin"
printf '%s\n' "$want" > "$STAMP"
echo "ipatool $BUILD_VERSION built into $OUT"
