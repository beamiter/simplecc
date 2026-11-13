" A pre-trust daemon must not open buffers using project-supplied commands.
set nocompatible
set encoding=utf-8
set nomore
let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)
let g:simplecc_auto_start = 0
let g:simplecc_no_default_maps = 1
runtime plugin/simplecc.vim
function! s:WaitStopped() abort
  let l:deadline = reltime()
  while reltimefloat(reltime(l:deadline)) < 10
    if filereadable(s:trace) && len(readfile(s:trace)) >= 2 && g:simplecc_status ==# ''
      return
    endif
    sleep 20m
  endwhile
endfunction
let s:workspace = tempname()
call mkdir(s:workspace, 'p')
call writefile(['{}'], s:workspace .. '/simplecc.json')
call writefile([], s:workspace .. '/Cargo.toml')
call writefile(['fn main() {}'], s:workspace .. '/main.rs')
let s:trace = tempname()
let s:daemon = tempname()
call writefile(['#!/usr/bin/env python3',
      \ 'import sys,json',
      \ 'for line in sys.stdin:',
      \ ' with open(' .. string(s:trace) .. ', "a") as f: f.write(line)',
      \ ' m=json.loads(line)',
      \ ' if m["type"]=="initialize": open(' .. string(s:workspace .. '/simplecc.json') .. ', "w").write("{}")',
      \ ' if m["type"]=="initialize": print(json.dumps({"type":"initialized","id":m["id"]}),flush=True)',
      \ ' if m["type"]=="shutdown": break'], s:daemon)
call setfperm(s:daemon, 'rwx------')
let g:simplecc_daemon_path = s:daemon
execute 'edit ' .. fnameescape(s:workspace .. '/main.rs')
setfiletype rust
" Headless confirm selects its default Cancel: no daemon starts, no approval.
SimpleCCTrustProject
call assert_equal(0, filereadable(s:trace), 'cancelling must not launch a daemon')
" The project file appears only AFTER initialize is sent: fail closed anyway.
call delete(s:workspace .. '/simplecc.json')
call simplecc#Start()
call s:WaitStopped()
call assert_notequal('ready', g:simplecc_status, 'old daemon must fail closed')
let s:messages = map(readfile(s:trace), {_, line -> json_decode(line)})
call assert_equal(1, len(filter(copy(s:messages), {_, m -> m.type ==# 'initialize'})))
call assert_equal(0, len(filter(copy(s:messages), {_, m -> m.type ==# 'textDocument/didOpen'})))
call assert_equal(v:null, s:messages[0].project_approval, 'no implicit approval')
SimpleCCUntrustProject
call assert_equal(2, exists(':SimpleCCTrustProject'))
" Remote config absent must still require the new protocol, regardless of
" whether an old daemon could discover a same-named local configuration.
call writefile([], s:trace)
let g:simpleremote_workspace = {'id': 1, 'kind': 'ssh', 'target': 'example.invalid', 'root': s:workspace}
unlet! g:vimrc_remote_simplecc_config
call simplecc#Start()
call s:WaitStopped()
call assert_notequal('ready', g:simplecc_status, 'remote discovery must fail closed')
let s:messages = map(readfile(s:trace), {_, line -> json_decode(line)})
call assert_equal(1, len(filter(copy(s:messages), {_, m -> m.type ==# 'initialize'})))
call assert_equal(0, len(filter(copy(s:messages), {_, m -> m.type ==# 'textDocument/didOpen'})))
call simplecc#Stop()
unlet! g:simpleremote_workspace
call delete(s:workspace, 'rf')
call delete(s:daemon)
call delete(s:trace)
if len(v:errors)
  call writefile(v:errors, s:root .. '/test/project-trust-errors.log')
  cquit
endif
qall!
