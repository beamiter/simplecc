set nocompatible nomore
let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set rtp^=' .. fnameescape(s:root)
let s:errors = s:root .. '/test/save-write-ack-errors.log'
call delete(s:errors)
let s:file = tempname()
call writefile(['original'], s:file)
execute 'edit ' .. fnameescape(s:file)
setlocal buftype=acwrite
let s:result = 'success'
let s:delay = 100
let s:serial = 0
function! s:Finish(buffer, identity, result, timer) abort
  let l:state = getbufvar(a:buffer, 'vimrc_remote_write_state', {})
  if get(l:state, 'serial', -1) != a:identity.serial
        \ || get(l:state, 'generation', -1) != a:identity.generation
    return
  endif
  if getbufvar(a:buffer, 'changedtick') != a:identity.tick
    let l:state.status = 'stale'
    call setbufvar(a:buffer, '&modified', 1)
  else
    let l:state.status = a:result
    call setbufvar(a:buffer, '&modified', a:result !=# 'success')
  endif
  let l:state.message = a:result ==# 'error' ? 'test write failure' : ''
endfunction
function! s:Begin() abort
  let s:serial += 1
  let b:vimrc_remote_write_state = {'serial':s:serial, 'generation':1,
        \ 'tick':b:changedtick, 'final_eol': !!(&endofline || (&fixendofline && !&binary)),
        \ 'status':'pending', 'message':'', 'path':get(get(b:, 'vimrc_remote', {}), 'path', '')}
  call timer_start(s:delay, function('s:Finish', [bufnr(), copy(b:vimrc_remote_write_state), s:result]))
endfunction
augroup AckSaveTest
  autocmd!
  autocmd BufWriteCmd <buffer> call s:Begin()
augroup END
let s:options = {'format':v:false, 'imports':v:false}
" A clean explicit remote write still waits for its actual ACK.
let s:token = simplecc#save#Start(s:options)
call assert_equal('writing', simplecc#save#Status(bufnr(), s:token).status)
sleep 150m
call assert_equal('success', simplecc#save#Status(bufnr(), s:token).status)
" Undo may make modified false, but cannot satisfy an in-flight write.
call setline(1, 'changed')
let s:token = simplecc#save#Start(s:options)
undo
call assert_equal('error', simplecc#save#Status(bufnr(), s:token).status)
sleep 150m
call assert_equal('error', simplecc#save#Status(bufnr(), s:token).status)
" Clean writes that fail never look like successful saves.
setlocal nomodified
let s:result = 'error'
let s:token = simplecc#save#Start(s:options)
call assert_equal('writing', simplecc#save#Status(bufnr(), s:token).status)
sleep 150m
call assert_equal('error', simplecc#save#Status(bufnr(), s:token).status)
" A newer write owns the ACK state; old callbacks cannot satisfy its token.
let s:result = 'success'
let s:old = simplecc#save#Start(s:options)
let s:new = simplecc#save#Start(s:options)
call assert_equal('error', simplecc#save#Status(bufnr(), s:old).status)
call assert_equal('writing', simplecc#save#Status(bufnr(), s:new).status)
sleep 150m
call assert_equal('success', simplecc#save#Status(bufnr(), s:new).status)
" A connection generation change invalidates otherwise identical ACK data.
let s:token = simplecc#save#Start(s:options)
let b:vimrc_remote_write_state.generation += 1
call assert_equal('error', simplecc#save#Status(bufnr(), s:token).status)
sleep 150m
" Changing only final-EOL after ACK but before Status is still a new payload.
setlocal nofixendofline endofline nomodified
let s:token = simplecc#save#Start(s:options)
sleep 150m
setlocal noendofline
call assert_equal('error', simplecc#save#Status(bufnr(), s:token).status)
setlocal endofline fixendofline
" A path switch after ACK cannot satisfy the old file's save request.
let b:vimrc_remote = {'path':'/first.rs'}
let s:token = simplecc#save#Start(s:options)
sleep 150m
let b:vimrc_remote.path = '/other.rs'
call assert_equal('error', simplecc#save#Status(bufnr(), s:token).status)
unlet b:vimrc_remote
" A provider without ACK metadata still must preserve the written tick.
unlet b:vimrc_remote_write_state
augroup AckSaveTest
  autocmd!
  autocmd BufWriteCmd <buffer> call timer_start(100, {_->execute('setlocal nomodified')})
augroup END
call setline(1, 'generic change')
let s:token = simplecc#save#Start(s:options)
undo
call assert_equal('error', simplecc#save#Status(bufnr(), s:token).status)
sleep 150m
" A clean generic acwrite is unknowable: never claim an unobserved ACK.
let s:token = simplecc#save#Start(s:options)
call assert_equal('writing', simplecc#save#Status(bufnr(), s:token).status)
sleep 150m
call assert_equal('writing', simplecc#save#Status(bufnr(), s:token).status)
augroup AckSaveTest
  autocmd!
augroup END
if !empty(v:errors)
  call writefile(v:errors, s:errors)
  cquit
endif
qa!
