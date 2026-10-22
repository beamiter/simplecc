" Exercise actual passive popup callbacks with no LSP process or GUI host.
set nocompatible
set encoding=utf-8
set nomore
let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
call delete(s:root .. '/test/native-popups-errors.log')
execute 'set runtimepath^=' .. fnameescape(s:root)
let g:simplecc_auto_start = 0
let g:simplecc_no_default_maps = 1
let g:simplecc_daemon_path = '/nonexistent/simplecc-daemon'
runtime plugin/simplecc.vim
runtime autoload/simplecc.vim
let s:sid = getscriptinfo({'name': 'autoload/simplecc.vim'})[0].sid

function! s:Call(name, ...) abort
  return call(function(printf('<SNR>%d_%s', s:sid, a:name)), a:000)
endfunction

function! s:Popup() abort
  let ids = popup_list()
  call assert_equal(1, len(ids))
  if empty(ids)
    return -1
  endif
  call assert_equal(1, getwinvar(ids[0], 'jterm_native_popup', 0))
  " Vim may normalize the all-sides border to [] (the documented shorthand).
  call assert_true(index([[], [1, 1, 1, 1]], popup_getoptions(ids[0]).border) >= 0)
  call assert_equal([0, 1, 0, 1], popup_getoptions(ids[0]).padding)
  return ids[0]
endfunction

let s:file = tempname() .. '.rs'
call writefile(['let alpha = 1;'], s:file)
execute 'edit ' .. fnameescape(s:file)
let s:source = [bufnr(), win_getid(), getcurpos(), getline(1, '$'), &modified]

" A disconnected/ordinary Vim session still uses its normal popup window.
call s:Call('OnHover', {'contents': '# 文档' . "\n" . 'alpha()'})
let s:hover = s:Popup()
call assert_equal('markdown', getbufvar(winbufnr(s:hover), '&filetype'))
call assert_equal(['# 文档', 'alpha()'], getbufline(winbufnr(s:hover), 1, '$'))
call s:Call('ShowCompletionDocumentation', {})
call assert_equal([], popup_list(), 'ordinary Vim closes without a host subscription')

" Native hosts subscribe to a cheap redraw event after the surface is ready.
let g:native_popup_events = 0
augroup simplecc_native_popup_test
  autocmd!
  autocmd User JTermVim9PopupChanged let g:native_popup_events += 1
augroup END
call s:Call('OnHover', {'contents': 'native hover'})
call s:Popup()
call assert_equal(1, g:native_popup_events)
call s:Call('ShowCompletionDocumentation', {})
call assert_equal([], popup_list())
call assert_equal(2, g:native_popup_events, 'empty completion documentation notifies its close')
call s:Call('OnSignatureHelp', {'signatures': [{'label': 'alpha(value)',
      \ 'documentation': 'signature help'}]})
call s:Popup()
call assert_equal(3, g:native_popup_events)
call s:Call('OnSignatureHelp', {'signatures': [{'label': 'alpha(value, options)'}]})
call s:Popup()
call assert_equal(4, g:native_popup_events, 'signature replacement sends one ready event')
call s:Call('OnSignatureHelp', {'signatures': []})
call assert_equal([], popup_list())
call assert_equal(5, g:native_popup_events, 'empty signature response notifies its close')
call s:Call('OnSignatureHelp', {'signatures': []})
call assert_equal(5, g:native_popup_events, 'closing an already absent signature is a no-op')
call s:Call('OnDiagnostics', {'uri': s:file, 'server': 'fixture',
      \ 'items': [{'line': 0, 'character': 0, 'severity': 1, 'message': 'native diagnostic'}]})
call simplecc#Diag()
call s:Popup()
call assert_equal(6, g:native_popup_events)
call s:Call('OnDiagnostics', {'uri': s:file, 'server': 'fixture', 'items': []})
call simplecc#Diag()
call assert_equal([], popup_list())
call assert_equal(7, g:native_popup_events, 'empty diagnostic detail notifies its close')
call assert_equal(s:source, [bufnr(), win_getid(), getcurpos(), getline(1, '$'), &modified],
      \ 'displaying plugin documentation must not edit or focus the source')
call popup_clear()
call delete(s:file)
if !empty(v:errors)
  call writefile(v:errors, s:root .. '/test/native-popups-errors.log')
  cquit
endif
qall!
