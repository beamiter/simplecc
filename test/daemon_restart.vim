" Crash recovery: a daemon that dies unexpectedly comes back on its own.
"
" doc/simplecc.txt promised exponential-backoff restarts and a crash-loop
" breaker for years while autoload/simplecc.vim ran its own supervision that
" restarted only when :SimpleCCRestart had already asked for it. An OOM-killed
" or panicking daemon therefore left every command answering
" '[SimpleCC] not initialized' until the user found the restart command.
"
" Run:  vim -Nu NONE -n -i NONE -es -S test/daemon_restart.vim

set nocompatible
set encoding=utf-8
set nomore

let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)
call delete(s:root .. '/test/daemon-restart-errors.log')

let g:simplecc_auto_start = 0
let g:simplecc_no_default_maps = 1
runtime plugin/simplecc.vim
execute 'source ' .. fnameescape(s:root .. '/autoload/simplecc.vim')

" A copy, so the checkout's mode bits are irrelevant and CI cannot fail on them.
let s:daemon = tempname()
call writefile(readfile(s:root .. '/test/fake_daemon_crash.sh'), s:daemon)
call assert_equal(1, setfperm(s:daemon, 'rwx------'))
let g:simplecc_daemon_path = s:daemon

" Poll rather than sleeping a fixed budget: sleep is also what lets Vim service
" the channel callbacks that carry the daemon's replies.
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

" ------------------------------------------------- unexpected death recovers ---

call simplecc#Start()
call assert_equal(1, s:Wait("g:simplecc_status ==# 'ready'", 2000),
      \ 'the daemon must reach ready before the crash')

" Kill it the way the OOM killer does: no shutdown, no ack, no warning.
SimpleCCReloadConfig
call assert_equal(1, s:Wait("g:simplecc_status !=# 'ready'", 2000),
      \ 'the plugin must notice the daemon died')

" No user action here on purpose -- this is the whole point.
call assert_equal(1, s:Wait("g:simplecc_status ==# 'ready'", 5000),
      \ 'a daemon that dies unexpectedly must be restarted automatically')

call simplecc#Stop()
call assert_equal(1, s:Wait("g:simplecc_status ==# ''", 3000))

" ------------------------------------------------------ opting out of restarts ---

let g:simplecc_auto_restart = 0
call simplecc#Start()
call assert_equal(1, s:Wait("g:simplecc_status ==# 'ready'", 2000))
SimpleCCReloadConfig
call assert_equal(1, s:Wait("g:simplecc_status ==# ''", 2000),
      \ 'the crash must still be reported when auto-restart is off')
sleep 400m
call assert_equal('', g:simplecc_status,
      \ 'g:simplecc_auto_restart = 0 must leave the daemon down')
let g:simplecc_auto_restart = 1

" An explicit restart still works, and clears whatever the breaker accumulated.
call simplecc#Restart()
call assert_equal(1, s:Wait("g:simplecc_status ==# 'ready'", 3000),
      \ ':SimpleCCRestart must start a daemon that is not running')

call simplecc#Stop()
call assert_equal(1, s:Wait("g:simplecc_status ==# ''", 3000))

" ------------------------------------------- a manual stop cancels the restart ---

" :SimpleCCStop used to return early whenever the daemon was not running, so it
" never reached core#Stop() -- the only thing that cancels a queued backoff
" restart. Typed during a crash loop, which is exactly when a user reaches for
" it, the daemon came straight back and there was no way to hold it down short
" of also setting g:simplecc_auto_restart = 0.
"
" The expected exit above reset the backoff, so crash repeatedly to widen the
" window this test has to type into: 100ms, 200ms, 400ms, then 800ms.
call simplecc#Start()
for s:attempt in range(3)
  call assert_equal(1, s:Wait("g:simplecc_status ==# 'ready'", 3000),
        \ 'the daemon must be up before crash ' .. s:attempt)
  SimpleCCReloadConfig
  call assert_equal(1, s:Wait("g:simplecc_status !=# 'ready'", 2000))
endfor
call assert_equal(1, s:Wait("g:simplecc_status ==# 'ready'", 3000))
SimpleCCReloadConfig
call assert_equal(1, s:Wait("!simplecc#core#IsRunning()", 2000),
      \ 'the daemon must be dead when the manual stop is issued')

" Still inside the backoff window: the restart is queued but has not fired.
call assert_false(simplecc#core#IsRunning(),
      \ 'the queued restart fired before the stop -- window too narrow to test')
call simplecc#Stop()

" Longer than the 800ms backoff that was queued, with margin.
sleep 2
call assert_false(simplecc#core#IsRunning(),
      \ ':SimpleCCStop must cancel a queued restart, not be undone by it')
call assert_equal('', g:simplecc_status,
      \ 'a stopped daemon must not keep reporting itself as restarting')

call delete(s:daemon)

if len(v:errors)
  call writefile(v:errors, s:root .. '/test/daemon-restart-errors.log')
  for s:e in v:errors
    echomsg s:e
  endfor
  cquit
endif
qall!
