set nocompatible
set nomore
call delete(expand('<sfile>:p:h') .. '/save-pipeline-errors.log')
let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set rtp^=' .. fnameescape(s:root)
let g:simplecc_auto_start = 0
let g:simplecc_no_default_maps = 1
let g:simplecc_daemon_path = s:root .. '/test/fake_save_daemon.py'
let $SIMPLECC_SAVE_TRACE = tempname()
let $SIMPLECC_SAVE_MODE = tempname()
call writefile([], $SIMPLECC_SAVE_TRACE)
call writefile(['normal'], $SIMPLECC_SAVE_MODE)
runtime plugin/simplecc.vim
call simplecc#Start()
sleep 200m
let s:file = tempname() .. '.rs'
call writefile(['raw'], s:file)
execute 'edit ' .. fnameescape(s:file)
setfiletype rust
call simplecc#save#Start({'format': v:true, 'imports': v:true})
sleep 300m
call assert_equal(['organized'], readfile(s:file))
undo
call assert_equal('raw', getline(1), 'pipeline edits undo together')
redo
let s:messages = map(readfile($SIMPLECC_SAVE_TRACE), {_, l -> json_decode(l)})
let s:types = map(copy(s:messages), {_, m -> m.type})
call assert_true(index(s:types, 'textDocument/formatting') < index(s:types, 'textDocument/organizeImports'))
let s:changes = filter(copy(s:messages), {_, m -> m.type ==# 'textDocument/didChange'})
call assert_true(!empty(s:changes), 'format edit synchronized before imports')
" A late response must not overwrite new input or save it.
call setline(1, 'raw')
call writefile(['delay'], $SIMPLECC_SAVE_MODE)
call simplecc#save#Start({'format': v:true})
call setline(1, 'typing')
sleep 500m
call assert_equal('typing', getline(1))
call assert_equal(['organized'], readfile(s:file))
" Expired request may never apply its edits or save on a late response.
call setline(1, 'raw')
call simplecc#save#Start({'format': v:true, 'timeout_ms': 100})
sleep 500m
call assert_equal('raw', getline(1))
call assert_equal(['organized'], readfile(s:file))
" Errors stop the pipeline, preserving unsaved contents.
call writefile(['error'], $SIMPLECC_SAVE_MODE)
call simplecc#save#Start({'format': v:true, 'imports': v:true})
sleep 200m
call assert_equal('raw', getline(1))
call assert_true(&modified)
" A normal acwrite write must keep its BufWriteCmd transport.
call writefile(['normal'], $SIMPLECC_SAVE_MODE)
setlocal buftype=acwrite
let g:save_remote_calls = 0
augroup SaveRemoteTest
  autocmd!
  autocmd BufWriteCmd <buffer> let g:save_remote_calls += 1 | setlocal nomodified
augroup END
call simplecc#save#Start({'format': v:true, 'imports': v:true})
sleep 300m
call assert_equal(1, g:save_remote_calls)
call assert_equal('organized', getline(1))
" Superseding request tokens never claim another request's success.
let s:old = simplecc#save#Start({'format': v:true})
let s:new = simplecc#save#Start({'format': v:true})
call assert_equal('error', simplecc#save#Status(bufnr(), s:old).status)
sleep 300m
call assert_equal('success', simplecc#save#Status(bufnr(), s:new).status)
" Write errors are caught and do not claim success/reset modified.
augroup SaveRemoteTest
  autocmd!
  autocmd BufWriteCmd <buffer> throw 'test remote write failed'
augroup END
call setline(1, 'raw')
call simplecc#save#Start({'format': v:true})
sleep 250m
call assert_true(&modified)
augroup SaveRemoteTest
  autocmd!
augroup END
call simplecc#Stop()
if !empty(v:errors)
  call writefile(v:errors, s:root .. '/test/save-pipeline-errors.log')
  cquit
endif
qa!
