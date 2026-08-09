" Vim's own extension points route through the language server.
"
" Everything the plugin does was reachable only through its own commands and
" mappings. <C-x><C-o>, <C-]> and gq -- the keys a Vim user reaches for without
" having read a README, and the ones the tag stack and 'formatoptions' are
" built on -- did whatever they would have done with no LSP client installed.
"
" Run:  vim -Nu NONE -n -i NONE -es -S test/native_options.vim

set nocompatible
set encoding=utf-8
set nomore
set noswapfile

let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)
call delete(s:root .. '/test/native-options-errors.log')

let g:simplecc_auto_start = 0
let g:simplecc_no_default_maps = 1
runtime plugin/simplecc.vim
execute 'source ' .. fnameescape(s:root .. '/autoload/simplecc.vim')

let s:sid = getscriptinfo({'name': 'autoload/simplecc.vim'})[0].sid

function! s:Call(name, ...) abort
  return call(function(printf('<SNR>%d_%s', s:sid, a:name)), a:000)
endfunction

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

" A daemon that answers `initialize`, answers `textDocument/definition` with a
" fixed location, and records everything else.
let s:trace = tempname()
let s:target = tempname() .. '.rs'
call writefile(['fn zero() {}', 'fn one() {}', 'pub fn thing() {}'], s:target)
let s:daemon = tempname()
call writefile([
      \ '#!/usr/bin/env bash',
      \ 'set -euo pipefail',
      \ 'while IFS= read -r line; do',
      \ '  id="$(printf ''%s\n'' "$line" | sed -n ''s/.*"id":\([0-9][0-9]*\).*/\1/p'')"',
      \ '  case "$line" in',
      \ '    *''"type":"initialize"''*)',
      \ '      printf ''{"type":"initialized","id":%s}\n'' "$id"',
      \ '      ;;',
      \ '    *''"type":"textDocument/didOpen"''*)',
      \ '      printf ''%s\n'' "$line" >> ' .. shellescape(s:trace),
      \ '      printf ''{"type":"serverStatus","server":"rust-analyzer",' ..
      \        '"status":"running","filetypes":["rust"],"capabilities":{}}\n''',
      \ '      ;;',
      \ '    *''"type":"shutdown"''*)',
      \ '      printf ''{"type":"shutdown","id":%s}\n'' "$id"',
      \ '      exit 0',
      \ '      ;;',
      \ '    *''"type":"textDocument/definition"''*)',
      \ '      printf ''%s\n'' "$line" >> ' .. shellescape(s:trace),
      \ '      printf ''{"type":"definition","id":%s,"locations":' ..
      \        '[{"uri":"file://' .. s:target .. '","line":2,"character":7}]}\n'' "$id"',
      \ '      ;;',
      \ '    *)',
      \ '      printf ''%s\n'' "$line" >> ' .. shellescape(s:trace),
      \ '      ;;',
      \ '  esac',
      \ 'done',
      \ ], s:daemon)
call assert_equal(1, setfperm(s:daemon, 'rwx------'))
let g:simplecc_daemon_path = s:daemon

function! s:Traced(type) abort
  call s:Wait('!empty(filter(readfile(' .. string(s:trace) .. '), {_, l -> l =~# ' ..
        \ string('"type":"' . a:type . '"') .. '}))', 2000)
  for l:line in reverse(readfile(s:trace))
    let l:msg = json_decode(l:line)
    if get(l:msg, 'type', '') ==# a:type
      return l:msg
    endif
  endfor
  return {}
endfunction

call writefile([], s:trace)
call simplecc#Start()
call assert_equal(1, s:Wait("g:simplecc_status ==# 'ready'", 2000))

let s:file = tempname() .. '.rs'
call writefile(['use crate::thing;', 'let value = thing();'], s:file)
execute 'edit! ' .. fnameescape(s:file)
setfiletype rust
call s:Wait('0', 100)

" ------------------------------------------------------------ set on attach ---

call assert_equal('simplecc#OmniFunc', &l:omnifunc,
      \ '<C-x><C-o> must reach the language server in a served buffer')
call assert_equal('simplecc#TagFunc', &l:tagfunc, '<C-]> must reach the server')
call assert_equal('simplecc#FormatExpr()', &l:formatexpr, 'gq must reach the server')

" ----------------------------------------------------------------- omnifunc ---

call cursor(2, 17)
call assert_equal(12, simplecc#OmniFunc(1, ''),
      \ 'findstart reports the byte where the keyword before the cursor begins')
call writefile([], s:trace)
call assert_equal([], simplecc#OmniFunc(0, 'thi'),
      \ 'the reply is asynchronous, so nothing is returned synchronously')
call assert_notequal({}, s:Traced('textDocument/completion'),
      \ 'CTRL-X CTRL-O must have asked the server')

" ------------------------------------------------------------------ tagfunc ---

call cursor(2, 13)
let s:tags = simplecc#TagFunc('thing', 'c', {})
call assert_equal(1, type(s:tags) == v:t_list && len(s:tags) == 1,
      \ 'CTRL-] on an identifier is answered from the server: ' .. string(s:tags))
call assert_equal(s:target, s:tags[0].filename)
call assert_equal('3', s:tags[0].cmd,
      \ 'the tag jumps to the line the server named, not to a re-search')
call assert_equal('thing', s:tags[0].name)

" A definition is a position: everything else belongs to the tags file.
call assert_equal(v:null, simplecc#TagFunc('somethingElse', 'c', {}),
      \ ':tag typed by hand must fall back to the tags file')
call assert_equal(v:null, simplecc#TagFunc('thing', 'ir', {}),
      \ 'insert-mode tag completion must fall back to the tags file')

" --------------------------------------------------------------- formatexpr ---

let v:lnum = 1
call writefile([], s:trace)
call assert_equal(0, simplecc#FormatExpr(),
      \ 'gq is handled here, so Vim must not also reformat internally')
let s:msg = s:Traced('textDocument/rangeFormatting')
call assert_notequal({}, s:msg, 'gq asks the server to format the range')
call assert_equal(0, s:msg.line, 'the range starts at the line gq was given')

" A server without range formatting has to give the key back to Vim, or gq
" stops working on comment blocks in every language that lacks it.
call s:Call('OnServerStatus', {'server': 'rust-analyzer', 'status': 'running',
      \ 'capabilities': {'range_formatting': v:false, 'definition': v:false}})
call assert_equal(1, simplecc#FormatExpr(),
      \ 'gq falls back to Vim when the server never offered range formatting')
call assert_equal(v:null, simplecc#TagFunc('thing', 'c', {}),
      \ 'CTRL-] falls back to the tags file when the server has no definitions')

" A daemon too old to report capabilities must not turn everything off.
call s:Call('OnServerStatus', {'server': 'rust-analyzer', 'status': 'running'})
call assert_equal(0, simplecc#FormatExpr(),
      \ 'an unreported capability means "try it", not "refuse"')

" ------------------------------------------------------- a buffer of its own ---

" g:simplecc_native_options = 1 never overwrites a value the buffer already has.
let s:other = tempname() .. '.rs'
call writefile(['fn other() {}'], s:other)
execute 'edit! ' .. fnameescape(s:other)
setlocal omnifunc=SomeoneElse
setfiletype rust
call s:Wait('0', 100)
call assert_equal('SomeoneElse', &l:omnifunc,
      \ 'a filetype plugin that set omnifunc must win by default')
call assert_equal('simplecc#TagFunc', &l:tagfunc,
      \ 'the options it did not set are still taken')

" ------------------------------------------------- a filetype with no server ---

" The hooks are per-buffer, and "is there a server" is a different question
" from "can that server format a range". Consulting every running server made a
" Rust session claim gq in a markdown buffer -- rust-analyzer advertises range
" formatting, no server had ever heard of markdown -- and gq then reformatted
" nothing at all, because there was nowhere to send the range.
let s:prose = tempname() .. '.md'
call writefile(['aaa bbb ccc ddd eee fff ggg hhh iii jjj kkk lll mmm nnn ooo ppp'],
      \ s:prose)
execute 'edit! ' .. fnameescape(s:prose)
setfiletype markdown
call s:Wait('0', 100)

call assert_equal('', &l:formatexpr,
      \ 'a buffer no server serves must keep the formatexpr its ftplugin chose')
call assert_equal('', &l:omnifunc, 'and its omnifunc')
call assert_equal('', &l:tagfunc, 'and its tagfunc')

let v:lnum = 1
call assert_equal(1, s:Call('ServerSupports', 'range_formatting', 'markdown') ? 0 : 1,
      \ 'no server serves markdown, whatever rust-analyzer advertises')
call assert_equal(1, simplecc#FormatExpr(),
      \ 'gq in an unserved buffer must hand the key back to Vim')

" The observable consequence, not just the return value: gq has to wrap.
setlocal textwidth=20
normal! gqq
call assert_equal(
      \ ['aaa bbb ccc ddd eee', 'fff ggg hhh iii jjj', 'kkk lll mmm nnn ooo', 'ppp'],
      \ getline(1, '$'),
      \ 'gq must still reformat a paragraph in a filetype with no server')

" A served filetype in the same session is unaffected by the above.
call assert_equal(1, s:Call('ServerSupports', 'definition', 'rust') ? 1 : 0,
      \ 'the rust buffers in this same session still reach their server')

" With a dead daemon every hook has to hand the key straight back.
call simplecc#Stop()
call s:Wait("g:simplecc_status ==# ''", 2000)
call assert_equal(1, simplecc#FormatExpr(), 'gq works without a daemon')
call assert_equal(v:null, simplecc#TagFunc('other', 'c', {}),
      \ 'CTRL-] works without a daemon')
call assert_equal([], simplecc#OmniFunc(0, ''), 'CTRL-X CTRL-O without a daemon')

" ------------------------------------------------------------ <Plug> targets ---

" A user who took control of the mappings with g:simplecc_no_default_maps must
" still be able to reach every feature by name rather than by command.
for s:plug in ['definition', 'references', 'hover', 'rename', 'code-action',
      \ 'format', 'selection-expand', 'selection-shrink', 'prev-diagnostic',
      \ 'next-diagnostic', 'show-diagnostic', 'diagnostics',
      \ 'diagnostics-workspace', 'pull-diagnostics', 'implementation',
      \ 'type-definition', 'outline', 'inlay-hints', 'signature-help',
      \ 'document-highlight', 'document-highlight-clear', 'incoming-calls',
      \ 'outgoing-calls', 'supertypes', 'subtypes', 'code-lens',
      \ 'code-lens-run', 'fold', 'semantic-tokens', 'workspace-symbol',
      \ 'workspace-symbol-live', 'health', 'restart']
  call assert_notequal('', maparg('<Plug>(simplecc-' .. s:plug .. ')', 'n'),
        \ '<Plug>(simplecc-' .. s:plug .. ') must exist')
endfor

call delete(s:daemon)
call delete(s:trace)
call delete(s:file)
call delete(s:other)
call delete(s:prose)
call delete(s:target)

if len(v:errors)
  call writefile(v:errors, s:root .. '/test/native-options-errors.log')
  for s:e in v:errors
    echomsg s:e
  endfor
  cquit
endif
qall!
