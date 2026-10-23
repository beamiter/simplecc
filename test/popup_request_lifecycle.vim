" Real request entry points, deterministic late replies, actual Vim popups.
set nocompatible
set encoding=utf-8
set nomore
set hidden
let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)
let g:simplecc_auto_start = 0
let g:simplecc_no_default_maps = 1
let g:simplecc_auto_complete = 0
let g:simplecc_daemon_path = '/nonexistent/simplecc-daemon'
runtime plugin/simplecc.vim
runtime autoload/simplecc.vim
let s:sid = getscriptinfo({'name': 'autoload/simplecc.vim'})[0].sid
function! s:Call(name, ...) abort
  return call(function(printf('<SNR>%d_%s', s:sid, a:name)), a:000)
endfunction
function! s:Vars() abort
  return getscriptinfo({'sid': s:sid})[0].variables
endfunction
function! s:Request(kind) abort
  if a:kind ==# 'hover'
    call simplecc#Hover()
  else
    call simplecc#SignatureHelp()
  endif
  return s:Vars().s_next_id
endfunction
function! s:Reply(kind, id) abort
  let event = {'id': a:id, 'type': a:kind ==# 'hover' ? 'hover' : 'signatureHelp'}
  if a:kind ==# 'hover'
    let event.contents = 'Current hover documentation'
  else
    let event.signatures = [{'label': 'current_signature(value)'}]
  endif
  call s:Call('OnBackendEvent', event)
endfunction
function! s:Absent(kind, id, message) abort
  call s:Reply(a:kind, a:id)
  call assert_equal([], popup_list(), a:message)
endfunction
call delete(s:root .. '/test/popup-request-lifecycle-errors.log')
call s:Call('OnBackendEvent', {'type': 'initialized'})
call setline(1, ['alpha beta', 'gamma delta'])
let s:source = bufnr()
for s:kind in ['hover', 'signature']
  call cursor(1, 1)
  let s:older = s:Request(s:kind)
  let s:latest = s:Request(s:kind)
  call s:Absent(s:kind, s:older, 'out-of-order old reply is ignored')
  call s:Reply(s:kind, s:latest)
  call assert_equal(1, len(popup_list()), 'newest request survives an older reply')
  call simplecc#PopupClose()
  call s:Absent(s:kind, s:latest, 'a consumed reply cannot reopen its popup')

  let s:id = s:Request(s:kind)
  call cursor(2, 1)
  call s:Absent(s:kind, s:id, 'cursor movement invalidates the source snapshot')
  call cursor(1, 1)

  let s:id = s:Request(s:kind)
  call cursor(1, 4)
  call simplecc#OnCursorMoved()
  call cursor(1, 1)
  call s:Absent(s:kind, s:id, 'moving away and back does not revive a request')

  let s:id = s:Request(s:kind)
  call setline(1, 'changed alpha')
  call s:Absent(s:kind, s:id, 'editing invalidates the source snapshot')

  let s:id = s:Request(s:kind)
  call simplecc#PopupClose()
  call s:Absent(s:kind, s:id, 'manual close also cancels a pending request')

  let s:id = s:Request(s:kind)
  call simplecc#OnInsertLeave()
  call s:Absent(s:kind, s:id, 'leaving insert mode cancels a pending request')

  let s:id = s:Request(s:kind)
  split
  call s:Absent(s:kind, s:id, 'another split of the same buffer is not the request window')
  close
  call s:Absent(s:kind, s:id, 'returning to the old window cannot revive its request')

  let s:id = s:Request(s:kind)
  enew
  call s:Absent(s:kind, s:id, 'another buffer is not the requested document')
  execute 'buffer ' .. s:source

  let s:id = s:Request(s:kind)
  call s:Call('OnBackendEvent', {'type': 'error', 'id': s:id, 'message': 'fixture error'})
  call s:Absent(s:kind, s:id, 'failed request cannot produce a later popup')

  let s:id = s:Request(s:kind)
  call s:Reply(s:kind, s:id)
  let s:popup = popup_list()[0]
  let s:pending = s:Request(s:kind)
  call popup_close(s:popup)
  call s:Absent(s:kind, s:pending, 'popup dismissal invalidates its pending refresh')

  for s:i in range(1, 100)
    call s:Request(s:kind)
  endfor
  call assert_true(len(s:Vars().s_popup_requests) <= 2, 'pending popup state is bounded')
  call simplecc#PopupClose()
endfor
" Empty valid refreshes retire a hover, but not documentation in the shared slot.
let g:empty_hover_events = 0
augroup empty_hover_lifecycle_test
  autocmd!
  autocmd User JTermVim9PopupChanged let g:empty_hover_events += 1
augroup END
let s:older = s:Request('hover')
let s:latest = s:Request('hover')
call s:Reply('hover', s:latest)
let s:popup = popup_list()[0]
let s:events = g:empty_hover_events
call s:Call('OnBackendEvent', {'type': 'hover', 'id': s:older, 'contents': ''})
call assert_equal([s:popup], popup_list(), 'old empty reply must preserve the new hover')
call assert_equal(s:events, g:empty_hover_events)
let s:id = s:Request('hover')
call s:Call('OnBackendEvent', {'type': 'hover', 'id': s:id, 'contents': ''})
call assert_equal([], popup_list(), 'valid empty hover refresh removes obsolete documentation')
call assert_equal(s:events + 1, g:empty_hover_events, 'empty refresh sends one native close event')
let s:id = s:Request('hover')
call s:Reply('hover', s:id)
let s:popup = popup_list()[0]
" Completion documentation intentionally reuses this exact internal hover slot.
call setwinvar(s:popup, 'simplecc_popup_kind', 'completion_documentation')
let s:events = g:empty_hover_events
let s:id = s:Request('hover')
call s:Call('OnBackendEvent', {'type': 'hover', 'id': s:id, 'contents': v:null})
call assert_equal([s:popup], popup_list(), 'empty hover must preserve completion documentation')
call assert_equal(s:events, g:empty_hover_events, 'preserved completion docs do not emit a close')
call simplecc#PopupClose()
" Independent kinds retain their own newest request.
let s:hover = s:Request('hover')
let s:sig = s:Request('signature')
call s:Reply('hover', s:hover)
call s:Reply('signature', s:sig)
call assert_equal(2, len(popup_list()), 'independent valid requests can both render')
call simplecc#PopupClose()
let s:id = s:Request('hover')
call s:Call('OnDaemonExit', 1, v:false)
call s:Absent('hover', s:id, 'daemon exit invalidates pending popup state')
call assert_equal({}, s:Vars().s_popup_requests)
if !empty(v:errors)
  call writefile(v:errors, s:root .. '/test/popup-request-lifecycle-errors.log')
  cquit
endif
qall!
