#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
SOURCE="$ROOT_DIR/target/release/simplecc-daemon"
DEST_DIR="$ROOT_DIR/lib"
DEST="$DEST_DIR/simplecc-daemon"
TMP_DEST=""

cd "$ROOT_DIR"
cargo build --release --locked

if [[ ! -f "$SOURCE" || ! -s "$SOURCE" || ! -x "$SOURCE" ]]; then
  echo "Build completed without an executable daemon at $SOURCE" >&2
  exit 1
fi

mkdir -p "$DEST_DIR"
TMP_DEST="$(mktemp "$DEST_DIR/.simplecc-daemon.XXXXXX")"
trap 'if [[ -n "$TMP_DEST" ]]; then rm -f -- "$TMP_DEST"; fi' EXIT

install -m 0755 "$SOURCE" "$TMP_DEST"
if [[ ! -f "$TMP_DEST" || ! -s "$TMP_DEST" || ! -x "$TMP_DEST" ]] ||
  ! cmp -s "$SOURCE" "$TMP_DEST"; then
  echo "Staged daemon verification failed" >&2
  exit 1
fi

mv -f "$TMP_DEST" "$DEST"
TMP_DEST=""

if [[ ! -f "$DEST" || ! -s "$DEST" || ! -x "$DEST" ]] ||
  ! cmp -s "$SOURCE" "$DEST"; then
  echo "Installed daemon verification failed" >&2
  exit 1
fi

echo "Installed executable: $DEST"
echo "Ensure this plugin directory is on Vim's 'runtimepath'."
