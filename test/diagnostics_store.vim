" Diagnostics store: keyed by resolved path, bucketed by publishing server.
"
" Two bugs lived here. (1) The store was keyed by the language server's own URI
" string while every query looked up Vim's. Vim's PercentEncodePath() escapes
" everything outside A-Za-z0-9-._~/: and the daemon's `url` crate leaves
" @ ( ) + , ; = & ' ! $ * alone, so for any node_modules/@types/... file the
" signs rendered (uri -> path -> bufnr) while DiagCounts(), :SimpleCCDiagnostics,
" :SimpleCCDiag and [d/]d all answered "no diagnostics". (2) The event carried
" no publisher, so with two servers for one filetype each publish replaced the
" other's whole set.
"
" Run:  vim -Nu NONE -n -i NONE -es -S test/diagnostics_store.vim

set nocompatible
set encoding=utf-8
set nomore

let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)
call delete(s:root .. '/test/diagnostics-store-errors.log')

let g:simplecc_auto_start = 0
let g:simplecc_daemon_path = '/nonexistent/simplecc-daemon'
let g:simplecc_no_default_maps = 1
runtime plugin/simplecc.vim
execute 'source ' .. fnameescape(s:root .. '/autoload/simplecc.vim')

let s:sid = getscriptinfo({'name': 'autoload/simplecc.vim'})[0].sid

function! s:Publish(event) abort
  call call(function(printf('<SNR>%d_OnDiagnostics', s:sid)), [a:event])
endfunction

function! s:Call(name, ...) abort
  return call(function(printf('<SNR>%d_%s', s:sid, a:name)), a:000)
endfunction

function! s:Diag(line, severity, message) abort
  return {'line': a:line, 'character': 0, 'end_line': a:line, 'end_character': 1,
        \ 'severity': a:severity, 'message': a:message}
endfunction

" A path with every character the two percent-encoders disagree about. This is
" what `node_modules/@types/...` looks like to the store.
let s:dir = tempname()
call mkdir(s:dir . '/node_modules/@types', 'p')
let s:file = s:dir . "/node_modules/@types/a(1)+b,c;d=e&f'g!h$i*j.d.ts"
call writefile(['export const x = 1;', 'export const y = 2;'], s:file)

execute 'edit! ' .. fnameescape(s:file)
setfiletype typescript

" Vim's own URI spelling escapes those characters; the server's does not.
let s:vim_uri = simplecc#PathToUri(s:file)
call assert_match('%40types', s:vim_uri, 'Vim escapes @ in a path URI')
call assert_notequal('file://' . s:file, s:vim_uri,
      \ 'this fixture must exercise the encoder disagreement')
call assert_equal(s:file, simplecc#UriToPath(s:vim_uri))

" ------------------------------------------------- the server's own spelling ---

" A current daemon sends the decoded path; UriToPath() passes it through.
call s:Publish({'uri': s:file, 'server': 'tsserver',
      \ 'items': [s:Diag(0, 1, 'decoded path')]})
call assert_equal({'error': 1, 'warning': 0, 'info': 0, 'hint': 0},
      \ simplecc#DiagCounts(),
      \ 'a decoded path from the daemon must reach the buffer that owns it')

" An older daemon sends the `url` crate's raw URI, which leaves @ ( ) + alone.
call s:Publish({'uri': s:file, 'server': 'tsserver', 'items': []})
call assert_equal({'error': 0, 'warning': 0, 'info': 0, 'hint': 0},
      \ simplecc#DiagCounts())
call s:Publish({'uri': 'file://' . s:file, 'server': 'tsserver',
      \ 'items': [s:Diag(0, 2, 'unescaped uri')]})
call assert_equal({'error': 0, 'warning': 1, 'info': 0, 'hint': 0},
      \ simplecc#DiagCounts(),
      \ 'an unescaped server URI must resolve to the same key as Vim''s own')

" And so does Vim's escaped spelling, which is what pull diagnostics echo back.
call s:Publish({'uri': s:vim_uri, 'server': 'tsserver',
      \ 'items': [s:Diag(0, 1, 'escaped uri'), s:Diag(1, 2, 'second')]})
call assert_equal({'error': 1, 'warning': 1, 'info': 0, 'hint': 0},
      \ simplecc#DiagCounts(),
      \ 'both URI spellings address one bucket, so the set is replaced not doubled')

" The workspace quickfix list reaches it too -- it walks the store's own keys.
SimpleCCDiagnostics! error
call assert_equal(1, len(getqflist()))
call assert_equal(s:file, fnamemodify(bufname(getqflist()[0].bufnr), ':p'))
cclose

" ----------------------------------------------------------- two publishers ---

" pyright and ruff-lsp both serve python and both publish for the same file.
call s:Publish({'uri': s:file, 'server': 'tsserver', 'items': []})
call s:Publish({'uri': s:file, 'server': 'pyright',
      \ 'items': [s:Diag(0, 1, 'type error')]})
call s:Publish({'uri': s:file, 'server': 'ruff-lsp',
      \ 'items': [s:Diag(1, 2, 'lint warning')]})
call assert_equal({'error': 1, 'warning': 1, 'info': 0, 'hint': 0},
      \ simplecc#DiagCounts(),
      \ 'a second server publishing must not erase the first one''s set')

" Navigation sees the merged set in position order, not one server's.
call cursor(1, 1)
call simplecc#DiagNext()
call assert_equal(2, line('.'), 'navigation crosses from one publisher to the other')

" Republishing one server replaces only that server's set.
call s:Publish({'uri': s:file, 'server': 'pyright',
      \ 'items': [s:Diag(0, 1, 'type error'), s:Diag(0, 1, 'another type error')]})
call assert_equal({'error': 2, 'warning': 1, 'info': 0, 'hint': 0},
      \ simplecc#DiagCounts())

" Clearing one server leaves the other's diagnostics standing.
call s:Publish({'uri': s:file, 'server': 'pyright', 'items': []})
call assert_equal({'error': 0, 'warning': 1, 'info': 0, 'hint': 0},
      \ simplecc#DiagCounts())

" ------------------------------------------------------ g:simplecc_diag_sources ---

call s:Publish({'uri': s:file, 'server': 'pyright',
      \ 'items': [s:Diag(0, 1, 'type error')]})
let g:simplecc_diag_sources = ['pyright']
call assert_equal({'error': 1, 'warning': 0, 'info': 0, 'hint': 0},
      \ simplecc#DiagCounts(), 'a named source filter hides the other server')
let g:simplecc_diag_sources = []
call assert_equal({'error': 1, 'warning': 1, 'info': 0, 'hint': 0},
      \ simplecc#DiagCounts(), 'an empty filter shows every server again')

" A string was treated as a character list: index('pyright', 'pyright') is
" never 0, so every diagnostic vanished, and :SimpleCCHealth threw E730 on
" join().
let g:simplecc_diag_sources = 'pyright'
call assert_equal({'error': 1, 'warning': 0, 'info': 0, 'hint': 0},
      \ simplecc#DiagCounts(), 'a bare string names one source, not a character list')
let g:simplecc_diag_sources = 0
call assert_equal({'error': 1, 'warning': 1, 'info': 0, 'hint': 0},
      \ simplecc#DiagCounts(), 'a non-list filter is ignored rather than throwing')
let g:simplecc_diag_sources = []

" -------------------------------------------- bufnr() is a pattern, not a name ---

" Signs used bufnr(path).  The path of foo.rs is a pattern that also matches
" a listed buffer named foxrs (`.` = any character), so diagnostics for a
" file that is not even open painted the decoy.
let s:decoy_dir = tempname()
call mkdir(s:decoy_dir)
let s:decoy = s:decoy_dir . '/foxrs'
let s:missing = s:decoy_dir . '/foo.rs'
call writefile(['xxxxxxxxxxxxxxxxxxxx'], s:decoy)
call writefile(['ab'], s:missing)
execute 'edit! ' .. fnameescape(s:decoy)
call s:Publish({'uri': s:missing, 'server': 'rust-analyzer',
      \ 'items': [{'line': 0, 'character': 10, 'end_line': 0, 'end_character': 11,
      \  'severity': 1, 'message': 'on the missing file'}]})
call assert_equal([], sign_getplaced(bufnr('%'), {'group': 'simplecc'})[0].signs,
      \ 'diagnostics for foo.rs must not paint signs onto foxrs')
let s:qf = s:Call('DiagnosticQfItem', s:missing,
      \ {'line': 0, 'character': 10, 'end_line': 0, 'end_character': 11,
      \  'severity': 1, 'message': 'on the missing file'})
call assert_equal(3, s:qf.col,
      \ 'PathLine must measure foo.rs (2 bytes → col 3), not the 20-byte decoy')
bwipeout!
call delete(s:decoy_dir, 'rf')

" ---------------------------------------------- bounded workbench snapshot ---
execute 'edit! ' .. fnameescape(s:file)
call setline(1, ['中😀alpha', 'second'])
let g:simplecc_diag_min_severity = 1
let g:test_diag_events = 0
augroup TestDiagnosticSnapshot
  autocmd!
  autocmd User SimpleCCDiagnosticsChanged let g:test_diag_events += 1
augroup END
call s:Publish({'uri': s:file, 'server': 'pyright', 'items': []})
call s:Publish({'uri': s:file, 'server': 'ruff-lsp', 'items': []})
let s:unicode = s:Diag(0, 2, "warning\n" .. repeat('界', 2100))
let s:unicode.character = 3
call s:Publish({'uri': s:file, 'server': 'snapshot', 'items': [s:unicode]})
call assert_true(g:test_diag_events > 0)
let s:before_qf = getqflist({'id': 0, 'changedtick': 0})
let s:before_loc = getloclist(0, {'id': 0, 'changedtick': 0})
let s:view = winsaveview()
let s:snapshot = simplecc#DiagnosticSnapshot()
call assert_equal(1, s:snapshot.total)
call assert_equal(8, s:snapshot.items[0].column, 'UTF-16 three units means seven UTF-8 bytes')
call assert_equal('warning', s:snapshot.items[0].kind, 'render severity filter does not hide snapshot rows')
call assert_equal(2048, strchars(s:snapshot.items[0].text))
call assert_notmatch('[[:cntrl:]]', s:snapshot.items[0].text)
let s:snapshot.items[0].text = 'changed copy'
call assert_notequal('changed copy', simplecc#DiagnosticSnapshot().items[0].text)
call assert_equal(s:before_qf, getqflist({'id': 0, 'changedtick': 0}))
call assert_equal(s:before_loc, getloclist(0, {'id': 0, 'changedtick': 0}))
call assert_equal(s:view, winsaveview())
call assert_equal({'items': [], 'total': 0}, simplecc#DiagnosticSnapshot(-1))
let g:simplecc_diag_sources = ['other']
call assert_equal({'items': [], 'total': 0}, simplecc#DiagnosticSnapshot())
let g:simplecc_diag_sources = []
call assert_equal(1, simplecc#DiagnosticSnapshot().total, 'source change restores cached diagnostics')
call setline(1, 'abcd')
call assert_equal(4, simplecc#DiagnosticSnapshot().items[0].column, 'edits recompute byte columns without a new publish')
let s:replacement = s:Diag(1, 4, 'new publication')
call s:Publish({'uri': s:file, 'server': 'snapshot', 'items': [s:replacement]})
call assert_equal({'line': 2, 'column': 1, 'text': 'new publication', 'kind': 'hint'}, simplecc#DiagnosticSnapshot().items[0])
let s:many = map(range(300), {_, n -> s:Diag(0, 1, 'row ' .. n)})
call s:Publish({'uri': s:file, 'server': 'snapshot', 'items': s:many})
call assert_equal(300, simplecc#DiagnosticSnapshot().total)
call assert_equal(256, len(simplecc#DiagnosticSnapshot().items))
" A malformed cached diagnostic must not break a read-only consumer, even
" when an older publisher supplied invalid position/message types.
try
  call s:Publish({'uri': s:file, 'server': 'snapshot', 'items': [
        \ {'line': 'bad', 'character': 0, 'message': 'invalid row'},
        \ {'line': 0, 'character': [], 'message': 'invalid column'},
        \ {'line': 0, 'character': 0, 'message': []}]})
catch
  " Rendering legacy malformed input may reject it; the snapshot must not.
endtry
call assert_equal({'items': [], 'total': 0}, simplecc#DiagnosticSnapshot())
" A failing consumer cannot abort publication or clearing.
augroup TestDiagnosticSnapshot
  autocmd User SimpleCCDiagnosticsChanged throw 'observer failure'
augroup END
call s:Publish({'uri': s:file, 'server': 'snapshot', 'items': []})
call assert_equal({'items': [], 'total': 0}, simplecc#DiagnosticSnapshot())
call s:Publish({'uri': s:file, 'server': 'snapshot', 'items': [s:Diag(0, 1, 'close me')]})
call assert_equal('close me', simplecc#DiagnosticSnapshot().items[0].text)
call simplecc#OnBufClose(bufnr('%'))
call assert_equal({'items': [], 'total': 0}, simplecc#DiagnosticSnapshot())
call assert_equal({'error': 0, 'warning': 0, 'info': 0, 'hint': 0}, b:simplecc_diag_counts)
augroup TestDiagnosticSnapshot
  autocmd!
augroup END

" Diagnostic motion opens its destination and behaves like a Vim jump.  Check
" actual Ctrl-O navigation, rather than only inspecting the helper's calls.
call setline(1, ['origin', 'two', 'three', 'four', '中😀target', 'six', 'seven', 'eight'])
setlocal foldmethod=manual
3,7fold
call cursor(1, 2)
clearjumps
call setreg('"', 'diagnostic-motion-register')
let s:text_before = getline(1, '$')
let s:undo_before = undotree()
let s:tags_before = gettagstack()
let s:target = {'line': 4, 'character': 3, 'end_line': 4,
      \ 'end_character': 4, 'severity': 1, 'message': 'folded Unicode target'}
call s:Publish({'uri': s:file, 'server': 'motion', 'items': [s:target]})
setlocal readonly nomodifiable buftype=acwrite
call simplecc#DiagNext()
call assert_equal([5, 8], [line('.'), col('.')], 'next uses UTF-16 columns')
call assert_equal(-1, foldclosed(5), 'next exposes the folded diagnostic')
execute "normal! \<C-O>"
call assert_equal([1, 2], [line('.'), col('.')], 'Ctrl-O returns to diagnostic origin')
" Next and previous wrapping use the same path, including an origin below
" the only diagnostic. A same-position wrap does not create another jump.
call cursor(8, 2)
3,7fold
clearjumps
call simplecc#DiagNext()
call assert_equal([5, 8], [line('.'), col('.')], 'next wraps to diagnostic')
call assert_equal(-1, foldclosed(5), 'next wrap opens fold')
execute "normal! \<C-O>"
call assert_equal([8, 2], [line('.'), col('.')], 'next wrap preserves origin')
call cursor(8, 2)
3,7fold
clearjumps
call simplecc#DiagPrev()
call assert_equal([5, 8], [line('.'), col('.')], 'previous uses UTF-16 columns')
call assert_equal(-1, foldclosed(5), 'previous opens fold')
execute "normal! \<C-O>"
call assert_equal([8, 2], [line('.'), col('.')], 'previous preserves origin')
call cursor(1, 2)
3,7fold
clearjumps
call simplecc#DiagPrev()
call assert_equal([5, 8], [line('.'), col('.')], 'previous wraps to diagnostic')
call assert_equal(-1, foldclosed(5), 'previous wrap opens fold')
execute "normal! \<C-O>"
call assert_equal([1, 2], [line('.'), col('.')], 'previous wrap preserves origin')
call cursor(5, 8)
clearjumps
let s:jumps_before = getjumplist()
call simplecc#DiagNext()
call simplecc#DiagPrev()
call assert_equal(s:jumps_before, getjumplist(), 'same-position wrapping adds no history')
call s:Publish({'uri': s:file, 'server': 'motion', 'items': []})
call simplecc#DiagNext()
call simplecc#DiagPrev()
call assert_equal(s:jumps_before, getjumplist(), 'empty diagnostics add no history')
call assert_equal(s:text_before, getline(1, '$'), 'navigation never edits text')
call assert_equal(s:undo_before, undotree(), 'navigation preserves undo history')
call assert_equal(s:tags_before, gettagstack(), 'diagnostics do not add symbol tags')
call assert_equal('diagnostic-motion-register', getreg('"'), 'navigation preserves register')
setlocal noreadonly modifiable buftype=

" ------------------------------------------------------------------ cleanup ---

" Closing the buffer drops its entire entry, whichever servers published it.
call simplecc#OnBufClose(bufnr('%'))
call assert_equal({'error': 0, 'warning': 0, 'info': 0, 'hint': 0},
      \ simplecc#DiagCounts())

bwipeout!
call delete(s:dir, 'rf')

if len(v:errors)
  call writefile(v:errors, s:root .. '/test/diagnostics-store-errors.log')
  for s:e in v:errors
    echomsg s:e
  endfor
  cquit
endif
qall!
