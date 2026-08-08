" Resource operations in a workspace edit.
"
" The create/rename/delete steps of a `documentChanges` array were dropped: the
" daemon answered `applied: false, failureReason: resource create/rename/delete
" operations are not supported` for the whole edit, so every rename-file
" refactor -- rust-analyzer's "move to submodule", tsserver's "move to a new
" file", any "rename symbol" that also renames its file -- did nothing at all
" and did not even say which half was missing.
"
" Run:  vim -Nu NONE -n -i NONE -es -S test/resource_operations.vim

set nocompatible
set encoding=utf-8
set nomore
set noswapfile

let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)
call delete(s:root .. '/test/resource-operations-errors.log')

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

" A daemon that answers `initialize` and records everything sent back to it, so
" the answer this client returns for a server-initiated applyEdit is visible.
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

let s:dir = tempname()
call mkdir(s:dir, 'p')

function! s:Uri(path) abort
  return simplecc#PathToUri(a:path)
endfunction

" ------------------------------------------------------------------ rename ---

" The shape a rename-file refactor really sends: rewrite the importer, then
" move the file the import points at.  Order matters -- the edits are written
" through the buffer for the old path, which stops existing the moment the
" rename runs.
let s:old = s:dir .. '/old.rs'
let s:new = s:dir .. '/moved/new.rs'
let s:user = s:dir .. '/user.rs'
call writefile(['pub fn thing() {}'], s:old)
call writefile(['use crate::old::thing;'], s:user)
" The refactor is invoked from the file that is about to move, so its buffer is
" open -- which is exactly the case that leaves a buffer pointing at a path that
" no longer exists.
execute 'edit! ' .. fnameescape(s:old)

call s:Call('OnBackendEvent', {'type': 'applyEdit', 'server': 'rust-analyzer',
      \ 'requestId': 7, 'edit': {'changes': [], 'operations': [
      \   {'kind': 'edit', 'uri': s:Uri(s:user), 'edits': [
      \     {'line': 0, 'character': 11, 'end_line': 0, 'end_character': 14,
      \      'new_text': 'new'}]},
      \   {'kind': 'rename', 'uri': s:Uri(s:old), 'new_uri': s:Uri(s:new),
      \    'overwrite': v:false, 'ignore_if_exists': v:false}]}})

call assert_equal(0, filereadable(s:old), 'the renamed file is gone from its old path')
call assert_equal(1, filereadable(s:new), 'the renamed file exists at its new path')
call assert_equal(['pub fn thing() {}'], readfile(s:new), 'the contents survive the move')
call assert_equal(['use crate::new::thing;'], getbufline(s:Call('BufnrForPath', s:user), 1, '$'),
      \ 'the text edit that travelled with the rename is applied too')

" A buffer left named after the old path would recreate it on the next :write.
call assert_equal(-1, s:Call('BufnrForPath', s:old),
      \ 'no buffer may still be named after the renamed-away path')
call assert_notequal(-1, s:Call('BufnrForPath', s:new),
      \ 'the moved file is open under its new name')

" The server asked, so it is told what actually happened.
let s:reply = s:Traced('server/response')
call assert_equal(7, s:reply.requestId)
call assert_equal(v:true, s:reply.result.applied,
      \ 'a rename-file refactor must be reported as applied, not refused')

" ------------------------------------------------------------------ create ---

let s:created = s:dir .. '/fresh/made.rs'
call s:Call('OnBackendEvent', {'type': 'applyEdit', 'edit': {'operations': [
      \ {'kind': 'create', 'uri': s:Uri(s:created),
      \  'overwrite': v:false, 'ignore_if_exists': v:false},
      \ {'kind': 'edit', 'uri': s:Uri(s:created), 'edits': [
      \   {'line': 0, 'character': 0, 'end_line': 0, 'end_character': 0,
      \    'new_text': 'fn made() {}'}]}]}})
call assert_equal(1, filereadable(s:created),
      \ 'create must make the file, including the directory leading to it')
call assert_equal('fn made() {}', getbufline(s:Call('BufnrForPath', s:created), 1, '$')[0],
      \ 'the edit that fills a freshly created file in is applied after it')

" An existing file is never truncated unless the server says overwrite.
call writefile(['keep me'], s:created)
call s:Call('OnBackendEvent', {'type': 'applyEdit', 'edit': {'operations': [
      \ {'kind': 'create', 'uri': s:Uri(s:created),
      \  'overwrite': v:false, 'ignore_if_exists': v:true}]}})
call assert_equal(['keep me'], readfile(s:created),
      \ 'create with ignoreIfExists must leave the existing file alone')

" ------------------------------------------------------------------ delete ---

let s:doomed = s:dir .. '/doomed.rs'
call writefile(['gone'], s:doomed)
execute 'edit! ' .. fnameescape(s:doomed)
call s:Call('OnBackendEvent', {'type': 'applyEdit', 'edit': {'operations': [
      \ {'kind': 'delete', 'uri': s:Uri(s:doomed),
      \  'recursive': v:false, 'ignore_if_not_exists': v:false}]}})
call assert_equal(0, filereadable(s:doomed), 'delete removes the file')
call assert_equal(-1, s:Call('BufnrForPath', s:doomed),
      \ 'a buffer for a deleted file would write it straight back')

" A delete of something that is not there is an error unless it says otherwise.
call writefile([], s:trace)
call s:Call('OnBackendEvent', {'type': 'applyEdit', 'server': 'rust-analyzer',
      \ 'requestId': 8, 'edit': {'operations': [
      \ {'kind': 'delete', 'uri': s:Uri(s:dir .. '/never-existed.rs'),
      \  'recursive': v:false, 'ignore_if_not_exists': v:false}]}})
let s:reply = s:Traced('server/response')
call assert_equal(v:false, s:reply.result.applied)
call assert_equal(1, s:reply.result.failureReason =~# 'does not exist',
      \ 'a failing operation names itself: ' .. string(s:reply.result))

" ------------------------------------------------------- opt out, and skew ---

" g:simplecc_resource_operations = 0 refuses the whole edit rather than
" applying half of a refactor that moves files.
let s:kept = s:dir .. '/kept.rs'
call writefile(['still here'], s:kept)
let g:simplecc_resource_operations = 0
call writefile([], s:trace)
call s:Call('OnBackendEvent', {'type': 'applyEdit', 'server': 'rust-analyzer',
      \ 'requestId': 9, 'edit': {'operations': [
      \ {'kind': 'delete', 'uri': s:Uri(s:kept)}]}})
call assert_equal(1, filereadable(s:kept),
      \ 'g:simplecc_resource_operations = 0 must not delete anything')
let s:reply = s:Traced('server/response')
call assert_equal(v:false, s:reply.result.applied)
let g:simplecc_resource_operations = 1

" An older lib/simplecc-daemon sends no `operations` key at all; the flat
" `changes` list must still be applied rather than mistaken for an empty edit.
let s:legacy = s:dir .. '/legacy.rs'
call writefile(['old text'], s:legacy)
execute 'edit! ' .. fnameescape(s:legacy)
call s:Call('OnBackendEvent', {'type': 'applyEdit', 'edit': {'changes': [
      \ {'uri': s:Uri(s:legacy), 'edits': [
      \   {'line': 0, 'character': 0, 'end_line': 0, 'end_character': 8,
      \    'new_text': 'new text'}]}]}})
call assert_equal(['new text'], getbufline(s:Call('BufnrForPath', s:legacy), 1, '$'),
      \ 'a daemon that sends only `changes` must keep working')

call simplecc#Stop()
call s:Wait("g:simplecc_status ==# ''", 2000)
call delete(s:daemon)
call delete(s:trace)
call delete(s:dir, 'rf')

if len(v:errors)
  call writefile(v:errors, s:root .. '/test/resource-operations-errors.log')
  for s:e in v:errors
    echomsg s:e
  endfor
  cquit
endif
qall!
