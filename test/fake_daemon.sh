#!/usr/bin/env bash
set -euo pipefail

open_count=0
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
    *'"type":"textDocument/didOpen"'*)
      uri="$(printf '%s\n' "$line" | sed -n 's/.*"uri":"\([^"]*\)".*/\1/p')"
      open_count=$((open_count + 1))
      if [[ "$open_count" == "1" ]]; then
        # Mixed severities let the Vim smoke test exercise exact filtering and
        # navigation's g:simplecc_diag_min_severity boundary.
        printf '{"type":"diagnostics","uri":"%s","items":[{"line":0,"character":0,"end_line":0,"end_character":1,"severity":2,"source":"lint","code":7,"message":"same-position warning"},{"line":0,"character":0,"end_line":0,"end_character":1,"severity":2,"source":"alint","code":9,"message":"lexically first warning"},{"line":0,"character":0,"end_line":0,"end_character":1,"severity":1,"source":"rustc","code":"E0001","message":"source error"},{"line":1,"character":5,"end_line":1,"end_character":6,"severity":2,"source":"lint","code":42,"message":"source warning"},{"line":1,"character":1,"end_line":1,"end_character":2,"severity":4,"message":"hidden hint"},{"line":2,"character":1,"end_line":2,"end_character":2,"severity":4,"message":null}]}\n' "$uri"
      else
        printf '{"type":"diagnostics","uri":"%s","items":[{"line":0,"character":0,"end_line":0,"end_character":1,"severity":1,"message":"workspace error"}]}\n' "$uri"
      fi
      ;;
    *'"type":"shutdown"'*)
      printf '{"type":"shutdown","id":%s}\n' "$id"
      exit 0
      ;;
  esac
done
