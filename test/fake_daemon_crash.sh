#!/usr/bin/env bash
# A daemon that dies the way a real one does: abruptly, mid-session, with no
# shutdown handshake.  `workspace/reloadConfiguration` is the trigger because
# it is reachable from the public API (:SimpleCCReloadConfig) and needs no
# buffer state, so the test can decide exactly when the crash happens.
set -euo pipefail

while IFS= read -r line; do
  id="$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')"
  case "$line" in
    *'"type":"initialize"'*)
      printf '{"type":"initialized","id":%s}\n' "$id"
      ;;
    *'"type":"workspace/reloadConfiguration"'*)
      # No reply, no shutdown ack: this is an OOM kill or a panic.
      exit 9
      ;;
    *'"type":"shutdown"'*)
      printf '{"type":"shutdown","id":%s}\n' "$id"
      exit 0
      ;;
  esac
done
