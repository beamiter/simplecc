" Inlay hints are scoped to the viewport, and semantic tokens are batched.
"
" textDocument/inlayHint takes a range and this asked for `0 .. line('$')`
" every time, so opening a 20k-line file made the server compute -- and this
" process decode and turn into text properties -- tens of thousands of hints to
" render the twenty-odd that fit on screen. Semantic tokens meanwhile issued
" one prop_add() per token, each in its own try/catch.
"
" Run:  vim -Nu NONE -n -i NONE -es -S test/viewport_hints.vim

set nocompatible
set encoding=utf-8
set nomore
set noswapfile

let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)
call delete(s:root .. '/test/viewport-hints-errors.log')

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

function! s:Props(type) abort
  let l:found = []
  for l:lnum in range(1, line('$'))
    call extend(l:found, filter(prop_list(l:lnum, {'bufnr': bufnr('%')}),
          \ {_, p -> p.type ==# a:type}))
  endfor
  return l:found
endfunction

" A daemon that answers `initialize` and records every later request, so the
" range actually asked for is visible.
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

" A file far taller than any window, so viewport and document differ.
let s:file = tempname() .. '.rs'
call writefile(map(range(1, 2000), {_, n -> printf('let v%d = compute();', n)}), s:file)
execute 'edit! ' .. fnameescape(s:file)
setfiletype rust

" --------------------------------------------------------- viewport request ---

call writefile([], s:trace)
normal! 1000G
normal! zz
redraw
call s:Call('RequestInlayHints')
let s:msg = s:Traced('textDocument/inlayHint')

call assert_notequal({}, s:msg, 'an inlay-hint request was sent')
call assert_equal(1, s:msg.startLine > 0,
      \ 'hints for line 1000 must not be requested from the top of the file: '
      \ .. string([s:msg.startLine, s:msg.endLine]))
call assert_equal(1, s:msg.endLine < 1999,
      \ 'hints must not be requested through to the end of the file: '
      \ .. string([s:msg.startLine, s:msg.endLine]))
" The margin is what makes a small scroll not need a round-trip.
call assert_equal(1, s:msg.startLine <= line('w0') - 1 - 1,
      \ 'the request must reach above the viewport')
call assert_equal(1, s:msg.endLine >= line('w$') - 1 + 1,
      \ 'the request must reach below the viewport')

" g:simplecc_inlay_margin widens it; 0 asks for exactly what is on screen.
let g:simplecc_inlay_margin = 0
call writefile([], s:trace)
call s:Call('RequestInlayHints')
let s:msg = s:Traced('textDocument/inlayHint')
" `vim -es` has no window on a screen, so line('w$') can come out just below
" line('w0'); the range is clamped so it never inverts.
call assert_equal([line('w0') - 1, max([line('w0'), line('w$')]) - 1],
      \ [s:msg.startLine, s:msg.endLine],
      \ 'g:simplecc_inlay_margin = 0 requests the viewport and nothing else')
let g:simplecc_inlay_margin = 100

" Scrolling is what fetches the part that just became visible.
call writefile([], s:trace)
call simplecc#OnWinScrolled()
call assert_equal(1, s:Wait('!empty(filter(readfile(' .. string(s:trace) ..
      \ '), {_, l -> l =~# ''"type":"textDocument/inlayHint"''}))', 2000),
      \ 'scrolling must re-request hints for the newly visible lines')

" -------------------------------------------------- batched semantic tokens ---

" Several tokens of several types in one reply: with the per-type batching they
" all have to survive, including the ones after the first in each batch.
let s:id = s:Call('NextId') + 1
call simplecc#SemanticTokens()
call s:Call('OnBackendEvent', {'type': 'semanticTokens', 'id': s:id, 'tokens': [
      \ {'line': 999, 'start': 0, 'length': 3, 'token_type': 'keyword', 'modifiers': []},
      \ {'line': 999, 'start': 4, 'length': 5, 'token_type': 'variable', 'modifiers': []},
      \ {'line': 1000, 'start': 0, 'length': 3, 'token_type': 'keyword', 'modifiers': []},
      \ {'line': 1000, 'start': 4, 'length': 5, 'token_type': 'variable', 'modifiers': []}]})
call assert_equal(2, len(s:Props('SimpleCCSemanticKeyword')),
      \ 'every token of a type must be added, not just the first of its batch')
call assert_equal(2, len(s:Props('SimpleCCSemanticVariable')))
call assert_equal([1, 3], [s:Props('SimpleCCSemanticKeyword')[0].col,
      \ s:Props('SimpleCCSemanticKeyword')[0].length],
      \ 'a batched property keeps the column and length of its token')

call simplecc#Stop()
call s:Wait("g:simplecc_status ==# ''", 2000)
call delete(s:daemon)
call delete(s:trace)
call delete(s:file)

if len(v:errors)
  call writefile(v:errors, s:root .. '/test/viewport-hints-errors.log')
  for s:e in v:errors
    echomsg s:e
  endfor
  cquit
endif
qall!
