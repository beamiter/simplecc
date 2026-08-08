" Document sync belongs to a buffer, not to whichever buffer is current.
"
" One global debounce timer whose callback read the *current* buffer meant that
" typing in a.rs and jumping to b.rs inside g:simplecc_change_delay (120ms --
" well within a gd jump or CTRL-^) sent b.rs a didChange it had not earned,
" burning a version on it, and never flushed a.rs at all. The server kept
" answering completions, hover and diagnostics from a.rs's pre-edit text until
" it was edited again, saved, or completion was triggered inside it.
"
" Run:  vim -Nu NONE -n -i NONE -es -S test/change_sync.vim

set nocompatible
set encoding=utf-8
set nomore
set noswapfile
" Buffers must survive being switched away from: with 'nohidden' the buffer
" left behind is unloaded, and a modified one cannot be left at all.
set hidden

let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)
call delete(s:root .. '/test/change-sync-errors.log')

let g:simplecc_auto_start = 0
let g:simplecc_no_default_maps = 1
runtime plugin/simplecc.vim
execute 'source ' .. fnameescape(s:root .. '/autoload/simplecc.vim')

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

let s:trace = tempname()
let s:daemon = tempname()
call writefile([
      \ '#!/usr/bin/env bash',
      \ 'set -euo pipefail',
      \ 'while IFS= read -r line; do',
      \ '  case "$line" in',
      \ '    *''"type":"initialize"''*)',
      \ '      id="$(printf ''%s\n'' "$line" | sed -n ''s/.*"id":\([0-9][0-9]*\).*/\1/p'')"',
      \ '      printf ''{"type":"initialized","id":%s}\n'' "$id"',
      \ '      ;;',
      \ '    *''"type":"shutdown"''*)',
      \ '      id="$(printf ''%s\n'' "$line" | sed -n ''s/.*"id":\([0-9][0-9]*\).*/\1/p'')"',
      \ '      printf ''{"type":"shutdown","id":%s}\n'' "$id"',
      \ '      exit 0',
      \ '      ;;',
      \ '    *)',
      \ '      printf ''%s\n'' "$line" >> ' .. shellescape(s:trace),
      \ '      ;;',
      \ '  esac',
      \ 'done',
      \ ], s:daemon)
call assert_equal(1, setfperm(s:daemon, 'rwx------'))
let g:simplecc_daemon_path = s:daemon

" Every didChange seen so far, oldest first.
function! s:Changes() abort
  let l:out = []
  for l:line in readfile(s:trace)
    let l:msg = json_decode(l:line)
    if get(l:msg, 'type', '') ==# 'textDocument/didChange'
      call add(l:out, l:msg)
    endif
  endfor
  return l:out
endfunction

" A didChange is full-text on the first versions of a document and incremental
" afterwards; both carry the edited text, in different shapes.
function! s:ChangeText(msg) abort
  if has_key(a:msg, 'text')
    return a:msg.text
  endif
  return join(map(copy(get(a:msg, 'changes', [])), {_, c -> get(c, 'text', '')}), "")
endfunction

function! s:ChangeFor(uri) abort
  for l:msg in reverse(s:Changes())
    if get(l:msg, 'uri', '') ==# a:uri
      return l:msg
    endif
  endfor
  return {}
endfunction

call writefile([], s:trace)
call simplecc#Start()
call assert_equal(1, s:Wait("g:simplecc_status ==# 'ready'", 2000))

let g:simplecc_change_delay = 30

let s:a = tempname() .. '.rs'
let s:b = tempname() .. '.rs'
call writefile(['fn a() {}'], s:a)
call writefile(['fn b() {}'], s:b)

execute 'edit! ' .. fnameescape(s:b)
setfiletype rust
execute 'edit! ' .. fnameescape(s:a)
setfiletype rust
call s:Wait('0', 100)

let s:uri_a = simplecc#PathToUri(s:a)
let s:uri_b = simplecc#PathToUri(s:b)
let s:bufa = s:Call('BufnrForPath', s:a)
let s:bufb = s:Call('BufnrForPath', s:b)

" ----------------------------------------------- the timer owns its buffer ---

" Edit a.rs, schedule its flush, then leave for b.rs without letting the
" autocommands help: this is the debounce firing after the user moved on.
call setline(1, 'fn a() { let edited = 1; }')
call s:Call('ScheduleDidChange', s:bufa)
execute 'noautocmd buffer ' .. s:bufb
call writefile([], s:trace)
call assert_equal(1, s:Wait('!empty(s:Changes())', 2000),
      \ 'the scheduled flush must happen even though the user moved on')

let s:msg = s:Changes()[0]
call assert_equal(s:uri_a, get(s:msg, 'uri', ''),
      \ 'the flush belongs to the buffer that scheduled it, not the current one')
call assert_equal(1, s:ChangeText(s:msg) =~# 'let edited',
      \ 'and it carries that buffer''s text: ' .. string(s:ChangeText(s:msg)))
call assert_equal({}, s:ChangeFor(s:uri_b),
      \ 'the buffer that was merely switched to must not get a didChange')

" ------------------------------------------------- leaving flushes, per buffer ---

execute 'noautocmd buffer ' .. s:bufa
call writefile([], s:trace)
call setline(1, 'fn a() { let again = 2; }')
call simplecc#OnTextChanged()
" BufLeave fires here: the edits must go out while a.rs is still current.
execute 'buffer ' .. s:bufb
call s:Wait('!empty(s:Changes())', 2000)
call assert_notequal({}, s:ChangeFor(s:uri_a),
      \ 'leaving a buffer inside the debounce window must flush it')
call assert_equal(1, s:ChangeText(s:ChangeFor(s:uri_a)) =~# 'let again',
      \ 'with the edits that were pending when it was left')

" Both buffers keep their own timer: editing b.rs cannot cancel a.rs's flush.
call writefile([], s:trace)
execute 'noautocmd buffer ' .. s:bufa
call setline(1, 'fn a() { let third = 3; }')
call s:Call('ScheduleDidChange', s:bufa)
execute 'noautocmd buffer ' .. s:bufb
call setline(1, 'fn b() { let other = 4; }')
call s:Call('ScheduleDidChange', s:bufb)
call assert_equal(1, s:Wait('len(s:Changes()) >= 2', 2000),
      \ 'two edited buffers produce two flushes: ')
call assert_equal(1, s:ChangeText(s:ChangeFor(s:uri_a)) =~# 'let third',
      \ 'a.rs keeps its own timer')
call assert_equal(1, s:ChangeText(s:ChangeFor(s:uri_b)) =~# 'let other',
      \ 'and editing b.rs does not cancel it')

call simplecc#Stop()
call s:Wait("g:simplecc_status ==# ''", 2000)
call delete(s:daemon)
call delete(s:trace)
call delete(s:a)
call delete(s:b)

if len(v:errors)
  call writefile(v:errors, s:root .. '/test/change-sync-errors.log')
  for s:e in v:errors
    echomsg s:e
  endfor
  cquit
endif
qall!
