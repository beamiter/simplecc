" SimpleCC in a SimpleRemote workspace: lifecycle, remote:// buffers, and
" what reaches the daemon.
"
" SimpleRemote is simulated, not loaded: g:simpleremote_workspace is set the
" way it publishes it, its User events are fired with g:simpleremote_event,
" its BufReadCmd for remote:// buffers is stood in for by one that fills the
" buffer from a local directory a moment later (asynchronously, like the real
" agent round trip), and the g:SimpleRemote* API functions the plugin calls
" are stubs that run their shell commands in that same directory.  The daemon
" is a script that answers `initialize` and writes everything else it is sent
" to a trace, so the test reads what a real daemon would have received.
"
" Run:  vim -Nu NONE -n -i NONE -es -S test/remote_lifecycle.vim

set nocompatible
set encoding=utf-8
set nomore
set noswapfile
set hidden

let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)
call delete(s:root .. '/test/remote-lifecycle-errors.log')

let g:simplecc_auto_start = 0
let g:simplecc_no_default_maps = 1
let g:simplecc_python_state_file = tempname()
runtime plugin/simplecc.vim
execute 'source ' .. fnameescape(s:root .. '/autoload/simplecc.vim')
filetype on

let s:sid = getscriptinfo({'name': 'autoload/simplecc.vim'})[0].sid
function! s:Call(name, ...) abort
  return call(function(printf('<SNR>%d_%s', s:sid, a:name)), a:000)
endfunction

function! s:Wait(expr, ms) abort
  let l:i = 0
  while l:i < a:ms / 10
    if eval(a:expr)
      return 1
    endif
    sleep 10m
    let l:i += 1
  endwhile
  return eval(a:expr)
endfunction

" ------------------------------------------------------------- the daemon ---

let s:trace = tempname()
let s:daemon = tempname()
call writefile([
      \ '#!/usr/bin/env bash',
      \ 'set -euo pipefail',
      \ 'while IFS= read -r line; do',
      \ '  printf ''%s\n'' "$line" >> ' .. shellescape(s:trace),
      \ '  case "$line" in',
      \ '    *''"type":"initialize"''*)',
      \ '      id="$(printf ''%s\n'' "$line" | sed -n ''s/.*"id":\([0-9][0-9]*\).*/\1/p'')"',
      \ '      printf ''{"type":"initialized","id":%s}\n'' "$id"',
      \ '      ;;',
      \ '    *''"type":"workspace/reloadConfiguration"''*)',
      \ '      id="$(printf ''%s\n'' "$line" | sed -n ''s/.*"id":\([0-9][0-9]*\).*/\1/p'')"',
      \ '      printf ''{"type":"configurationReloaded","id":%s,"servers":0}\n'' "$id"',
      \ '      ;;',
      \ '    *''"type":"shutdown"''*)',
      \ '      id="$(printf ''%s\n'' "$line" | sed -n ''s/.*"id":\([0-9][0-9]*\).*/\1/p'')"',
      \ '      printf ''{"type":"shutdown","id":%s}\n'' "$id"',
      \ '      exit 0',
      \ '      ;;',
      \ '  esac',
      \ 'done',
      \ ], s:daemon)
call assert_equal(1, setfperm(s:daemon, 'rwx------'))
let g:simplecc_daemon_path = s:daemon
call writefile([], s:trace)

" Every traced message of one type, oldest first.
function! s:Traced(type) abort
  let l:found = []
  for l:line in readfile(s:trace)
    let l:msg = json_decode(l:line)
    if get(l:msg, 'type', '') ==# a:type
      call add(l:found, l:msg)
    endif
  endfor
  return l:found
endfunction

function! s:WaitTraced(type, count, ms) abort
  return s:Wait('len(s:Traced(' .. string(a:type) .. ')) >= ' .. a:count, a:ms)
endfunction

" ------------------------------------------------------- fake SimpleRemote ---

" The "remote host": a local directory that stands in for the workspace root.
let s:host = tempname()
call mkdir(s:host .. '/pkg', 'p')
let s:remote_root = s:host

function! s:Snapshot(id, ...) abort
  let l:extra = a:0 ? a:1 : {}
  return extend({'id': a:id, 'kind': 'ssh', 'target': 'devbox',
        \ 'root': s:remote_root, 'tree_root': s:remote_root, 'local_root': '',
        \ 'mode': 'virtual', 'runtime': '', 'runtime_version': '', 'protocol': 'json',
        \ 'probe': {}, 'uri': 'remote://' .. s:remote_root}, l:extra)
endfunction

let s:events = []
function! s:Emit(event, payload) abort
  let g:simpleremote_event = extend(copy(a:payload), {'event': a:event,
        \ 'status': get(g:, 'simpleremote_status', 'disconnected'), 'time': localtime()})
  call add(s:events, a:event)
  execute 'doautocmd <nomodeline> User ' .. a:event
endfunction

" What SimpleRemote does: publish the snapshot, then announce it.
function! s:Connect(id, ...) abort
  let g:simpleremote_workspace = call('s:Snapshot', [a:id] + a:000)
  let g:simpleremote_status = 'ssh:devbox'
  call s:Emit('SimpleRemoteConnected', copy(g:simpleremote_workspace))
endfunction

" ... and clear the globals BEFORE Disconnected fires.
function! s:Disconnect(reason) abort
  unlet! g:simpleremote_workspace
  let g:simpleremote_status = 'disconnected'
  call s:Emit('SimpleRemoteDisconnected', {'reason': a:reason})
endfunction

" SimpleRemote's BufReadCmd: the read is asynchronous, the buffer holds one
" empty line meanwhile, and User SimpleRemoteBufferRead follows the fill.
let s:read_delay = 40
let s:reads = 0
function! s:FakeRead(uri) abort
  let l:buf = bufnr('%')
  let l:path = substitute(a:uri, '^remote://', '', '')
  let l:generation = get(get(g:, 'simpleremote_workspace', {}), 'id', -1)
  if !exists('g:simpleremote_workspace')
    return
  endif
  let s:reads += 1
  let l:request = s:reads
  call setbufvar(l:buf, 'vimrc_remote_read', {'request_id': l:request, 'tick': getbufvar(l:buf, 'changedtick')})
  call timer_start(s:read_delay, {_ -> s:FinishRead(l:buf, l:path, a:uri, l:generation, l:request)})
endfunction

function! s:FinishRead(buf, path, uri, generation, request) abort
  if !bufexists(a:buf) || get(getbufvar(a:buf, 'vimrc_remote_read', {}), 'request_id', -1) != a:request
    return
  endif
  call setbufvar(a:buf, 'vimrc_remote_read', {})
  if !filereadable(a:path)
    return
  endif
  let l:lines = readfile(a:path)
  call setbufline(a:buf, 1, empty(l:lines) ? [''] : l:lines)
  if len(l:lines) < len(getbufline(a:buf, 1, '$'))
    call deletebufline(a:buf, len(l:lines) + 1, '$')
  endif
  call setbufvar(a:buf, '&buftype', 'acwrite')
  call setbufvar(a:buf, '&swapfile', 0)
  call setbufvar(a:buf, 'vimrc_remote', {'path': a:path, 'uri': a:uri, 'generation': a:generation})
  call setbufvar(a:buf, '&modified', 0)
  " DetectRemoteFiletype: only when nothing set one, and only in a window.
  if getbufvar(a:buf, '&filetype') ==# ''
    let l:winid = bufwinid(a:buf)
    if l:winid > 0
      call win_execute(l:winid, 'filetype detect')
    endif
  endif
  call s:Emit('SimpleRemoteBufferRead', {'type': 'buffer-read', 'bufnr': a:buf,
        \ 'path': a:path, 'workspace': copy(get(g:, 'simpleremote_workspace', {}))})
endfunction

function! s:FakeWrite() abort
  let l:info = get(b:, 'vimrc_remote', {})
  if empty(l:info)
    return
  endif
  call writefile(getline(1, '$'), l:info.path)
  setlocal nomodified
  doautocmd <nomodeline> BufWritePost
endfunction

" g:VimrcRemoteActivateBuffer: a buffer that was filled while hidden has no
" filetype yet (`filetype detect` needs a window), so SimpleRemote detects it
" the first time the buffer is entered -- which is what attaches a buffer a
" workspace edit loaded in the background.
function! s:FakeActivate() abort
  if !empty(get(b:, 'vimrc_remote_read', {})) || empty(get(b:, 'vimrc_remote', {}))
    return
  endif
  if &filetype ==# ''
    filetype detect
  endif
endfunction

augroup fake_simpleremote
  autocmd!
  autocmd BufReadCmd remote://* call s:FakeRead(expand('<amatch>'))
  autocmd BufWriteCmd remote://* call s:FakeWrite()
  autocmd BufEnter remote://* call s:FakeActivate()
augroup END

" The API the plugin reaches for, run against the local stand-in host.
let s:executed = []
function! g:SimpleRemoteExecute(command, Callback) abort
  call add(s:executed, a:command)
  " g:SimpleRemoteExecute() runs the script in the workspace root.
  let l:output = system('cd ' .. shellescape(s:remote_root) .. ' && ' .. a:command)
  let l:ok = v:shell_error == 0
  call timer_start(5, {_ -> call(a:Callback, [l:ok, l:output])})
  return len(s:executed)
endfunction

let s:written = []
function! g:SimpleRemoteWriteFile(path, content, Callback) abort
  call add(s:written, a:path)
  call writefile(split(a:content, "\n", 1)[:-2], a:path)
  call timer_start(5, {_ -> call(a:Callback, [v:true, a:path])})
  return len(s:written)
endfunction

function! g:SimpleRemoteStatusline() abort
  return exists('g:simpleremote_workspace') ? 'ssh:devbox:' .. fnamemodify(s:remote_root, ':t') .. '@12ms' : ''
endfunction

let s:config_reloads = 0
function! g:VimrcRemoteReloadConfig() abort
  let s:config_reloads += 1
  let l:path = s:remote_root .. '/simplecc.json'
  if filereadable(l:path)
    let g:vimrc_remote_simplecc_config = join(readfile(l:path), "\n")
    call s:Emit('SimpleRemoteConfigChanged', {'config': g:vimrc_remote_simplecc_config})
  endif
endfunction

" ------------------------------------------------- lifecycle: single owner ---

call simplecc#Start()
call assert_equal(1, s:Wait("g:simplecc_status ==# 'ready'", 3000), 'local start')
call assert_equal(1, s:WaitTraced('initialize', 1, 2000))
call assert_equal(v:null, s:Traced('initialize')[0].remote, 'a local initialize has no remote')

" Connected: the daemon is re-initialized against the workspace.
call s:Connect(1)
call assert_equal(1, s:WaitTraced('initialize', 2, 4000),
      \ 'SimpleRemoteConnected restarts the daemon for the remote workspace')
call assert_equal(1, s:Wait("g:simplecc_status ==# 'ready'", 3000))
let s:init = s:Traced('initialize')[-1]
call assert_equal({'kind': 'ssh', 'target': 'devbox', 'root': s:remote_root, 'runtime': ''},
      \ s:init.remote, 'the remote transport reaches the daemon')
call assert_equal(s:remote_root, s:init.root)
call assert_equal('', s:init.config_path, 'no local config path for a remote workspace')
call assert_equal('', s:init.python_path, 'no probe, no selection: the daemon decides')

" The same generation announced again is not a second restart, and the
" first half of a workspace switch (Disconnected/reconnect) is not either.
call s:Emit('SimpleRemoteConnected', copy(g:simpleremote_workspace))
sleep 300m
call assert_equal(2, len(s:Traced('initialize')),
      \ 'a duplicate Connected for the workspace already served must not restart')
call s:Disconnect('reconnect')
sleep 300m
call assert_equal(2, len(s:Traced('initialize')),
      \ 'Disconnected(reconnect) must not restart the local servers')
call assert_equal('ready', g:simplecc_status, 'the daemon keeps serving until the new workspace is announced')

" The second half: a new generation, so a restart.
call s:Connect(2)
call assert_equal(1, s:WaitTraced('initialize', 3, 4000), 'the following Connected restarts')
call assert_equal(1, s:Wait("g:simplecc_status ==# 'ready'", 3000))
call assert_equal(s:remote_root, s:Traced('initialize')[-1].remote.root)

" ---------------------------------------- BufferRead: attach + generation ---

call writefile(['import os', 'x = 1'], s:remote_root .. '/pkg/mod.py')
call writefile([], s:trace)
execute 'edit ' .. fnameescape('remote://' .. s:remote_root .. '/pkg/mod.py')
let s:mod = bufnr('%')
call assert_equal(v:true, s:Call('RemoteReadPending', s:mod), 'the read is in flight')
" A filetype set while the read is pending (a modeline plugin, the user)
" fires FileType -> OnBufOpen, but the buffer has no contents and no
" b:vimrc_remote yet: it must not be sent, or the server opens an empty file.
setlocal filetype=python
sleep 20m
call assert_equal([], s:Traced('textDocument/didOpen'),
      \ 'a remote buffer whose read has not completed is not opened on the server')
" When the contents land, User SimpleRemoteBufferRead attaches it.
call assert_equal(1, s:WaitTraced('textDocument/didOpen', 1, 2000),
      \ 'SimpleRemoteBufferRead attaches the filled buffer')
let s:open = s:Traced('textDocument/didOpen')[0]
call assert_equal('file://' .. s:remote_root .. '/pkg/mod.py', s:open.uri,
      \ 'the server sees the remote path, not the remote:// scheme')
call assert_equal('python', s:open.languageId)
call assert_equal("import os\nx = 1\n", s:open.text)
call assert_equal(['import os', 'x = 1'], getline(1, '$'))
call assert_equal('acwrite', &buftype)
call assert_equal(2, b:vimrc_remote.generation)

" The plain path: no filetype until SimpleRemote's own `filetype detect`
" after the fill; FileType opens it and the event handler is idempotent.
call writefile(['def f(): pass'], s:remote_root .. '/pkg/other.py')
call writefile([], s:trace)
execute 'edit ' .. fnameescape('remote://' .. s:remote_root .. '/pkg/other.py')
call assert_equal(1, s:WaitTraced('textDocument/didOpen', 1, 2000))
sleep 100m
call assert_equal(1, len(s:Traced('textDocument/didOpen')),
      \ 'FileType and SimpleRemoteBufferRead together open the document exactly once')

" A buffer from a previous connection is not sent to this one, and a remote
" buffer is not sent to the local servers after a disconnect.
call writefile([], s:trace)
enew!
silent file remote:///srv/elsewhere/old.py
setlocal buftype=acwrite
let b:vimrc_remote = {'path': '/srv/elsewhere/old.py', 'uri': 'remote:///srv/elsewhere/old.py', 'generation': 1}
setlocal filetype=python
sleep 50m
call assert_equal([], s:Traced('textDocument/didOpen'),
      \ 'a buffer filled by a previous connection is not replayed to the new host')
call s:Call('SendDidOpen', s:mod)
call assert_equal(1, s:WaitTraced('textDocument/didOpen', 1, 1000), 'a current one still is')
" Special buffers are skipped whatever their filetype.
call writefile([], s:trace)
enew!
setlocal buftype=nofile
silent file /tmp/scratch-nofile.py
setlocal filetype=python
sleep 50m
call assert_equal([], s:Traced('textDocument/didOpen'), 'nofile buffers are never sent')
setlocal buftype=

" A file that exists only on this machine is not sent either: every server
" runs on the host, so it could not resolve the file -- and the file:// URIs
" it answered with would be read back as paths on the host, which turned a
" `gd` inside a local buffer into a jump to remote://<that local path>.
let s:local_file = tempname() .. '.py'
call writefile(['import os', 'y = 2'], s:local_file)
call writefile([], s:trace)
execute 'edit ' .. fnameescape(s:local_file)
call assert_equal('python', &filetype)
sleep 50m
call assert_equal([], s:Traced('textDocument/didOpen'),
      \ 'a local buffer is not offered to the servers on the host')

" ---------------------------------------------- deferred jumps into remote ---

call writefile(['line 1', 'line 2', 'line 3', 'line 4', 'a中😀z tail'],
      \ s:remote_root .. '/pkg/target.py')
let s:target_uri = 'file://' .. s:remote_root .. '/pkg/target.py'
execute 'buffer ' .. s:mod
" character 4 on 'a中😀z tail' is 'z' (a=1, 中=1, 😀=2 UTF-16 units) = byte column 9.
call s:Call('JumpToLocation', {'uri': s:target_uri, 'line': 4, 'character': 4})
call assert_equal('remote://' .. s:remote_root .. '/pkg/target.py', bufname('%'),
      \ 'the jump opens the remote buffer')
call assert_equal(1, line('.'), 'nothing to land on yet: one empty line')
call assert_equal(1, s:Wait("line('.') == 5", 2000),
      \ 'the cursor is placed once the contents arrive')
call assert_equal(9, col('.'), 'with the UTF-16 column converted against the real line')

" Already loaded: no deferral, the cursor moves at once.
execute 'buffer ' .. s:mod
call s:Call('JumpToLocation', {'uri': s:target_uri, 'line': 1, 'character': 2})
call assert_equal('remote://' .. s:remote_root .. '/pkg/target.py', bufname('%'))
call assert_equal(2, line('.'), 'the contents are here: no deferral')
call assert_equal(3, col('.'))

" showDocument with a selection, into a not-yet-read file.
call writefile(['one', 'two', 'three'], s:remote_root .. '/pkg/shown.py')
call s:Call('OnShowDocument', {'uri': 'file://' .. s:remote_root .. '/pkg/shown.py',
      \ 'selection': {'line': 2, 'character': 1}})
call assert_equal('remote://' .. s:remote_root .. '/pkg/shown.py', bufname('%'))
call assert_equal(1, s:Wait("line('.') == 3", 2000), 'showDocument selection lands after the read')
call assert_equal(2, col('.'))

" Quickfix: the UTF-16 column travels in user_data so <CR> on an entry for
" an unread file can still land on the right column.
execute 'bwipeout! ' .. bufnr('remote://' .. s:remote_root .. '/pkg/target.py')
execute 'buffer ' .. s:mod
call s:Call('LocationsToQuickfix', [
      \ {'uri': s:target_uri, 'line': 4, 'character': 4},
      \ {'uri': s:target_uri, 'line': 0, 'character': 0}], 'References')
call assert_equal('quickfix', &buftype, 'the location list is open')
let s:items = getloclist(0)
call assert_equal(2, len(s:items))
call assert_equal({'character': 4}, s:items[0].user_data)
call assert_equal(5, s:items[0].col, 'the byte column of an unread file is only an estimate')
call cursor(1, 1)
call simplecc#QfEnter()
call assert_equal('remote://' .. s:remote_root .. '/pkg/target.py', bufname('%'))
call assert_equal(1, s:Wait("line('.') == 5", 2000), 'QfEnter defers into the unread buffer')
call assert_equal(9, col('.'), 'and uses the UTF-16 column, not the estimate')

" The call- and type-hierarchy lists are the same kind of list and get the
" same <CR>.  Their columns are built while the target buffer is loaded, so
" they are the real byte columns -- and by the time <CR> is pressed that
" buffer may have been dropped and have to be read again, which is exactly
" when a byte column must not be re-read as a UTF-16 one.
execute 'buffer ' .. s:mod
call s:Call('OnBackendEvent', {'type': 'incomingCalls', 'calls': [
      \ {'item': {'uri': s:target_uri, 'line': 4, 'character': 4,
      \   'kind': 'function', 'name': 'caller'}}]})
call assert_equal('quickfix', &buftype, 'the incoming-calls list is open')
let s:calls = getqflist()
call assert_equal(1, len(s:calls))
call assert_equal({'character': 4}, s:calls[0].user_data,
      \ 'the UTF-16 column travels with the entry')
call assert_equal(9, s:calls[0].col, 'the loaded buffer gives the real byte column')
" ... and now the buffer is unloaded (a hidden remote buffer is dropped, a
" reconnect re-reads it), so pressing <CR> reads it again, asynchronously.
execute 'bunload! ' .. bufnr('remote://' .. s:remote_root .. '/pkg/target.py')
call cursor(1, 1)
call simplecc#QfEnter()
call assert_equal('remote://' .. s:remote_root .. '/pkg/target.py', bufname('%'),
      \ 'the entry opens the remote buffer')
call assert_equal(1, s:Wait("line('.') == 5", 2000),
      \ 'and the cursor is placed once the contents arrive')
call assert_equal(9, col('.'),
      \ 'on the UTF-16 column from user_data, not on byte column 8')

" The type hierarchy is the same list, built by the same producer.
call s:Call('OnBackendEvent', {'type': 'supertypes', 'items': [
      \ {'uri': s:target_uri, 'line': 4, 'character': 4,
      \   'kind': 'class', 'name': 'Base'}]})
call assert_equal('quickfix', &buftype, 'the supertypes list is open')
call assert_equal({'character': 4}, getqflist()[0].user_data,
      \ 'and carries the LSP column like every other list')
cclose

" ------------------------------------------ out-of-root definitions: remote ---

call assert_equal('remote:///usr/lib/python3/os.py', simplecc#UriToPath('file:///usr/lib/python3/os.py'),
      \ 'a definition outside the workspace root opens as a remote buffer')

" ------------------------------------------ workspace edits on remote files ---

" A text edit for a remote file that has no buffer: the buffer is loaded
" hidden, the edit waits for the read, and only then is the server answered.
call writefile(['def old(): pass', 'old()'], s:remote_root .. '/pkg/edit_me.py')
let s:edit_uri = 'file://' .. s:remote_root .. '/pkg/edit_me.py'
call writefile([], s:trace)
call s:Call('OnBackendEvent', {'type': 'applyEdit', 'server': 'pyright', 'requestId': 21,
      \ 'edit': {'operations': [
      \   {'kind': 'edit', 'uri': s:edit_uri, 'edits': [
      \     {'line': 0, 'character': 4, 'end_line': 0, 'end_character': 7, 'new_text': 'renamed'},
      \     {'line': 1, 'character': 0, 'end_line': 1, 'end_character': 3, 'new_text': 'renamed'}]}]}})
call assert_equal([], s:Traced('server/response'), 'no answer before the file is read')
call assert_equal(1, s:WaitTraced('server/response', 1, 3000), 'the server is answered after the read')
let s:reply = s:Traced('server/response')[0]
call assert_equal(21, s:reply.requestId)
call assert_equal(v:true, s:reply.result.applied, string(s:reply.result))
let s:edited = s:Call('BufnrForPath', 'remote://' .. s:remote_root .. '/pkg/edit_me.py')
call assert_notequal(-1, s:edited)
call assert_equal(['def renamed(): pass', 'renamed()'], getbufline(s:edited, 1, '$'),
      \ 'the edits are applied to the real contents, not to the empty placeholder')

" create + edit + rename + delete, run on the "host" through
" g:SimpleRemoteExecute, and answered once at the end.
call writefile(['pub fn thing() {}'], s:remote_root .. '/pkg/old.rs')
call writefile([], s:trace)
let s:executed = []
let s:R = {name -> 'file://' .. s:remote_root .. '/pkg/' .. name}
call s:Call('OnBackendEvent', {'type': 'applyEdit', 'server': 'rust-analyzer', 'requestId': 22,
      \ 'edit': {'operations': [
      \   {'kind': 'create', 'uri': s:R('fresh/made.rs'), 'overwrite': v:false, 'ignore_if_exists': v:false},
      \   {'kind': 'edit', 'uri': s:R('fresh/made.rs'), 'edits': [
      \     {'line': 0, 'character': 0, 'end_line': 0, 'end_character': 0, 'new_text': 'fn made() {}'}]},
      \   {'kind': 'rename', 'uri': s:R('old.rs'), 'new_uri': s:R('moved/new.rs'),
      \    'overwrite': v:false, 'ignore_if_exists': v:false},
      \   {'kind': 'delete', 'uri': s:R('mod.py'), 'recursive': v:false, 'ignore_if_not_exists': v:false}]}})
call assert_equal(1, s:WaitTraced('server/response', 1, 5000), 'the whole batch is answered')
let s:reply = s:Traced('server/response')[0]
call assert_equal(v:true, s:reply.result.applied, string(s:reply.result))
call assert_equal(1, filereadable(s:remote_root .. '/pkg/fresh/made.rs'), 'create makes the file and its directory')
let s:made = s:Call('BufnrForPath', 'remote://' .. s:remote_root .. '/pkg/fresh/made.rs')
call assert_equal(['fn made() {}'], getbufline(s:made, 1, '$'), 'the edit fills the freshly created file')
call assert_equal(0, filereadable(s:remote_root .. '/pkg/old.rs'), 'rename moves the file away')
call assert_equal(['pub fn thing() {}'], readfile(s:remote_root .. '/pkg/moved/new.rs'))
call assert_equal(0, filereadable(s:remote_root .. '/pkg/mod.py'), 'delete removes the file')
call assert_equal(1, s:Wait('!bufexists(' .. s:mod .. ')', 1000), 'and its buffer')
call assert_equal(3, len(s:executed), 'three resource operations ran on the host: ' .. string(s:executed))
call assert_equal(1, s:executed[0] =~# 'mkdir -p .* && : >', 'create: ' .. s:executed[0])
call assert_equal(1, s:executed[1] =~# 'mv -f', 'rename: ' .. s:executed[1])
call assert_equal(1, s:executed[2] =~# 'rm -f', 'delete: ' .. s:executed[2])

" Refusals keep their names.
call writefile([], s:trace)
call s:Call('OnBackendEvent', {'type': 'applyEdit', 'server': 'rust-analyzer', 'requestId': 23,
      \ 'edit': {'operations': [
      \   {'kind': 'delete', 'uri': s:R('never.rs'), 'recursive': v:false, 'ignore_if_not_exists': v:false}]}})
call assert_equal(1, s:WaitTraced('server/response', 1, 3000))
let s:reply = s:Traced('server/response')[0]
call assert_equal(v:false, s:reply.result.applied)
call assert_equal(1, s:reply.result.failureReason =~# 'does not exist', string(s:reply.result))

" A rename with unsaved changes in the moved buffer saves them through the
" API first, then retargets the buffer to the new name.
call writefile(['old body'], s:remote_root .. '/pkg/dirty.rs')
execute 'edit ' .. fnameescape('remote://' .. s:remote_root .. '/pkg/dirty.rs')
call assert_equal(1, s:Wait("getline(1) ==# 'old body'", 2000))
call setline(1, 'new body')
call assert_equal(1, &modified)
let s:written = []
call writefile([], s:trace)
call s:Call('OnBackendEvent', {'type': 'applyEdit', 'server': 'rust-analyzer', 'requestId': 24,
      \ 'edit': {'operations': [
      \   {'kind': 'rename', 'uri': s:R('dirty.rs'), 'new_uri': s:R('clean.rs'),
      \    'overwrite': v:false, 'ignore_if_exists': v:false}]}})
call assert_equal(1, s:WaitTraced('server/response', 1, 3000))
call assert_equal(v:true, s:Traced('server/response')[0].result.applied)
call assert_equal([s:remote_root .. '/pkg/dirty.rs'], s:written, 'unsaved text is written before the move')
call assert_equal(['new body'], readfile(s:remote_root .. '/pkg/clean.rs'))
call assert_equal(1, s:Wait("bufname('%') ==# 'remote://' .. s:remote_root .. '/pkg/clean.rs'", 2000),
      \ 'the window now shows the buffer for the new name')
call assert_equal(-1, s:Call('BufnrForPath', 'remote://' .. s:remote_root .. '/pkg/dirty.rs'),
      \ 'no buffer keeps the old name')

" ------------------------------------------- watched files and configuration ---

call writefile([], s:trace)
call s:Emit('SimpleRemoteFilesChanged', {'changes': [
      \ {'path': s:remote_root .. '/pkg/new.py', 'type': 'created'},
      \ {'path': s:remote_root .. '/pkg/gone.py', 'type': 'deleted'},
      \ {'path': s:remote_root .. '/pkg/touched.py', 'type': 'changed'},
      \ {'path': 'relative/ignored.py', 'type': 'changed'}],
      \ 'workspace': copy(g:simpleremote_workspace)})
call assert_equal(1, s:WaitTraced('workspace/didChangeWatchedFiles', 1, 2000),
      \ 'SimpleRemoteFilesChanged is forwarded to the daemon')
call assert_equal([
      \ {'uri': 'file://' .. s:remote_root .. '/pkg/new.py', 'type': 1},
      \ {'uri': 'file://' .. s:remote_root .. '/pkg/gone.py', 'type': 3},
      \ {'uri': 'file://' .. s:remote_root .. '/pkg/touched.py', 'type': 2}],
      \ s:Traced('workspace/didChangeWatchedFiles')[0].changes,
      \ 'as LSP FileChangeType values with the remote file URIs')

" ConfigChanged: a hot reload carrying the remote text, never a local path.
let g:vimrc_remote_simplecc_config = '{"languageServers": {"pyright": {"command": "pyright-langserver", "args": ["--stdio"], "filetypes": ["python"]}}}'
call writefile([], s:trace)
call s:Emit('SimpleRemoteConfigChanged', {'config': g:vimrc_remote_simplecc_config})
call assert_equal(1, s:WaitTraced('workspace/reloadConfiguration', 1, 2000))
let s:reload = s:Traced('workspace/reloadConfiguration')[0]
call assert_equal(g:vimrc_remote_simplecc_config, s:reload.remoteConfig)
call assert_equal('', s:reload.configPath)

" Saving the remote simplecc.json asks SimpleRemote to fetch it again, which
" is what fires ConfigChanged; :SimpleCCConfig opens exactly that file.
call writefile([], s:trace)
let s:config_reloads = 0
call simplecc#OpenConfig()
call assert_equal(1, s:Wait("bufname('%') ==# 'remote://' .. s:remote_root .. '/simplecc.json'", 2000),
      \ ':SimpleCCConfig opens the remote configuration as a remote buffer')
call assert_equal(1, s:Wait("getline(1) ==# '{'", 2000), 'a missing config is created on the host first')
call assert_equal(1, filereadable(s:remote_root .. '/simplecc.json'))
call assert_equal(1, s:Wait("!empty(get(b:, 'vimrc_remote', {}))", 2000))
call setline(2, '  "languageServers": {"ruff": {"command": "ruff", "args": ["server"], "filetypes": ["python"]},')
write
call assert_equal(1, s:config_reloads, 'saving the remote config triggers SimpleRemote''s refetch')
call assert_equal(1, s:WaitTraced('workspace/reloadConfiguration', 1, 2000))
call assert_equal(1, s:Traced('workspace/reloadConfiguration')[0].remoteConfig =~# 'ruff',
      \ 'and the daemon gets the saved text')

" ------------------------------------------------------- health and status ---

execute 'buffer ' .. s:edited
let s:report = simplecc#HealthReport()
function! s:Line(pattern) abort
  for l:line in s:report
    if l:line =~# a:pattern
      return l:line
    endif
  endfor
  return ''
endfunction
call assert_notequal('', s:Line('^REMOTE$'), 'the report has a REMOTE section')
call assert_notequal('', s:Line('^\[OK\] workspace: ssh:devbox ' .. s:remote_root .. ' (generation 2)'))
call assert_notequal('', s:Line('^\[INFO\] mode: virtual (remote:// buffers)'))
call assert_notequal('', s:Line('^\[WARN\] runtime: none'), 'no runtime published: plain ssh')
call assert_notequal('', s:Line('^\[INFO\] runtime probe: not run yet'))
call assert_notequal('', s:Line('^\[OK\] remote config: ' .. s:remote_root .. '/simplecc.json'))
call assert_notequal('', s:Line('^\[INFO\] server ruff: ruff \[python\] on ssh:devbox'),
      \ 'remote servers are listed without a local executable() verdict')
call assert_equal('', s:Line(':SimpleCCInstall'), 'no local install hint for a remote server')
call assert_notequal('', s:Line('^\[OK\] remote file: ' .. s:remote_root .. '/pkg/edit_me.py (connection current)'))
call assert_equal('', s:Line('special buffers are never sent'), 'a remote acwrite buffer is not "special"')
call assert_notequal('', s:Line('^\[INFO\] workspace root: ' .. s:remote_root .. ' (remote)'))
call assert_notequal('', s:Line('^\[OK\] document: open, version'), 'the remote buffer is served')

" With a probe, the report says what was found.
let g:simpleremote_workspace.probe = {'python': '/usr/bin/python3', 'python_version': 'Python 3.12.1',
      \ 'python_lsp': '', 'runtime_ms': '17', 'uname': 'Linux x86_64', 'status': '0'}
let g:simpleremote_workspace.runtime = s:daemon
let g:simpleremote_workspace.runtime_version = '0.9.0'
let s:report = simplecc#HealthReport()
call assert_notequal('', s:Line('^\[OK\] runtime: ' .. s:daemon .. ' v0.9.0 (protocol json)'))
call assert_notequal('', s:Line('^\[OK\] remote python: /usr/bin/python3 (Python 3.12.1)'))
call assert_notequal('', s:Line('^\[WARN\] remote python LSP: (none found)'))
call assert_notequal('', s:Line('^\[INFO\] probe: uname=Linux x86_64 runtime_ms=17'))
call assert_notequal('', s:Line('^\[INFO\] python selection: /usr/bin/python3 / (auto) (from the runtime probe)'))

let s:status = execute('SimpleCC')
call assert_equal(1, s:status =~# 'remote: ssh:devbox:' .. fnamemodify(s:remote_root, ':t') .. '@12ms',
      \ ':SimpleCC shows the SimpleRemote statusline: ' .. s:status)

" ------------------------------------------ RuntimeReady: python selection ---

" The probe just changed what `initialize` would send (python found, no
" :SimpleCCPython selection): a restart picks it up ...
call writefile([], s:trace)
call s:Emit('SimpleRemoteRuntimeReady', copy(g:simpleremote_workspace))
call assert_equal(1, s:WaitTraced('initialize', 1, 4000),
      \ 'a probe that changes the Python selection re-initializes')
call assert_equal(1, s:Wait("g:simplecc_status ==# 'ready'", 3000))
call assert_equal('/usr/bin/python3', s:Traced('initialize')[0].python_path)
call assert_equal('', s:Traced('initialize')[0].python_lsp_path)
" ... and the same probe again does not.
call s:Emit('SimpleRemoteRuntimeReady', copy(g:simpleremote_workspace))
sleep 300m
call assert_equal(1, len(s:Traced('initialize')), 'an unchanged probe is not a restart')

" ---------------------------------------------- disconnect: back to local ---

call writefile([], s:trace)
call s:Disconnect('disconnect')
call assert_equal(1, s:WaitTraced('initialize', 1, 4000), 'a real disconnect restarts locally')
call assert_equal(1, s:Wait("g:simplecc_status ==# 'ready'", 3000))
call assert_equal(v:null, s:Traced('initialize')[0].remote)
" The remote buffers that are still open were not replayed to the local
" servers.
sleep 100m
for s:open in s:Traced('textDocument/didOpen')
  call assert_equal(1, s:open.uri !~# s:remote_root, 'stale remote buffer replayed locally: ' .. s:open.uri)
endfor
" The local file that was inert while connected is served again the moment
" the servers are local ones.
call assert_equal(1, index(map(copy(s:Traced('textDocument/didOpen')),
      \ {_, m -> m.uri}), 'file://' .. s:local_file) >= 0,
      \ 'the local buffer is opened on the local servers after the disconnect: '
      \ .. string(map(copy(s:Traced('textDocument/didOpen')), {_, m -> m.uri})))
" A Disconnected for a workspace that was never served is not a restart.
call s:Disconnect('disconnect')
sleep 300m
call assert_equal(1, len(s:Traced('initialize')), 'a Disconnected while local stays local')

call simplecc#Stop()
call s:Wait("g:simplecc_status ==# ''", 3000)
call delete(s:daemon)
call delete(s:trace)
call delete(s:host, 'rf')
call delete(s:local_file)

if len(v:errors)
  call writefile(v:errors, s:root .. '/test/remote-lifecycle-errors.log')
  for s:e in v:errors
    echomsg s:e
  endfor
  cquit
endif
qall!
