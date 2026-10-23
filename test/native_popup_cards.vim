" Exercise actual passive popup callbacks with no LSP process or GUI host.
set nocompatible
set encoding=utf-8
set nomore
let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
call delete(s:root .. '/test/native-popup-cards-errors.log')
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

let g:jterm_vim9_render_protocol = 7
let g:jterm_vim9_popup_cards = v:true
set columns=40 lines=30 tabstop=4
let s:source = [bufnr(), win_getid(), getcurpos(), getline(1, '$'), &modified]
let g:card_events = 0
augroup popup_card_test
  autocmd!
  autocmd User JTermVim9PopupChanged let g:card_events += 1
augroup END
let s:unicode = repeat('界é', 25) . "\t" . '**literal markdown**'
call s:Call('OnHover', {'contents': s:unicode . "\n\n" . repeat("line\n", 35)})
let s:id = popup_list()[0]
call assert_equal('hover', getwinvar(s:id, 'jterm_native_popup_kind'))
call assert_equal(1, getwinvar(s:id, 'jterm_native_popup'))
call assert_equal([0, 0, 0, 0], get(popup_getoptions(s:id), 'border', [0, 0, 0, 0]))
call assert_equal([1, 2, 1, 2], popup_getoptions(s:id).padding)
call assert_equal(0, popup_getoptions(s:id).wrap)
call assert_equal('markdown', getbufvar(winbufnr(s:id), '&syntax'))
call assert_equal('SimpleCCPopupCard', popup_getoptions(s:id).highlight)
let s:lines = getbufline(winbufnr(s:id), 1, '$')
call assert_true(index(s:lines, '') >= 0, 'preserves empty lines')
call assert_equal(substitute(s:unicode, '[ \t]', '', 'g'), substitute(join(s:lines[:index(s:lines, '')-1], ''), '[ \t]', '', 'g'))
for s:line in s:lines
  call assert_true(strdisplaywidth(s:line) <= 36, string(s:line))
  call assert_notequal('́', strcharpart(s:line, 0, 1, v:true), 'combining mark stays with its base')
endfor
call assert_true(simplecc#PopupVisible())
call assert_equal("\<Cmd>SimpleCCPopupScroll 3\<CR>", simplecc#PopupScrollKey(1, "\<C-f>"))
let s:before = popup_getpos(s:id).firstline
SimpleCCPopupScroll 3
call assert_equal(s:before + 3, popup_getpos(s:id).firstline)
SimpleCCPopupScroll -3
call assert_equal(s:before, popup_getpos(s:id).firstline)
" Tabs are expanded at source columns, including vartabstop, before wrapping.
set vartabstop=4,8
let s:tabtext = "界\tA\tB\t" . repeat('界', 30)
call s:Call('OnHover', {'contents': s:tabtext . "\n" . repeat("long\n", 40)})
let s:id = popup_list()[0]
for s:line in getbufline(winbufnr(s:id), 1, '$')
  call assert_true(strdisplaywidth(s:line) <= popup_getpos(s:id).core_width)
  call assert_equal(-1, stridx(s:line, "\t"))
endfor
call assert_equal(['    X'], s:Call('PopupCardLines', ["\tX"], 9))
call assert_equal([' ', ' ', ' ', ' ', 'X'], s:Call('PopupCardLines', ["\tX"], 1))
set vartabstop=
let s:other = popup_create(['unrelated picker'], {})
SimpleCCPopupClose
call assert_equal([s:other], popup_list())
call assert_false(simplecc#PopupVisible())
call assert_equal("\<C-b>", simplecc#PopupScrollKey(-1, "\<C-b>"))
call popup_close(s:other)
call s:Call('OnSignatureHelp', {'signatures': [{'label': 'alpha(value)'}]})
call assert_equal('signature', getwinvar(popup_list()[0], 'jterm_native_popup_kind'))
SimpleCCPopupClose
call s:Call('CreateTextPopup', ['docs'], {'maxwidth': 60, 'maxheight': 15}, 'completion_documentation', v:false)
call assert_equal('completion_documentation', getwinvar(popup_list()[0], 'jterm_native_popup_kind'))
call popup_clear()
let s:file = tempname() .. '.rs'
call writefile(['let a = 1;'], s:file)
execute 'edit ' .. fnameescape(s:file)
let s:source = [bufnr(), win_getid(), getcurpos(), getline(1, '$'), &modified]
call s:Call('OnDiagnostics', {'uri': s:file, 'server': 'fixture',
      \ 'items': [{'line': 0, 'character': 0, 'severity': 1, 'message': 'diagnostic card'}]})
SimpleCCDiag
call assert_equal('diagnostic', getwinvar(popup_list()[0], 'jterm_native_popup_kind'))
SimpleCCPopupClose
call delete(s:file)
call assert_equal(s:source, [bufnr(), win_getid(), getcurpos(), getline(1, '$'), &modified])
call s:Call('OnHover', {'contents': 'keep hover'})
call simplecc#OnInsertLeave()
call assert_true(simplecc#PopupVisible(), 'insert leave preserves a real hover')
call setwinvar(popup_list()[0], 'simplecc_popup_kind', 'completion_documentation')
call simplecc#OnInsertLeave()
call assert_false(simplecc#PopupVisible(), 'insert leave closes completion docs')
call s:Call('OnHover', {'contents': 'docs'})
call setwinvar(popup_list()[0], 'simplecc_popup_kind', 'completion_documentation')
call simplecc#OnCompleteDone()
call assert_false(simplecc#PopupVisible(), 'complete done closes completion docs')
let g:simplecc_popup_cards = 0
call s:Call('OnHover', {'contents': 'opt out'})
call assert_equal([0, 1, 0, 1], popup_getoptions(popup_list()[0]).padding)
SimpleCCPopupClose
let g:simplecc_popup_cards = 1
let g:jterm_vim9_render_protocol = 6
call s:Call('OnHover', {'contents': 'compat'})
call assert_equal([0, 1, 0, 1], popup_getoptions(popup_list()[0]).padding)
call assert_equal('', getwinvar(popup_list()[0], 'jterm_native_popup_kind', ''))
SimpleCCPopupClose
let g:jterm_vim9_render_protocol = 7
let g:jterm_vim9_popup_cards = v:false
call s:Call('OnHover', {'contents': 'compat'})
call assert_equal([0, 1, 0, 1], popup_getoptions(popup_list()[0]).padding)
SimpleCCPopupClose
if !empty(v:errors)
  call writefile(v:errors, s:root .. '/test/native-popup-cards-errors.log')
  cquit
endif
qall!
