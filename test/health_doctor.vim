" :SimpleCCHealth is a doctor, not a status line.
"
" It used to echo a dozen lines into the message area, name ~/.simplecc.json as
" the user config (a file it never reads), and say nothing about the two things
" that actually break installs: a lib/simplecc-daemon older than the Vim files
" a plugin manager just updated, and a config whose server commands are not
" executable.
"
" Run:  vim -Nu NONE -n -i NONE -es -S test/health_doctor.vim

set nocompatible
set encoding=utf-8
set nomore
set noswapfile

let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)
call delete(s:root .. '/test/health-doctor-errors.log')

let g:simplecc_auto_start = 0
let g:simplecc_no_default_maps = 1
runtime plugin/simplecc.vim
execute 'source ' .. fnameescape(s:root .. '/autoload/simplecc.vim')

" One line of the report matching a pattern, or '' when the report is silent
" about it -- which is the failure this file mostly guards against.
function! s:Line(pattern) abort
  for l:line in simplecc#HealthReport()
    if l:line =~# a:pattern
      return l:line
    endif
  endfor
  return ''
endfunction

" ---------------------------------------------------------------- structure ---

for s:section in ['ENVIRONMENT', 'BINARY', 'CONFIG', 'RUNTIME', 'CONTEXT']
  call assert_notequal('', s:Line('^' .. s:section),
        \ 'the report has a ' .. s:section .. ' section')
endfor
call assert_notequal('', s:Line('^\[OK\] encoding: utf-8'),
      \ 'LSP columns are UTF-16 over UTF-8, so the encoding is a fact worth stating')

" ------------------------------------------------------------- version skew ---

" The single most common failure in this suite: the plugin manager pulls new
" Vim files and the Rust daemon beside them is never rebuilt.
let s:daemon = tempname()
call writefile(['#!/bin/sh', 'cat > /dev/null'], s:daemon)
call assert_equal(1, setfperm(s:daemon, 'rwx------'))
let g:simplecc_daemon_path = s:daemon

call system('touch -t 200101010000 ' .. shellescape(s:daemon))
call assert_notequal('', s:Line('^\[ERROR\] daemon is older than the plugin'),
      \ 'a daemon older than the plugin sources must be reported as the error it is')
call assert_equal(1, s:Line('^\[ERROR\] daemon is older than the plugin')
      \ =~# 'run ./install.sh', 'and it must say what to do about it')

call system('touch -d "+1 day" ' .. shellescape(s:daemon))
call assert_notequal('', s:Line('^\[OK\] daemon is newer than every plugin source'),
      \ 'a freshly built daemon is not an error')


" ------------------------------------------------------- config validation ---

let s:cfg = tempname() .. '.json'
let g:simplecc_config_path = s:cfg

call writefile(['{"languageServers": {'], s:cfg)
call assert_notequal('', s:Line('^\[ERROR\] config is not valid JSON'),
      \ 'a config that does not parse is silently ignored by the daemon; say so')

call writefile([json_encode({'languageServers': {
      \ 'nope-lsp': {'command': 'definitely-not-on-this-path', 'filetypes': ['nope']},
      \ 'fine-lsp': {'command': 'sh', 'filetypes': ['sh']},
      \ 'vague-lsp': {'command': 'sh'}}})], s:cfg)
call assert_notequal('', s:Line('^\[ERROR\] server nope-lsp: command .* is not executable'),
      \ 'a server command that resolves nowhere is why nothing ever starts')
call assert_equal(1, s:Line('^\[ERROR\] server nope-lsp') =~# ':SimpleCCInstall nope-lsp',
      \ 'and the remedy names the server')
call assert_notequal('', s:Line('^\[OK\] server fine-lsp: .*\[sh\]'),
      \ 'a resolvable command is reported with the path and its filetypes')
call assert_notequal('', s:Line('^\[WARN\] server vague-lsp: no "filetypes"'),
      \ 'a server no buffer can select is a warning, not silence')

call writefile([json_encode({'languageServers': {'bad': {'filetypes': ['x']}}})], s:cfg)
call assert_notequal('', s:Line('^\[ERROR\] server bad: no "command"'))

let s:missing = tempname() .. '.json'
let g:simplecc_config_path = s:missing
call assert_notequal('', s:Line('^\[ERROR\] g:simplecc_config_path names a file'),
      \ 'a config path pointing at nothing is a typo worth reporting')
let g:simplecc_config_path = ''

" -------------------------------------------------------------- this buffer ---

call assert_notequal('', s:Line('^\[WARN\] filetype: (none)'),
      \ 'the report explains why the buffer you are in is inert')

let s:file = tempname() .. '.rs'
call writefile(['fn main() {}'], s:file)
execute 'edit! ' .. fnameescape(s:file)
setfiletype rust
call assert_notequal('', s:Line('^\[OK\] filetype: rust'))
call assert_notequal('', s:Line('^\[ERROR\] document: never opened on the server'),
      \ 'a buffer the daemon has never heard of is the answer to "why nothing works"')

" -------------------------------------------------------- remote workspace ---

" What the report says about a SimpleRemote workspace.  SimpleRemote is not
" on the runtimepath: g:simpleremote_workspace is set the way it publishes it.
let g:simplecc_python_state_file = tempname()

" A probe SimpleRemote has started but not answered yet is seeded as
" {status: -1}.  Reporting that as "no python on the remote PATH" sends the
" reader after a problem that does not exist.
let g:simpleremote_workspace = {'id': 7, 'kind': 'ssh', 'target': 'devbox',
      \ 'root': '/srv/app', 'tree_root': '/srv/app', 'local_root': '',
      \ 'mode': 'virtual', 'runtime': '', 'runtime_version': '',
      \ 'protocol': 'json', 'probe': {'status': -1}}
call assert_notequal('', s:Line('^REMOTE$'), 'the report has a REMOTE section')
call assert_notequal('', s:Line('^\[INFO\] runtime probe: not run yet'),
      \ 'a probe still in flight has not found nothing -- it has not answered')
call assert_equal('', s:Line('remote python: (none found)'),
      \ 'so nothing is claimed about the host''s python yet')

" A probe entry that is not a string (a truncated or hand-written snapshot)
" is no answer either -- and must not throw out of the middle of the report.
let g:simpleremote_workspace.probe = {'status': 0, 'python': 42, 'python_lsp': ''}
call assert_notequal('', s:Line('^\[WARN\] remote python: (none found)'),
      \ 'a probe value that is not a string is treated as no answer')

let g:simpleremote_workspace.probe = {'status': 0, 'python': '/usr/bin/python3',
      \ 'python_version': 'Python 3.12.1', 'python_lsp': '', 'uname': 'Linux'}
call assert_notequal('', s:Line('^\[OK\] remote python: /usr/bin/python3 (Python 3.12.1)'))
call assert_notequal('', s:Line('^\[INFO\] probe: uname=Linux'))
call assert_notequal('', s:Line('^\[INFO\] python selection: /usr/bin/python3 / (auto) (from the runtime probe)'))

" The buffer this report is about is a local file, and every server of a
" connected workspace runs on the host: it is not sent to any of them.
call assert_notequal('', s:Line('^\[WARN\] local file: outside the remote workspace'),
      \ 'the report says why a local buffer is inert while a workspace is connected')
call assert_equal(1, s:Line('^\[WARN\] local file:') =~# 'ssh:devbox',
      \ 'and names the host whose servers would have to serve it')
unlet g:simpleremote_workspace
call assert_equal('', s:Line('^\[WARN\] local file: outside the remote workspace'),
      \ 'while local, a local file is exactly what the daemon serves')
unlet g:simplecc_python_state_file

" --------------------------------------------------------- scratch rendering ---

call simplecc#Health()
call assert_equal('nofile', &buftype, 'the report lands in a scratch buffer')
call assert_equal(0, &modifiable, 'and is not editable')
call assert_equal(1, getline(1) =~# '^SimpleCC health',
      \ 'with the report in it: ' .. getline(1))
call assert_equal(1, line('$') > 15, 'the whole report, not a summary')

" Re-running is the normal way to use a health check: you change something and
" ask again. The second run used to abort with E95 on the buffer name, leaving
" the freshly split, empty window behind and the report unwritten.
let s:windows = winnr('$')
let s:first = bufnr('%')
let v:errmsg = ''
call simplecc#Health()
call assert_equal('', v:errmsg, ':SimpleCCHealth twice must not raise')
call assert_equal(s:windows, winnr('$'),
      \ 're-running must reuse the report window, not stack another one')
call assert_equal(s:first, bufnr('%'), 'and reuse its buffer')
call assert_equal(1, getline(1) =~# '^SimpleCC health',
      \ 'the reused buffer holds the new report, not an empty scratch: '
      \ .. getline(1))
call assert_equal(1, line('$') > 15, 'the whole report again, not a remnant')
call assert_equal(0, &modifiable, 'and is locked again afterwards')
bwipeout!

call delete(s:daemon)
call delete(s:cfg)
call delete(s:file)

if len(v:errors)
  call writefile(v:errors, s:root .. '/test/health-doctor-errors.log')
  for s:e in v:errors
    echomsg s:e
  endfor
  cquit
endif
qall!
