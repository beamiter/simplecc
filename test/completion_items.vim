" Completion: how an LSP item becomes a Vim popup-menu entry.
"
" The daemon ranks by sortText before truncating to maxItems; this file covers
" the Vim half of the same decision -- filterText and preselect. Both were
" carried across the wire and then dropped on the floor: an item whose
" filterText differs from its inserted word disappeared from the menu on the
" next keystroke (Vim filters on `word`), and a preselected item was never
" selected because the menu always carried `noselect`.
"
" Run:  vim -Nu NONE -n -i NONE -es -S test/completion_items.vim

set nocompatible
set encoding=utf-8
set nomore

let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)
call delete(s:root .. '/test/completion-items-errors.log')

let g:simplecc_auto_start = 0
let g:simplecc_daemon_path = '/nonexistent/simplecc-daemon'
let g:simplecc_no_default_maps = 1
runtime plugin/simplecc.vim
execute 'source ' .. fnameescape(s:root .. '/autoload/simplecc.vim')

let s:sid = getscriptinfo({'name': 'autoload/simplecc.vim'})[0].sid

function! s:Build(items) abort
  let l:existing = {}
  let l:built = call(function(printf('<SNR>%d_ServerCompletionItems', s:sid)),
        \ [a:items, 7, l:existing])
  return [l:built, l:existing]
endfunction

function! s:Completeopt(preselect) abort
  return call(function(printf('<SNR>%d_CompletionCompleteopt', s:sid)), [a:preselect])
endfunction

" ------------------------------------------------------------- filterText ---

" A rust-analyzer postfix completion: the user types "dbg", the server inserts
" something else entirely and says what to match on through filterText.
let [s:built, s:existing] = s:Build([
      \ {'label': 'dbg', 'insert_text': 'dbg!(value)',
      \  'filter_text': 'dbg', 'index': 0},
      \ {'label': 'len', 'insert_text': 'len', 'filter_text': 'len', 'index': 1},
      \ ])
call assert_equal(['dbg!(value)', 'len'], map(copy(s:built), {_, v -> v.word}))
call assert_equal(1, get(s:built[0], 'equal', 0),
      \ 'an item whose filterText differs from its word must not be filtered by Vim')
call assert_equal(0, get(s:built[1], 'equal', 0),
      \ 'an item Vim can match itself keeps normal filtering')
call assert_equal({'dbg!(value)': v:true, 'len': v:true}, s:existing)

" Turning the ranking hints off restores the raw, unannotated mapping.
let g:simplecc_complete_sort = 0
let [s:built, s:existing] = s:Build([
      \ {'label': 'dbg', 'insert_text': 'dbg!(value)',
      \  'filter_text': 'dbg', 'index': 0},
      \ ])
call assert_equal(0, get(s:built[0], 'equal', 0),
      \ 'g:simplecc_complete_sort = 0 opts out of the filterText hint')
let g:simplecc_complete_sort = 1

" --------------------------------------------------------------- preselect ---

" The server's preselected item has to end up where Vim's selection lands,
" and the menu must drop `noselect` so <CR> accepts it.
let [s:built, s:existing] = s:Build([
      \ {'label': 'alpha', 'insert_text': 'alpha', 'index': 0},
      \ {'label': 'beta', 'insert_text': 'beta', 'index': 1, 'preselect': v:true},
      \ {'label': 'gamma', 'insert_text': 'gamma', 'index': 2},
      \ ])
call assert_equal(['beta', 'alpha', 'gamma'], map(copy(s:built), {_, v -> v.word}),
      \ 'the preselected item moves to the position Vim actually selects')
call assert_equal(1, s:built[0].user_data.index,
      \ 'reordering must not disturb the resolve index')
call assert_equal('menu,menuone,noinsert', s:Completeopt(v:true))
call assert_equal('menu,menuone,noselect,noinsert', s:Completeopt(v:false))

" Without preselect the ranked order is preserved exactly as the daemon sent it.
let [s:built, s:existing] = s:Build([
      \ {'label': 'alpha', 'insert_text': 'alpha', 'index': 0},
      \ {'label': 'beta', 'insert_text': 'beta', 'index': 1},
      \ ])
call assert_equal(['alpha', 'beta'], map(copy(s:built), {_, v -> v.word}))

" --------------------------------------------------------------- max_items ---

let g:simplecc_complete_max_items = 2
let [s:built, s:existing] = s:Build([
      \ {'label': 'a', 'insert_text': 'a', 'index': 0},
      \ {'label': 'b', 'insert_text': 'b', 'index': 1},
      \ {'label': 'c', 'insert_text': 'c', 'index': 2},
      \ ])
call assert_equal(['a', 'b'], map(copy(s:built), {_, v -> v.word}))
let g:simplecc_complete_max_items = 100

" --------------------------------------------------------------- byte vs char ---

" col() is a byte column; Vim9 string slices are character indexes.  A CJK
" character before the identifier used to shift the prefix walk so the server
" saw '中abc' (or worse) instead of 'abc', and complete() replaced from the
" wrong byte.
function! s:Ctx(text, col) abort
  return call(function(printf('<SNR>%d_CompletionContext', s:sid)), [a:text, a:col])
endfunction

" 'x：abc' — fullwidth colon is 3 bytes and not 'iskeyword', so the identifier
" is just 'abc'.  Cursor after c is byte column 8.
let s:line = 'x：abc'
call assert_equal(8, strlen(s:line) + 1)
let s:ctx = s:Ctx(s:line, 8)
call assert_equal(v:true, s:ctx.ok)
call assert_equal('abc', s:ctx.prefix,
      \ 'the prefix is the keyword in bytes, not characters: ' .. string(s:ctx))
call assert_equal(4, s:ctx.start,
      \ 'complete() must replace from the first byte of the identifier')
call assert_equal('：', s:ctx.trigger)

" Trigger-only path with a multi-byte character before the dot: 'x：.abc'
let s:dotted = 'x：.abc'
call assert_equal(9, strlen(s:dotted) + 1)
let s:ctx = s:Ctx(s:dotted, 9)
call assert_equal('abc', s:ctx.prefix)
call assert_equal('.', s:ctx.trigger,
      \ 'the trigger is the punctuation immediately before the keyword')

if len(v:errors)
  call writefile(v:errors, s:root .. '/test/completion-items-errors.log')
  for s:e in v:errors
    echomsg s:e
  endfor
  cquit
endif
qall!
