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

" --------------------------------------------------------- scratch rendering ---

call simplecc#Health()
call assert_equal('nofile', &buftype, 'the report lands in a scratch buffer')
call assert_equal(0, &modifiable, 'and is not editable')
call assert_equal(1, getline(1) =~# '^SimpleCC health',
      \ 'with the report in it: ' .. getline(1))
call assert_equal(1, line('$') > 15, 'the whole report, not a summary')
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
