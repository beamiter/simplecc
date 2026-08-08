" Empty replies must clear, not be ignored.
"
" Both OnInlayHints and OnSemanticTokens returned early on an empty reply,
" before the code that removes the previous text properties. So deleting the
" line that produced an inlay hint left its `: i32` attached to whatever
" replaced it -- and RestoreInlayHints() re-added it from the cache on every
" CursorHold -- while select-all-and-delete left semantic-token highlights
" describing code that no longer existed.
"
" Run:  vim -Nu NONE -n -i NONE -es -S test/stale_props.vim

set nocompatible
set encoding=utf-8
set nomore

let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)
call delete(s:root .. '/test/stale-props-errors.log')

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

" prop_list() filters by `types` (a list) on newer Vim and silently ignores an
" unknown option key, so filter here instead of trusting the option.
function! s:Props(type) abort
  return filter(prop_list(1, {'bufnr': bufnr('%')}), {_, p -> p.type ==# a:type})
endfunction

" A daemon that answers `initialize` is all this needs: the replies are fed in
" by hand so the test controls exactly which snapshot is answered.
let s:daemon = tempname()
call writefile(readfile(s:root .. '/test/fake_daemon.sh'), s:daemon)
call assert_equal(1, setfperm(s:daemon, 'rwx------'))
let g:simplecc_daemon_path = s:daemon
call simplecc#Start()
call assert_equal(1, s:Wait("g:simplecc_status ==# 'ready'", 2000))

let s:file = tempname() .. '.rs'
call writefile(['let x = compute();'], s:file)
execute 'edit! ' .. fnameescape(s:file)
setfiletype rust

" ------------------------------------------------------------- inlay hints ---

" NextId() is deterministic, so the id the request is about to take is known.
let s:id = s:Call('NextId') + 1
call s:Call('RequestInlayHints')
call s:Call('OnBackendEvent', {'type': 'inlayHint', 'id': s:id,
      \ 'hints': [{'line': 0, 'character': 5, 'label': ': i32'}]})
call assert_equal(1, len(s:Props('SimpleCCInlay')), 'the hint is rendered')

" The line is gone; the server answers with no hints at all.
let s:id = s:Call('NextId') + 1
call s:Call('RequestInlayHints')
call s:Call('OnBackendEvent', {'type': 'inlayHint', 'id': s:id, 'hints': []})
call assert_equal([], s:Props('SimpleCCInlay'),
      \ 'an empty inlay-hint reply must clear the hints that were there')

" ...and CursorHold must not put them back from the cache.
call s:Call('RestoreInlayHints')
call assert_equal([], s:Props('SimpleCCInlay'),
      \ 'a cleared hint must not be restored from the cache')

" --------------------------------------------------------- semantic tokens ---

let s:id = s:Call('NextId') + 1
call simplecc#SemanticTokens()
call s:Call('OnBackendEvent', {'type': 'semanticTokens', 'id': s:id,
      \ 'tokens': [{'line': 0, 'start': 4, 'length': 1,
      \             'token_type': 'variable', 'modifiers': []}]})
call assert_equal(1, len(s:Props('SimpleCCSemanticVariable')),
      \ 'the token is highlighted')

let s:id = s:Call('NextId') + 1
call simplecc#SemanticTokens()
call s:Call('OnBackendEvent', {'type': 'semanticTokens', 'id': s:id, 'tokens': []})
call assert_equal([], s:Props('SimpleCCSemanticVariable'),
      \ 'an empty semantic-token reply must remove the previous highlights')

call simplecc#Stop()
call s:Wait("g:simplecc_status ==# ''", 2000)
call delete(s:daemon)
call delete(s:file)

if len(v:errors)
  call writefile(v:errors, s:root .. '/test/stale-props-errors.log')
  for s:e in v:errors
    echomsg s:e
  endfor
  cquit
endif
qall!
