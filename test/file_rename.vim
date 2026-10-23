set nocompatible
let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set rtp^=' .. fnameescape(s:root)
let s:fake = tempname()
call mkdir(s:fake .. '/autoload/simplecc', 'p')
call writefile(['vim9script',
      \ 'export def IsRunning(): bool', '  return get(g:, "rename_running", true)', 'enddef',
      \ 'export def Health(): dict<any>', '  return {starts: 1}', 'enddef',
      \ 'export def Request(req: dict<any>, Cb: func, timeout: number = 0): number',
      \ '  add(g:rename_requests, copy(req))',
      \ '  if get(g:, "rename_error", false)',
      \ '    Cb({type: "error", message: "fake timeout"})',
      \ '  elseif req.phase ==# "check" && get(g:, "rename_changed_connection", false)',
      \ '    Cb({type: "error", message: "connection changed"})',
      \ '  else',
      \ '    Cb({type: "fileRename", identity: "fake", supported: true, edits: req.phase ==# "prepare" ? g:rename_edits : []})',
      \ '  endif', '  return 1', 'enddef'], s:fake .. '/autoload/simplecc/core.vim')
execute 'set rtp^=' .. fnameescape(s:fake)
call simplecc#PathToUri('')
let s:sid = getscriptinfo({'name': 'autoload/simplecc.vim'})[0].sid
call call(function('<SNR>' .. s:sid .. '_OnBackendEvent'), [{'type': 'initialized', 'capabilities': {'file_rename': v:true}}])
let g:rename_requests = []
let g:rename_edits = []
let s:dir = tempname()
call mkdir(s:dir, 'p')
function! s:Move(old, new) abort
  return rename(a:old, a:new) == 0
endfunction
function! s:FailMove() abort
  return 0
endfunction
function! s:CountMove() abort
  let g:rename_move_count += 1
  return 1
endfunction
function! s:Edit(path, text) abort
  return {'changes': [{'uri': simplecc#PathToUri(a:path), 'edits': [
        \ {'line': 0, 'character': 0, 'end_line': 0, 'end_character': 3, 'new_text': a:text}]}], 'operations': []}
endfunction
function! s:Setup() abort
  call simplecc#files#Cancel()
  silent! %bwipeout!
  call delete(s:dir, 'rf')
  call mkdir(s:dir, 'p')
  call writefile(['old'], s:dir .. '/old.py')
  call writefile(['old import'], s:dir .. '/consumer.py')
  let g:rename_requests = []
  let g:rename_edits = [s:Edit(s:dir .. '/consumer.py', 'new')]
  let g:rename_error = 0
  let g:rename_changed_connection = 0
  let g:rename_running = 1
endfunction
call s:Setup()
call assert_true(simplecc#files#Begin(s:dir .. '/old.py', s:dir .. '/new.py', v:false, function('s:Move', [s:dir .. '/old.py', s:dir .. '/new.py'])))
call assert_equal('diff', &filetype)
call assert_true(index(getline(1, '$'), '-old import') >= 0)
call assert_equal(['old import'], readfile(s:dir .. '/consumer.py'))
call assert_equal(['old import'], getbufline(bufnr(s:dir .. '/consumer.py'), 1, '$'))
call simplecc#files#Apply()
call assert_true(filereadable(s:dir .. '/new.py'))
call assert_equal(['new import'], getbufline(bufnr(s:dir .. '/consumer.py'), 1, '$'))
call assert_true(getbufvar(bufnr(s:dir .. '/consumer.py'), '&modified'))
call assert_true(buflisted(bufnr(s:dir .. '/consumer.py')), 'imports edits remain discoverable by save-all')
call assert_equal(['prepare', 'check', 'did'], map(copy(g:rename_requests), 'v:val.phase'))
execute 'sbuffer ' .. bufnr(s:dir .. '/consumer.py')
normal! u
call assert_equal(['old import'], getline(1, '$'))
execute "normal! \<C-R>"
call assert_equal(['new import'], getline(1, '$'))


" Literal LSP paths must never use bufnr()'s filename-pattern fallback.
call s:Setup()
let s:literal = s:dir .. '/consumer[1].py'
let s:similar = s:dir .. '/consumer1.py'
call writefile(['old import'], s:literal)
call writefile(['old import'], s:similar)
let s:similar_bnr = bufadd(s:similar)
call bufload(s:similar_bnr)
let g:rename_edits = [s:Edit(s:literal, 'new')]
call assert_true(simplecc#files#Begin(s:dir .. '/old.py', s:dir .. '/new.py', v:false, function('s:Move', [s:dir .. '/old.py', s:dir .. '/new.py'])))
call simplecc#files#Apply()
let s:literal_buffers = filter(getbufinfo(), {_, info -> info.name ==# s:literal})
call assert_equal(1, len(s:literal_buffers), 'the bracketed imports path gets its own exact buffer')
let s:literal_bnr = get(get(s:literal_buffers, 0, {}), 'bufnr', -1)
call assert_equal(['new import'], getbufline(s:literal_bnr, 1, '$'), 'imports update the literal bracketed path')
call assert_equal(['old import'], getbufline(s:similar_bnr, 1, '$'), 'a regex-similar existing buffer is untouched')
call assert_false(getbufvar(s:similar_bnr, '&modified'))
call assert_equal(['old import'], readfile(s:literal), 'preview/import application does not write the source file')


call s:Setup()
call simplecc#files#Begin(s:dir .. '/old.py', s:dir .. '/new.py', v:false, function('s:Move', [s:dir .. '/old.py', s:dir .. '/new.py']))
call simplecc#files#Cancel()
call assert_true(filereadable(s:dir .. '/old.py'))
call assert_equal(['old import'], getbufline(bufnr(s:dir .. '/consumer.py'), 1, '$'))
call assert_equal(['prepare'], map(copy(g:rename_requests), 'v:val.phase'))

call s:Setup()
call simplecc#files#Begin(s:dir .. '/old.py', s:dir .. '/new.py', v:false, function('s:FailMove'))
call simplecc#files#Apply()
call assert_equal(['old import'], getbufline(bufnr(s:dir .. '/consumer.py'), 1, '$'))
call assert_true(filereadable(s:dir .. '/old.py'))
call assert_equal(['prepare', 'check'], map(copy(g:rename_requests), 'v:val.phase'))

call s:Setup()
call simplecc#files#Begin(s:dir .. '/old.py', s:dir .. '/new.py', v:false, function('s:Move', [s:dir .. '/old.py', s:dir .. '/new.py']))
call setbufline(bufnr(s:dir .. '/consumer.py'), 1, 'user typing')
call simplecc#files#Apply()
call assert_true(filereadable(s:dir .. '/old.py'))
call assert_equal(['user typing'], getbufline(bufnr(s:dir .. '/consumer.py'), 1, '$'))

call s:Setup()
call simplecc#files#Begin(s:dir .. '/old.py', s:dir .. '/new.py', v:false, function('s:Move', [s:dir .. '/old.py', s:dir .. '/new.py']))
let g:rename_changed_connection = 1
call simplecc#files#Apply()
call assert_true(filereadable(s:dir .. '/old.py'))

call s:Setup()
let g:rename_error = 1
call assert_false(simplecc#files#Begin(s:dir .. '/old.py', s:dir .. '/new.py', v:false, function('s:Move', [s:dir .. '/old.py', s:dir .. '/new.py'])))
call assert_true(filereadable(s:dir .. '/old.py'))

call s:Setup()
call writefile(['target'], s:dir .. '/new.py')
call assert_false(simplecc#files#Begin(s:dir .. '/old.py', s:dir .. '/new.py', v:false, function('s:Move', [s:dir .. '/old.py', s:dir .. '/new.py'])))
call assert_equal(['target'], readfile(s:dir .. '/new.py'))

" Missing capability is detected without issuing an unsupported request.
call s:Setup()
call call(function('<SNR>' .. s:sid .. '_OnBackendEvent'), [{'type': 'initialized', 'capabilities': {}}])
call assert_false(simplecc#FileRenameAvailable())
call assert_false(simplecc#files#Begin(s:dir .. '/old.py', s:dir .. '/new.py', v:false, function('s:Move', [s:dir .. '/old.py', s:dir .. '/new.py'])))
call assert_equal([], g:rename_requests)
call assert_true(filereadable(s:dir .. '/old.py'))
call call(function('<SNR>' .. s:sid .. '_OnBackendEvent'), [{'type': 'initialized', 'capabilities': {'file_rename': v:true}}])

call s:Setup()
let g:rename_running = 0
call simplecc#files#Begin(s:dir .. '/old.py', s:dir .. '/new.py', v:false, function('s:Move', [s:dir .. '/old.py', s:dir .. '/new.py']))
call assert_true(filereadable(s:dir .. '/new.py'))
call assert_equal([], g:rename_requests)

call s:Setup()
call mkdir(s:dir .. '/pkg')
call writefile(['old'], s:dir .. '/pkg/a.py')
let g:rename_edits = [s:Edit(s:dir .. '/pkg/a.py', 'new'), s:Edit(s:dir .. '/consumer.py', 'pkg')]
call simplecc#files#Begin(s:dir .. '/pkg', s:dir .. '/newpkg', v:true, function('s:Move', [s:dir .. '/pkg', s:dir .. '/newpkg']))
call simplecc#files#Apply()
call assert_true(filereadable(s:dir .. '/newpkg/a.py'))
call assert_equal(['new'], getbufline(bufnr(s:dir .. '/pkg/a.py'), 1, '$'))
call assert_equal(['pkg import'], getbufline(bufnr(s:dir .. '/consumer.py'), 1, '$'))

" Remote URI buffers are loaded asynchronously and must never hit local disk.
call s:Setup()
let g:simpleremote_workspace = {'id': 12, 'kind': 'ssh', 'target': 'fake', 'root': '/workspace', 'local_root': ''}
let g:remote_files = {'remote:///workspace/old.py': ['old', ''], 'remote:///workspace/consumer.py': ['old import', '']}
" Older SimpleRemote installations must still support ordinary remote rename.
let g:rename_move_count = 0
let g:rename_running = 0
call assert_false(exists('*g:SimpleRemoteReadFileLines'))
call assert_true(simplecc#files#Begin('remote:///workspace/old.py', 'remote:///workspace/new.py', v:false, function('s:CountMove')))
call assert_equal(1, g:rename_move_count, 'no backend needs no imports reader for ordinary remote moves')
let g:rename_running = 1
let g:rename_edits = [s:Edit('remote:///workspace/consumer.py', 'new')]
call assert_false(simplecc#files#Begin('remote:///workspace/old.py', 'remote:///workspace/new.py', v:false, function('s:CountMove')))
call assert_equal(1, g:rename_move_count, 'imports-aware rename with an old provider defaults to cancel')
let g:rename_edits = []
call assert_true(simplecc#files#Begin('remote:///workspace/old.py', 'remote:///workspace/new.py', v:false, function('s:CountMove')))
call assert_equal(2, g:rename_move_count, 'a server without imports edits does not require the preview reader')
let g:rename_requests = []
function! g:SimpleRemoteReadFileLines(path, Callback) abort
  call a:Callback(has_key(g:remote_files, a:path), get(g:remote_files, a:path, ['missing']))
  return 1
endfunction
function! s:FinishRemoteLoad(bnr, uri, timer) abort
  call setbufline(a:bnr, 1, g:remote_files[a:uri][0:-2])
  call setbufvar(a:bnr, '&buftype', 'acwrite')
  call setbufvar(a:bnr, 'vimrc_remote', {'uri': a:uri, 'path': substitute(a:uri, '^remote://', '', ''), 'generation': 12})
  call setbufvar(a:bnr, 'vimrc_remote_read', {})
  call setbufvar(a:bnr, '&modified', 0)
endfunction
function! s:RemoteLoad(uri) abort
  call setbufvar(bufnr(), 'vimrc_remote_read', {'request_id': 1})
  call timer_start(20, function('s:FinishRemoteLoad', [bufnr(), a:uri]))
endfunction
augroup FakeRenameRemote
  autocmd!
  autocmd BufReadCmd remote://* call s:RemoteLoad(expand('<amatch>'))
augroup END
function! s:MoveRemote() abort
  let g:remote_files['remote:///workspace/new.py'] = remove(g:remote_files, 'remote:///workspace/old.py')
  return 1
endfunction
let g:rename_edits = [s:Edit('remote:///workspace/consumer.py', 'new')]
call assert_true(simplecc#files#Begin('remote:///workspace/old.py', 'remote:///workspace/new.py', v:false, function('s:MoveRemote')))
call assert_equal('diff', &filetype)
call simplecc#files#Apply()
call assert_true(has_key(g:remote_files, 'remote:///workspace/new.py'))
call assert_equal(['new import'], getbufline(bufnr('remote:///workspace/consumer.py'), 1, '$'))
call assert_equal(['old import', ''], g:remote_files['remote:///workspace/consumer.py'])
call assert_equal(['prepare', 'check', 'did'], map(copy(g:rename_requests), 'v:val.phase'))

let g:remote_files['remote:///workspace/old.py'] = ['old', '']
let g:rename_edits = []
let g:rename_requests = []
call simplecc#files#Begin('remote:///workspace/old.py', 'remote:///workspace/new.py', v:false, function('s:FailMove'))
call assert_equal(['prepare', 'check'], map(copy(g:rename_requests), 'v:val.phase'))
augroup FakeRenameRemote
  autocmd!
augroup END
unlet g:simpleremote_workspace

call simplecc#files#Cancel()
call delete(s:dir, 'rf')
call delete(s:fake, 'rf')
if !empty(v:errors)
  call writefile(v:errors, '/tmp/file-rename-errors.log')
  cquit
endif
qa!
