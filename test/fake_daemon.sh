#!/usr/bin/env bash
set -euo pipefail

while IFS= read -r line; do
  id="$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')"
  case "$line" in
    *'"type":"initialize"'*)
      printf '{"type":"initialized","id":%s}\n' "$id"
      ;;
    *'"type":"textDocument/definition"'*)
      uri="$(printf '%s\n' "$line" | sed -n 's/.*"uri":"\([^"]*\)".*/\1/p')"
      request_line="$(printf '%s\n' "$line" | sed -n 's/.*"line":\([0-9][0-9]*\).*/\1/p')"
      # Keep the response asynchronous long enough for the smoke test to move
      # to another split before Vim handles it.
      sleep 0.1
      if [[ "$request_line" == "0" ]]; then
        printf '{"type":"definition","id":%s,"locations":[{"uri":"%s","line":2,"character":1}]}\n' "$id" "$uri"
      else
        printf '{"type":"definition","id":%s,"locations":[{"uri":"%s","line":2,"character":1},{"uri":"%s","line":3,"character":1}]}\n' "$id" "$uri" "$uri"
      fi
      ;;
    *'"type":"shutdown"'*)
      printf '{"type":"shutdown","id":%s}\n' "$id"
      exit 0
      ;;
  esac
done
