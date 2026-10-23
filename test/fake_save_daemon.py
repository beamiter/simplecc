#!/usr/bin/env python3
"""Deterministic protocol peer for the asynchronous save controller."""
import json
import os
import sys
import time

for line in sys.stdin:
    request = json.loads(line)
    kind = request.get('type', '')
    with open(os.environ['SIMPLECC_SAVE_TRACE'], 'a') as stream:
        stream.write(json.dumps(request) + '\n')
    reply = None
    if kind == 'initialize':
        reply = {'type': 'initialized', 'capabilities': {'save_pipeline': True}}
    elif kind == 'shutdown':
        break
    elif kind in ('textDocument/formatting', 'textDocument/organizeImports'):
        mode = open(os.environ['SIMPLECC_SAVE_MODE']).read().strip()
        if mode in ('delay', 'timeout'):
            time.sleep(0.35)
        text = 'formatted' if kind.endswith('formatting') else 'organized'
        reply = {'type': kind.split('/')[-1], 'edits': [
            {'line': 0, 'character': 0, 'end_line': 0,
             'end_character': 9 if text == 'organized' else 3, 'new_text': text}]}
        if mode == 'error':
            reply = {'type': 'error', 'message': 'test LSP failure'}
    if reply is not None:
        reply['id'] = request.get('id', 0)
        print(json.dumps(reply), flush=True)
