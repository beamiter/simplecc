#!/usr/bin/env bash
set -euo pipefail

while IFS= read -r line; do
  id="$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')"
  case "$line" in
    *'"type":"initialize"'*)
      printf '{"type":"initialized","id":%s}\n' "$id"
      ;;
    *'"type":"shutdown"'*)
      printf '{"type":"shutdown","id":%s}\n' "$id"
      exit 0
      ;;
  esac
done
