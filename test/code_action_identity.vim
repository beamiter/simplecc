" Code-action request identity, menu snapshots, context guards and legacy daemon gating.
set nocompatible
set encoding=utf-8
set nomore

let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)
call delete(s:root .. '/test/code-action-identity-errors.log')

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
      \ '      printf ''{"type":"initialized","id":%s,"capabilities":{"code_action_identity":true}}\n'' "$id"',
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
call writefile(['fn one() {}', 'fn two() {}', 'fn three() {}'], s:file)
execute 'edit ' .. fnameescape(s:file)
setfiletype rust
call cursor(1, 1)

function! s:Request(...) abort
  call writefile([], s:trace)
  if a:0
    execute a:1 .. 'SimpleCCAction'
  else
    SimpleCCAction
  endif
  return s:Traced('textDocument/codeAction')
endfunction
function! s:Reply(req, title) abort
  call s:Call('OnCodeAction', {'id': a:req.id, 'generation': a:req.id,
        \ 'actions': [{'title': a:title, 'index': 4}]})
endfunction
function! s:NoExecute() abort
  sleep 30m
  call assert_equal([], filter(readfile(s:trace), {_, l -> l =~# '"type":"textDocument/executeAction"'}))
endfunction

" Unrequested and superseded replies never create menus.
call s:Reply({'id': 999999}, 'unknown')
call assert_equal([], popup_list())
let s:a = s:Request()
let s:b = s:Request()
call s:Reply(s:a, 'old')
call assert_equal([], popup_list())
call s:Reply(s:b, 'current')
call assert_equal(1, len(popup_list()))
let s:old = popup_list()[0]
" Requesting again retires the old menu; selection cannot dispatch it.
let s:c = s:Request()
call assert_equal([], popup_list())
call s:NoExecute()
call s:Reply(s:c, 'correct action')
call popup_close(popup_list()[0], 1)
let s:execution = s:Traced('textDocument/executeAction')
call assert_equal(s:c.id, s:execution.generation)
call assert_equal(s:c.uri, s:execution.uri)
call assert_equal('rust', s:execution.languageId)
call assert_equal(4, s:execution.index)

" Token-bound execution edits are applied once; stale documents reject them.
function! s:EditReply(execution, text) abort
  call s:Call('OnBackendEvent', {'type': 'executeAction', 'id': a:execution.id,
        \ 'generation': a:execution.generation, 'uri': a:execution.uri,
        \ 'edit': {'changes': [{'uri': a:execution.uri, 'edits': [
        \ {'line': 0, 'character': 0, 'end_line': 0, 'end_character': 2,
        \ 'new_text': a:text}]}]}})
endfunction
call s:EditReply(s:execution, 'FN')
call assert_equal('FN one() {}', getline(1))
call s:EditReply(s:execution, 'XX')
call assert_equal('FN one() {}', getline(1), 'execution edit cannot be replayed')
let s:a = s:Request()
call s:Reply(s:a, 'edit after delay')
call popup_close(popup_list()[0], 1)
let s:execution = s:Traced('textDocument/executeAction')
call setline(1, 'fn typed() {}')
call s:EditReply(s:execution, 'XX')
call assert_equal('fn typed() {}', getline(1), 'late execution edit rejects changed text')

" Missing identity and mismatched URI never reach the generic edit handler.
function! s:Execution() abort
  let l:req = s:Request()
  call s:Reply(l:req, 'identity checks')
  call popup_close(popup_list()[0], 1)
  return s:Traced('textDocument/executeAction')
endfunction
let s:execution = s:Execution()
let s:invalid = copy(s:execution)
call remove(s:invalid, 'generation')
let s:edit = {'changes': [{'uri': s:execution.uri, 'edits': [
      \ {'line': 0, 'character': 0, 'end_line': 0, 'end_character': 2,
      \ 'new_text': 'XX'}]}]}
call s:Call('OnBackendEvent', {'type': 'executeAction', 'id': s:invalid.id,
      \ 'uri': s:invalid.uri, 'edit': s:edit})
call assert_equal('fn typed() {}', getline(1), 'missing token cannot apply an edit')
let s:execution = s:Execution()
call s:Call('OnBackendEvent', {'type': 'executeAction', 'id': s:execution.id,
      \ 'generation': s:execution.generation, 'uri': 'file:///wrong.rs', 'edit': s:edit})
call assert_equal('fn typed() {}', getline(1), 'wrong URI cannot apply an edit')
call s:EditReply(s:execution, 'XX')
call assert_equal('fn typed() {}', getline(1), 'rejected response consumes the execution')

" Cursor movement and edits invalidate both replies and an already-open menu.
let s:a = s:Request()
call cursor(2, 1)
call s:Reply(s:a, 'wrong cursor')
call assert_equal([], popup_list())
let s:a = s:Request()
call setline(2, 'fn changed() {}')
call s:Reply(s:a, 'old text')
call assert_equal([], popup_list())
let s:a = s:Request()
call s:Reply(s:a, 'menu before edit')
call setline(2, 'fn changed_again() {}')
call popup_close(popup_list()[0], 1)
call s:NoExecute()

" Changing filetype in place invalidates replies and menu selections.
let s:a = s:Request()
setlocal filetype=python
call s:Reply(s:a, 'wrong language')
call assert_equal([], popup_list())
setlocal filetype=rust
let s:a = s:Request()
call s:Reply(s:a, 'menu from Rust')
setlocal filetype=python
call popup_close(popup_list()[0], 1)
call s:NoExecute()
setlocal filetype=rust

" Null kind is legitimate; absent or negative action index cannot execute.
for s:action in [{'title': 'missing index', 'kind': v:null},
      \ {'title': 'negative index', 'kind': v:null, 'index': -1}]
  let s:a = s:Request()
  call s:Call('OnCodeAction', {'id': s:a.id, 'generation': s:a.id,
        \ 'actions': [s:action]})
  call assert_equal(1, len(popup_list()), 'nullable kind must render a menu')
  call assert_equal([s:action.title], getbufline(winbufnr(popup_list()[0]), 1, '$'))
  call popup_close(popup_list()[0], 1)
  call s:NoExecute()
endfor

" Same-buffer window switches and changed Visual marks invalidate the anchor.
let s:a = s:Request()
split
call s:Reply(s:a, 'wrong split')
call assert_equal([], popup_list())
close
call setpos("'<", [0, 1, 1, 0])
call setpos("'>", [0, 2, 1, 0])
let s:a = s:Request('1,2')
call setpos("'>", [0, 3, 1, 0])
call s:Reply(s:a, 'wrong selection')
call assert_equal([], popup_list())

" Replies without identity never consume the valid pending request.
let s:a = s:Request()
call s:Call('OnCodeAction', {'id': s:a.id, 'actions': [{'title': 'legacy', 'index': 0}]})
call assert_equal([], popup_list())
call s:Reply(s:a, 'identified')
call assert_equal(1, len(popup_list()))
call simplecc#OnPopupContextLeave()
call assert_equal([], popup_list())
call s:Reply(s:a, 'closed context')
call assert_equal([], popup_list())

" Old daemons are rejected explicitly rather than trusting ignored JSON fields.
call s:Call('OnBackendEvent', {'type': 'initialized'})
call writefile([], s:trace)
SimpleCCAction
sleep 30m
call assert_equal([], filter(readfile(s:trace), {_, l -> l =~# '"type":"textDocument/codeAction"'}))
call simplecc#Stop()
call delete(s:daemon)
call delete(s:trace)
call delete(s:file)
if len(v:errors)
  call writefile(v:errors, s:root .. '/test/code-action-identity-errors.log')
  cquit
endif
qall!
