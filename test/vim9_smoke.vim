set nocompatible
set encoding=utf-8

let s:root = fnamemodify(expand('<sfile>:p'), ':h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)

" The plugin must not overwrite mappings that were defined by the user.
let g:simplecc_auto_start = 0
let g:simplecc_sign_error = 'X|let g:simplecc_sign_definition_injected=1|'
nnoremap <silent> gd <Cmd>let g:simplecc_existing_mapping_ran = 1<CR>
runtime plugin/simplecc.vim
execute 'source ' .. fnameescape(s:root .. '/autoload/simplecc.vim')
defcompile

call assert_match('simplecc_existing_mapping_ran', maparg('gd', 'n'))
call assert_match('SimpleCCDefinition', maparg('<Plug>(simplecc-definition)', 'n'))
call assert_match('SimpleCCDiag', maparg('<Plug>(simplecc-show-diagnostic)', 'n'))
call assert_false(exists('g:simplecc_sign_definition_injected'))

" LSP positions are UTF-16 code units; Vim columns and string slices are bytes.
let s:text = 'a中😀z'
call assert_equal(0, simplecc#ByteOffsetToUtf16(s:text, 0))
call assert_equal(1, simplecc#ByteOffsetToUtf16(s:text, 1))
call assert_equal(1, simplecc#ByteOffsetToUtf16(s:text, 2))
call assert_equal(2, simplecc#ByteOffsetToUtf16(s:text, 4))
call assert_equal(4, simplecc#ByteOffsetToUtf16(s:text, 8))
call assert_equal(5, simplecc#ByteOffsetToUtf16(s:text, 999))
call assert_equal(0, simplecc#Utf16ToByteOffset(s:text, 0))
call assert_equal(1, simplecc#Utf16ToByteOffset(s:text, 1))
call assert_equal(4, simplecc#Utf16ToByteOffset(s:text, 2))
call assert_equal(4, simplecc#Utf16ToByteOffset(s:text, 3))
call assert_equal(8, simplecc#Utf16ToByteOffset(s:text, 4))
call assert_equal(9, simplecc#Utf16ToByteOffset(s:text, 999))

" URI escaping operates on UTF-8 bytes and preserves reserved path characters.
let s:path = '/tmp/simplecc 中 #%25?.rs'
let s:uri = simplecc#PathToUri(s:path)
call assert_equal('file:///tmp/simplecc%20%E4%B8%AD%20%23%2525%3F.rs', s:uri)
call assert_equal(s:path, simplecc#UriToPath(s:uri))
call assert_equal('/tmp/中 #%.rs',
      \ simplecc#UriToPath('file:///tmp/%e4%b8%ad%20%23%25.rs'))
call assert_equal('/tmp/%ZZ.rs', simplecc#UriToPath('file:///tmp/%ZZ.rs'))

" Text edits cover astral characters, reverse ordering, empty replacement,
" and a range that spans multiple lines.
enew!
call setline(1, ['a中😀z'])
call simplecc#ApplyTextEdits(bufnr('%'), [
      \ {'line': 0, 'character': 4, 'end_line': 0, 'end_character': 5,
      \  'new_text': 'Z'},
      \ {'line': 0, 'character': 0, 'end_line': 0, 'end_character': 1,
      \  'new_text': 'A'},
      \ ])
call assert_equal(['A中😀Z'], getline(1, '$'))

call setline(1, ['a中😀z'])
call simplecc#ApplyTextEdits(bufnr('%'), [
      \ {'line': 0, 'character': 1, 'end_line': 0, 'end_character': 4,
      \  'new_text': ''},
      \ ])
call assert_equal(['az'], getline(1, '$'))

call setline(1, ['a中😀z', 'tail行'])
call simplecc#ApplyTextEdits(bufnr('%'), [
      \ {'line': 0, 'character': 1, 'end_line': 1, 'end_character': 4,
      \  'new_text': "M\nN"},
      \ ])
call assert_equal(['aM', 'N行'], getline(1, '$'))

" Statusline diagnostic counts: the exported function returns the documented
" shape even for buffers with no diagnostics or a bufnr that does not exist.
let s:zero_counts = {'error': 0, 'warning': 0, 'info': 0, 'hint': 0}
call assert_equal(s:zero_counts, simplecc#DiagCounts())
call assert_equal(s:zero_counts, simplecc#DiagCounts(bufnr('%')))
call assert_equal(s:zero_counts, simplecc#DiagCounts(99999))

" Signature help auto-trigger is enabled by default and user-overridable.
call assert_equal(1, g:simplecc_signature_help)

" Restart is generation-aware: the replacement starts only after the old
" daemon exits, and an old exit callback cannot reset the replacement state.
let s:fake_daemon = tempname()
call writefile(readfile(s:root .. '/test/fake_daemon.sh'), s:fake_daemon)
call assert_equal(1, setfperm(s:fake_daemon, 'rwx------'))
let g:simplecc_daemon_path = s:fake_daemon
call simplecc#Start()
sleep 300m
call assert_equal('ready', g:simplecc_status)

" Definition replies must stay attached to the split that issued the request.
" Selecting a result must replace that source split and create one precise
" jumplist entry for Ctrl-O, even for a same-buffer target.
let s:source_file = tempname() .. '.rs'
let s:other_file = tempname() .. '.rs'
call writefile(['alpha', 'beta symbol', 'gamma target', 'delta target'],
      \ s:source_file)
call writefile(['unrelated window'], s:other_file)
execute 'edit! ' .. fnameescape(s:source_file)
setfiletype rust
call cursor(2, 6)
let s:source_winid = win_getid()
execute 'vsplit ' .. fnameescape(s:other_file)
setfiletype rust
let s:other_winid = win_getid()
call win_gotoid(s:source_winid)
sleep 150m

" Diagnostics stay split-local by default, while :SimpleCCDiagnostics! gathers
" all known buffers.  An exact severity argument must filter both scopes.
call assert_equal({'error': 1, 'warning': 3, 'info': 0, 'hint': 2},
      \ simplecc#DiagCounts())
SimpleCCDiagnostics error
let s:source_loclist = getloclist(win_id2win(s:source_winid))
call assert_equal(1, len(s:source_loclist))
call assert_equal('E', s:source_loclist[0].type)
lclose
call win_gotoid(s:source_winid)
SimpleCCDiagnostics info
call assert_equal([], getloclist(win_id2win(s:source_winid)),
      \ 'an empty severity filter left stale diagnostics in the location list')
SimpleCCDiagnostics! error
call assert_equal(2, len(getqflist()))
call assert_equal(['E', 'E'], map(getqflist(), {_, item -> item.type}))
cclose
call win_gotoid(s:source_winid)

" Navigation follows the same visible-severity boundary as signs and virtual
" text, and wraps using the sorted diagnostic order.
let g:simplecc_diag_min_severity = 2
" The on-demand inspector works even with automatic floats disabled, shares
" the visible-severity filter, and normalizes string/integer LSP codes.
call cursor(1, 1)
let s:before_popups = popup_list()
SimpleCCDiag
let s:new_popups = filter(popup_list(), {_, id -> index(s:before_popups, id) < 0})
call assert_equal(1, len(s:new_popups))
call assert_equal(['[Error rustc(E0001)] source error',
      \ '[Warning alint(9)] lexically first warning',
      \ '[Warning lint(7)] same-position warning'],
      \ getbufline(winbufnr(s:new_popups[0]), 1, '$'))
call popup_close(s:new_popups[0])
call cursor(2, 1)
let s:before_popups = popup_list()
SimpleCCDiag
let s:new_popups = filter(popup_list(), {_, id -> index(s:before_popups, id) < 0})
call assert_equal(1, len(s:new_popups))
call assert_equal(['[Warning lint(42)] source warning'],
      \ getbufline(winbufnr(s:new_popups[0]), 1, '$'))
call popup_close(s:new_popups[0])

call cursor(1, 1)
call simplecc#DiagNext()
call assert_equal([2, 6], [line('.'), col('.')])
call simplecc#DiagNext()
call assert_equal([1, 1], [line('.'), col('.')])
call simplecc#DiagPrev()
call assert_equal([2, 6], [line('.'), col('.')])

" An explicit exact severity is independent of the display threshold. This
" lets users jump through hidden hints temporarily without changing signs or
" virtual text; no argument above retains the old visible-only behaviour.
call cursor(1, 1)
SimpleCCNextDiag hint
call assert_equal([2, 2], [line('.'), col('.')])
SimpleCCNextDiag hint
call assert_equal([3, 2], [line('.'), col('.')])
SimpleCCPrevDiag hint
call assert_equal([2, 2], [line('.'), col('.')])
SimpleCCNextDiag error
call assert_equal([1, 1], [line('.'), col('.')])

" Same-position ties have a deterministic message when navigation wraps.
call cursor(2, 6)
let s:warning_nav = execute('SimpleCCNextDiag warning')
call assert_equal([1, 1], [line('.'), col('.')])
call assert_match('\[Warning alint(9)\] lexically first warning', s:warning_nav)

" Invalid filters fail closed: no cursor movement and no fallback navigation.
call cursor(2, 6)
messages clear
SimpleCCNextDiag fatal
call assert_equal([2, 6], [line('.'), col('.')])
call assert_match('diagnostic severity must be', execute('messages'))
messages clear
SimpleCCNextDiag warn
call assert_equal([2, 6], [line('.'), col('.')])
call assert_match('diagnostic severity must be', execute('messages'))
call assert_equal(['all', 'error', 'warning', 'info', 'hint'],
      \ simplecc#CompleteDiagnosticSeverity('', '', 0))
let g:simplecc_diag_min_severity = 4

" Empty/malformed messages still render a useful, nonempty detail line.
call cursor(3, 2)
let s:before_popups = popup_list()
SimpleCCDiag
let s:new_popups = filter(popup_list(), {_, id -> index(s:before_popups, id) < 0})
call assert_equal(1, len(s:new_popups))
call assert_equal(['[Hint] (no diagnostic message)'],
      \ getbufline(winbufnr(s:new_popups[0]), 1, '$'))
call popup_close(s:new_popups[0])
call cursor(2, 6)

call simplecc#Definition()
call win_gotoid(s:other_winid)
sleep 250m

let s:list_winid = win_getid()
call assert_equal(1, get(getwininfo(s:list_winid), 0, {})->get('loclist', 0))
call assert_equal(win_screenpos(s:source_winid)[1], win_screenpos(s:list_winid)[1])
call assert_notequal(win_screenpos(s:other_winid)[1], win_screenpos(s:list_winid)[1])
call assert_equal(s:other_file, fnamemodify(bufname(winbufnr(s:other_winid)), ':p'))

call simplecc#QfEnter()
call assert_equal(s:source_winid, win_getid())
call assert_equal(s:source_file, expand('%:p'))
call assert_equal([3, 2], [line('.'), col('.')])
execute "normal! \<C-o>"
call assert_equal(s:source_file, expand('%:p'))
call assert_equal([2, 6], [line('.'), col('.')])

" A single-location reply uses the same originating window and jumplist path.
call cursor(1, 3)
call simplecc#Definition()
call win_gotoid(s:other_winid)
sleep 250m
call assert_equal(s:source_winid, win_getid())
call assert_equal(s:source_file, expand('%:p'))
call assert_equal([3, 2], [line('.'), col('.')])
execute "normal! \<C-o>"
call assert_equal([1, 3], [line('.'), col('.')])

call simplecc#Restart()
sleep 700m
call assert_equal('ready', g:simplecc_status)
call simplecc#Stop()
sleep 300m
call assert_equal('', g:simplecc_status)
call delete(s:fake_daemon)
call delete(s:source_file)
call delete(s:other_file)

if !empty(v:errors)
  call writefile(v:errors, '/dev/stderr')
  cquit 1
endif
qa!
