#!/usr/bin/env python3
"""Real interactive Vim: Insert Ctrl-S preserves completion and defers safely."""
import json
import os
from pathlib import Path
import pty
import select
import subprocess
import tempfile
import time

root = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='simplecc-save-insert-') as directory:
    directory = Path(directory)
    file = directory / 'input.rs'
    state = directory / 'state'
    ready = directory / 'ready'
    trace = directory / 'trace'
    mode = directory / 'mode'
    file.write_text('raw\n')
    mode.write_text('normal')
    trace.touch()
    script = directory / 'init.vim'
    script.write_text(f'''
set nocompatible nomore noswapfile
set runtimepath^={root}
let g:simplecc_auto_start = 0
let g:simplecc_no_default_maps = 1
let g:simplecc_daemon_path = '{root}/test/fake_save_daemon.py'
runtime plugin/simplecc.vim
call simplecc#Start()
edit {file}
setfiletype rust
set completeopt=menuone,noselect
function! SaveComplete(findstart, base)
  return a:findstart ? 0 : ['raw', 'rain']
endfunction
set omnifunc=SaveComplete
let g:save_token = 0
let g:save_timeout = 3000
let g:user_insert_leaves = 0
augroup SaveUserOwned
  autocmd!
  autocmd InsertLeave <buffer> let g:user_insert_leaves += 1
augroup END
inoremap <C-S> <Cmd>let g:save_token = simplecc#save#Start({{'format': v:true, 'timeout_ms': g:save_timeout}})<CR>
function! SaveState(timer)
  call writefile([json_encode({{'mode':mode(1), 'text':getline(1), 'pum':pumvisible(), 'modified':&modified, 'timers':len(timer_info()),
        \\ 'deferred_autocmds':exists('#SimpleCCDeferredSave') ? len(autocmd_get({{'group':'SimpleCCDeferredSave'}})) : 0, 'user_autocmds':len(autocmd_get({{'group':'SaveUserOwned'}})),
        \\ 'status':simplecc#save#Status(bufnr(), g:save_token)}})], '{state}')
endfunction
call timer_start(30, function('SaveState'), {{'repeat': -1}})
call writefile(['ready'], '{ready}')
''')
    master, slave = pty.openpty()
    env = dict(os.environ, TERM='xterm-256color', SIMPLECC_SAVE_TRACE=str(trace), SIMPLECC_SAVE_MODE=str(mode))
    process = subprocess.Popen(['vim', '-Nu', 'NONE', '-n', '-i', 'NONE', '-S', str(script)], stdin=slave, stdout=slave, stderr=slave, env=env)
    os.close(slave)

    output = bytearray()

    def pump():
        if select.select([master], [], [], 0)[0]:
            try:
                output.extend(os.read(master, 65536))
            except OSError:
                pass

    def wait(predicate, description):
        until = time.monotonic() + 5
        while time.monotonic() < until:
            pump()
            try:
                value = json.loads(state.read_text()) if state.exists() else {}
                if predicate(value):
                    return value
            except (ValueError, OSError):
                pass
            time.sleep(.02)
        raise AssertionError(description + ': ' + (state.read_text() if state.exists() else 'no state') + '\nPTY output: ' + output[-3000:].decode(errors='replace'))

    try:
        wait(lambda s: ready.exists(), 'Vim startup')
        time.sleep(.3)
        os.write(master, b':setlocal omnifunc=SaveComplete\r')
        time.sleep(.1)
        os.write(master, b'i\x18\x0f')  # real Insert completion popup
        wait(lambda s: s.get('pum') == 1, 'completion visible')
        os.write(master, b'\x13')
        current = wait(lambda s: s.get('status', {}).get('message') == 'save deferred until InsertLeave', 'Ctrl-S deferred')
        assert current['mode'].startswith('i') and current['pum'] == 1, current
        # Repeated Ctrl-S supersedes the owner instead of accumulating hooks or
        # deadline timers, and never touches unrelated user autocmds.
        timer_count = current['timers']
        os.write(master, b'\x13' * 50)
        time.sleep(.25)
        current = wait(lambda s: s.get('status', {}).get('message') == 'save deferred until InsertLeave', 'repeated saves remain deferred')
        assert current['deferred_autocmds'] == 4, current
        assert current['timers'] <= timer_count + 1, current
        assert current['user_autocmds'] == 1 and current['pum'] == 1, current
        assert file.read_text() == 'raw\n'
        assert 'textDocument/formatting' not in trace.read_text()
        os.write(master, b'\x1b')
        current = wait(lambda s: s.get('status', {}).get('status') == 'success', 'deferred save completes')
        assert current['deferred_autocmds'] == 0 and current['user_autocmds'] == 1, current
        assert current.get('deferred_autocmds', 0) <= 4
        assert file.read_text() == 'formatted\n'
        os.write(master, b'u')
        wait(lambda s: s.get('text') == 'raw', 'single undo restores original text')
        os.write(master, b'0i\x13')
        wait(lambda s: s.get('status', {}).get('message') == 'save deferred until InsertLeave', 'second Ctrl-S deferred')
        os.write(master, b'X\x1b')
        current = wait(lambda s: s.get('status', {}).get('status') == 'error', 'typing cancels deferred save')
        assert current['text'] == 'Xraw' and current['modified'], current
        assert file.read_text() == 'formatted\n'
        assert current['deferred_autocmds'] == 0 and current['user_autocmds'] == 1, current
        os.write(master, b':let g:save_timeout = 150\r0i\x13')
        current = wait(lambda s: 'InsertLeave timed out' in s.get('status', {}).get('message', ''), 'deferred deadline cleans owner')
        assert current['deferred_autocmds'] == 0 and current['user_autocmds'] == 1, current
        os.write(master, b'\x1b:let g:save_timeout = 3000\r0i\x13')
        wait(lambda s: s.get('status', {}).get('message') == 'save deferred until InsertLeave', 'buffer leave test deferred')
        os.write(master, b'\x0f:new\r')
        current = wait(lambda s: s.get('text') == '' and s.get('deferred_autocmds') == 0, 'leaving owner buffer cleans hooks')
        assert current['user_autocmds'] == 1, current
        print('Insert Ctrl-S / completion / undo / typing race: PASS')
    finally:
        os.write(master, b'\x1b:qa!\r')
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        os.close(master)
