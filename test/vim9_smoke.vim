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

" Restart is generation-aware: the replacement starts only after the old
" daemon exits, and an old exit callback cannot reset the replacement state.
let s:fake_daemon = tempname()
call writefile(readfile(s:root .. '/test/fake_daemon.sh'), s:fake_daemon)
call assert_equal(1, setfperm(s:fake_daemon, 'rwx------'))
let g:simplecc_daemon_path = s:fake_daemon
call simplecc#Start()
sleep 300m
call assert_equal('ready', g:simplecc_status)
call simplecc#Restart()
sleep 700m
call assert_equal('ready', g:simplecc_status)
call simplecc#Stop()
sleep 300m
call assert_equal('', g:simplecc_status)
call delete(s:fake_daemon)

if !empty(v:errors)
  call writefile(v:errors, '/dev/stderr')
  cquit 1
endif
qa!
