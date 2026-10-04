" Range-aware code actions and formatting.
"
" :SimpleCCAction always sent end_line == line and an empty diagnostics
" context, so no refactor.extract action ("extract function", "extract
" variable", TypeScript's "move to a new file") and no quickfix action bound to
" a diagnostic was ever reachable. There was no rangeFormatting at all, and
" :SimpleCCSelExpand had no -range, so repeating it from the selection it had
" just created aborted with E481.
"
" Run:  vim -Nu NONE -n -i NONE -es -S test/range_requests.vim

set nocompatible
set encoding=utf-8
set nomore

let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)
call delete(s:root .. '/test/range-requests-errors.log')

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

" A daemon that answers `initialize` and echoes every later request back into a
" file, so the test can assert on exactly what went over the wire.
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

function! s:Traced(type) abort
  call s:Wait('!empty(filter(readfile(' .. string(s:trace) .. '), {_, l -> l =~# ' ..
        \ string('"type":"' . a:type . '"') .. '}))', 2000)
  for l:line in reverse(readfile(s:trace))
    let l:msg = json_decode(l:line)
    if get(l:msg, 'type', '') ==# a:type
      return l:msg
    endif
  endfor
  return {}
endfunction

call writefile([], s:trace)
call simplecc#Start()
call assert_equal(1, s:Wait("g:simplecc_status ==# 'ready'", 2000))

let s:file = tempname() .. '.rs'
call writefile(['fn main() {', '    let x = 1;', '    let y = 2;', '}'], s:file)
execute 'edit! ' .. fnameescape(s:file)
setfiletype rust
call s:Wait('0', 100)

" ------------------------------------------------------------ no range given ---

call cursor(2, 9)
SimpleCCAction
let s:msg = s:Traced('textDocument/codeAction')
call assert_equal([1, 8, 1, 8],
      \ [s:msg.line, s:msg.character, s:msg.end_line, s:msg.end_character],
      \ 'without a range the code action is still a cursor position')

SimpleCCFormat
call assert_notequal({}, s:Traced('textDocument/formatting'),
      \ 'without a range formatting still covers the whole document')

" --------------------------------------------------------------- with a range ---

call writefile([], s:trace)
2,3SimpleCCAction
let s:msg = s:Traced('textDocument/codeAction')
call assert_equal([1, 0, 2, 14],
      \ [s:msg.line, s:msg.character, s:msg.end_line, s:msg.end_character],
      \ 'a selection must be sent as a real range, or refactor.extract is unreachable')

" Diagnostics intersecting the range travel with it, so quickfix actions bind.
call s:Call('OnDiagnostics', {'uri': s:file, 'server': 'rust-analyzer', 'items': [
      \ {'line': 2, 'character': 8, 'end_line': 2, 'end_character': 9,
      \  'severity': 2, 'message': 'unused variable: `y`'},
      \ {'line': 0, 'character': 0, 'end_line': 0, 'end_character': 1,
      \  'severity': 1, 'message': 'somewhere else entirely'}]})
call writefile([], s:trace)
2,3SimpleCCAction
let s:msg = s:Traced('textDocument/codeAction')
call assert_equal(1, len(s:msg.diagnostics),
      \ 'only the diagnostics that intersect the range are sent as context')
call assert_equal('unused variable: `y`', s:msg.diagnostics[0].message)

call writefile([], s:trace)
2,3SimpleCCFormat
let s:msg = s:Traced('textDocument/rangeFormatting')
call assert_equal([1, 0, 2, 14],
      \ [s:msg.line, s:msg.character, s:msg.end_line, s:msg.end_character],
      \ 'a selection formats that range, not the whole document')
call assert_equal(&tabstop, s:msg.tab_size)

" ------------------------------------------------ formatting belongs to a buffer ---

set hidden
let s:src_buf = bufnr('%')
let s:src_tick = b:changedtick
let s:fmt_id = s:Call('NextId') + 1
call simplecc#Format()
let s:otherfmt = tempname() .. '.rs'
call writefile(['fn untouched() {}'], s:otherfmt)
execute 'edit! ' .. fnameescape(s:otherfmt)
call s:Call('OnFormatting', {'id': s:fmt_id, 'edits': [
      \ {'line': 1, 'character': 0, 'end_line': 1, 'end_character': 15,
      \  'new_text': '    let x = FORMATTED;'}]})
call assert_equal(['fn untouched() {}'], getline(1, '$'),
      \ 'a format reply must not rewrite the buffer you jumped to')
call assert_equal(['fn main() {', '    let x = FORMATTED;', '    let y = 2;', '}'],
      \ getbufline(s:src_buf, 1, '$'),
      \ 'it must still apply to the buffer that asked, even after a jump')
execute 'buffer ' .. s:src_buf
setfiletype rust
call writefile([], s:trace)
SimpleCCAction
let s:act = s:Traced('textDocument/codeAction')
let s:act_id = get(s:act, 'id', 0)
call assert_equal('rust', get(s:act, 'languageId', ''),
      \ 'the request itself names the originating filetype: ' .. string(s:act))
let s:md = tempname() .. '.md'
call writefile(['hello'], s:md)
execute 'edit! ' .. fnameescape(s:md)
setfiletype markdown
call assert_equal('rust', s:Call('PendingActionFiletype', s:act_id),
      \ 'code-action execute must keep the filetype of the requesting buffer')
execute 'buffer ' .. s:src_buf
call delete(s:md)

" ------------------------------------------------------- repeatable expansion ---

" With -range this no longer aborts on the '<,'> Vim inserts after the first
" invocation left the buffer in Visual mode.
let v:errmsg = ''
2,3SimpleCCSelExpand
2,3SimpleCCSelShrink
call assert_equal('', v:errmsg,
      \ 'selection expand/shrink must accept the range Vim inserts in Visual mode')

call simplecc#Stop()
call s:Wait("g:simplecc_status ==# ''", 2000)
call delete(s:daemon)
call delete(s:trace)
call delete(s:file)

if len(v:errors)
  call writefile(v:errors, s:root .. '/test/range-requests-errors.log')
  for s:e in v:errors
    echomsg s:e
  endfor
  cquit
endif
qall!
