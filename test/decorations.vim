" Real Vim lifecycle and executable identity regression for SimpleCC text props.
set nocompatible
set encoding=utf-8
set nomore hidden noswapfile
let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)
call delete(s:root .. '/test/decorations-errors.log')
let g:simplecc_auto_start = 0
let g:simplecc_no_default_maps = 1
let g:simplecc_inlay_hints = 0
let g:simplecc_semantic_tokens = 0
runtime plugin/simplecc.vim
runtime autoload/simplecc.vim
let s:sid = getscriptinfo({'name': 'autoload/simplecc.vim'})[0].sid
function! s:Call(name, ...) abort
  return call(function(printf('<SNR>%d_%s', s:sid, a:name)), a:000)
endfunction
function! s:Wait(expr, ms) abort
  for l:i in range(a:ms / 10)
    if eval(a:expr) | return 1 | endif
    sleep 10m
  endfor
  return eval(a:expr)
endfunction
function! s:Props(buffer, type) abort
  return filter(prop_list(1, {'bufnr': a:buffer, 'end_lnum': -1}),
        \ {_, p -> p.type ==# a:type})
endfunction
let s:trace = tempname()
let s:daemon = tempname()
call writefile([], s:trace)
call writefile([
      \ '#!/usr/bin/env python3',
      \ 'import json, sys',
      \ 'for line in sys.stdin:',
      \ '    msg = json.loads(line)',
      \ '    with open(' .. json_encode(s:trace) .. ', "a") as out:',
      \ '        out.write(json.dumps(msg) + "\n")',
      \ '    if msg["type"] == "initialize":',
      \ '        print(json.dumps({"type":"initialized", "id":msg["id"]}), flush=True)',
      \ '    elif msg["type"] == "shutdown":',
      \ '        print(json.dumps({"type":"shutdown", "id":msg["id"]}), flush=True)',
      \ '        break',
      \ ], s:daemon)
call assert_equal(1, setfperm(s:daemon, 'rwx------'))
let g:simplecc_daemon_path = s:daemon
call simplecc#Start()
call assert_equal(1, s:Wait("g:simplecc_status ==# 'ready'", 2000))
let s:file_a = tempname() .. '.rs'
let s:file_b = tempname() .. '.rs'
call writefile(['let alpha = compute();', 'fn other() {}'], s:file_a)
call writefile(['let bravo = compute();', 'fn another() {}'], s:file_b)
execute 'edit ' .. fnameescape(s:file_a)
setfiletype rust
let s:buf_a = bufnr()
execute 'edit ' .. fnameescape(s:file_b)
setfiletype rust
let s:buf_b = bufnr()
call s:Call('StopFeatureTimers')
let g:decoration_events = 0
augroup simplecc_decoration_test
  autocmd!
  autocmd User SimpleCCDecorationsChanged let g:decoration_events += 1
augroup END
function! s:Buffer(buffer) abort
  execute 'buffer ' .. a:buffer
  call s:Call('StopFeatureTimers')
endfunction
function! s:HintRequest() abort
  call s:Call('RequestInlayHints')
  return s:Call('NextId') - 1
endfunction
function! s:HintReply(id, label) abort
  call s:Call('OnBackendEvent', {'type': 'inlayHint', 'id': a:id,
        \ 'hints': empty(a:label) ? [] : [{'line': 0, 'character': 9, 'label': a:label}]})
endfunction
function! s:LensRequest() abort
  call simplecc#CodeLens()
  return s:Call('NextId') - 1
endfunction
function! s:LensReply(id, buffer, titles, identity) abort
  let lenses = []
  for title in a:titles
    call add(lenses, {'line': 0, 'character': 0, 'end_line': 0,
          \ 'end_character': 1, 'command_title': title, 'index': len(lenses)})
  endfor
  let msg = {'type': 'codeLens', 'id': a:id, 'lenses': lenses}
  if a:identity
    let msg.generation = a:id
    let msg.uri = s:Call('BufUri', a:buffer)
  endif
  call s:Call('OnBackendEvent', msg)
endfunction
function! s:Executed() abort
  sleep 40m
  return filter(map(readfile(s:trace), {_, line -> json_decode(line)}),
        \ {_, msg -> msg.type ==# 'codeLens/execute'})
endfunction

" Inlay replies from two visible/loaded buffers have independent ownership.
let g:simplecc_inlay_hints = 1
call s:Buffer(s:buf_a)
let s:hint_a = s:HintRequest()
call s:Buffer(s:buf_b)
let s:hint_b = s:HintRequest()
call s:HintReply(s:hint_a, ': Alpha')
call s:HintReply(s:hint_b, ': Bravo')
call assert_equal(': Alpha', s:Props(s:buf_a, 'SimpleCCInlay')[0].text)
call assert_equal(': Bravo', s:Props(s:buf_b, 'SimpleCCInlay')[0].text)
call s:Buffer(s:buf_a)
call prop_remove({'type': 'SimpleCCInlay', 'bufnr': s:buf_a, 'all': 1})
call s:Call('RestoreInlayHints')
call assert_equal(': Alpha', s:Props(s:buf_a, 'SimpleCCInlay')[0].text,
      \ 'querying another buffer must not replace this buffer restore cache')
call setline(1, 'let changed = compute();')
call s:Call('RestoreInlayHints')
call assert_equal([], s:Props(s:buf_a, 'SimpleCCInlay'), 'an edited source cannot restore old hints')
let s:hint_a = s:HintRequest()
call s:HintReply(s:hint_a, ': Changed')
let s:pending = s:HintRequest()
call simplecc#InlayHintsToggle()
call assert_equal(0, g:simplecc_inlay_hints)
call assert_equal([], s:Props(s:buf_a, 'SimpleCCInlay'))
call assert_equal([], s:Props(s:buf_b, 'SimpleCCInlay'), 'turning hints off clears other buffers too')
call simplecc#InlayHintsToggle()
call assert_equal(1, g:simplecc_inlay_hints, 'a zero managed preference can be enabled by the command')
call s:HintReply(s:pending, ': Stale')
call assert_equal([], s:Props(s:buf_a, 'SimpleCCInlay'), 'off/on does not revive a pre-toggle reply')
call simplecc#InlayHintsToggle()

" Valid code lenses paint once, notify the host, and execute captured identity.
let s:before_events = g:decoration_events
let s:lens_a = s:LensRequest()
call s:LensReply(s:lens_a, s:buf_a, ['Run alpha'], 1)
call assert_equal(1, len(s:Props(s:buf_a, 'SimpleCCCodeLens')))
call assert_true(g:decoration_events > s:before_events)
call simplecc#CodeLensRun()
let s:executed = s:Executed()
call assert_equal(1, len(s:executed))
call assert_equal([s:lens_a, s:Call('BufUri', s:buf_a), 'rust', 0],
      \ [s:executed[0].generation, s:executed[0].uri, s:executed[0].languageId, s:executed[0].index])
call s:Buffer(s:buf_b)
call simplecc#CodeLensRun()
call assert_equal(1, len(s:Executed()), 'a lens never executes against the next buffer')
call s:Buffer(s:buf_a)
setlocal filetype=python
call simplecc#CodeLensRun()
call assert_equal(1, len(s:Executed()), 'a filetype change invalidates the cached server identity')
setlocal filetype=rust
call setline(1, 'let modified = compute();')
call simplecc#CodeLensRun()
call assert_equal(1, len(s:Executed()), 'changedtick is checked even before TextChanged is delivered')
call simplecc#OnTextChanged()
call assert_equal([], s:Props(s:buf_a, 'SimpleCCCodeLens'))

" An empty reply and a replacement request both retire executable indices.
let s:lens_a = s:LensRequest()
call s:LensReply(s:lens_a, s:buf_a, ['Old'], 1)
let s:before_events = g:decoration_events
let s:empty = s:LensRequest()
call assert_equal([], s:Props(s:buf_a, 'SimpleCCCodeLens'), 'requesting a fresh set clears old lenses')
call assert_true(g:decoration_events > s:before_events, 'the clearing emits a native refresh')
call s:LensReply(s:empty, s:buf_a, [], 1)
call simplecc#CodeLensRun()
call assert_equal(1, len(s:Executed()))
call s:LensReply(s:lens_a, s:buf_a, ['Late'], 1)
call assert_equal([], s:Props(s:buf_a, 'SimpleCCCodeLens'), 'late replies cannot replace a newer empty set')

" Menu callbacks revalidate after a new request or a buffer switch.
let s:menu_request = s:LensRequest()
call s:LensReply(s:menu_request, s:buf_a, ['One', 'Two'], 1)
call simplecc#CodeLensRun()
let s:menu = popup_list()[0]
let s:new_request = s:LensRequest()
call s:LensReply(s:new_request, s:buf_a, ['New'], 1)
call popup_close(s:menu, 1)
call assert_equal(1, len(s:Executed()), 'a menu cannot execute a replaced generation')
let s:menu_request = s:LensRequest()
call s:LensReply(s:menu_request, s:buf_a, ['One', 'Two'], 1)
call simplecc#CodeLensRun()
let s:menu = popup_list()[0]
call s:Buffer(s:buf_b)
call popup_close(s:menu, 1)
call assert_equal(1, len(s:Executed()), 'a menu cannot execute after moving to another document')

" New plugin with a legacy daemon remains displayable but never sends an
" unsafe index-only execution, since old daemons ignore extra request fields.
call s:Buffer(s:buf_a)
let s:legacy = s:LensRequest()
call s:LensReply(s:legacy, s:buf_a, ['Legacy'], 0)
call assert_equal(1, len(s:Props(s:buf_a, 'SimpleCCCodeLens')))
let s:message = execute('call simplecc#CodeLensRun()')
call assert_match('Rebuild the SimpleCC daemon', s:message)
call assert_equal(1, len(s:Executed()))

" A command's returned edit has its own context-bound event. Unknown, stale
" and replayed responses never fall through to generic workspace/applyEdit.
let s:request = s:LensRequest()
call s:LensReply(s:request, s:buf_a, ['Edit source'], 1)
call simplecc#CodeLensRun()
let s:execution = s:Executed()[-1]
let s:edit_event = {'type': 'codeLensExecute', 'id': s:execution.id,
      \ 'generation': s:execution.generation, 'uri': s:execution.uri,
      \ 'edit': {'changes': [{'uri': s:execution.uri, 'edits': [
      \ {'line': 1, 'character': 0, 'end_line': 1, 'end_character': 13,
      \ 'new_text': 'fn applied() {}'}]}]}}
call s:Call('OnBackendEvent', s:edit_event)
call assert_equal('fn applied() {}', getline(2), 'unchanged source accepts the command result')
call setline(2, 'fn user_kept() {}')
call s:Call('OnBackendEvent', s:edit_event)
call assert_equal('fn user_kept() {}', getline(2), 'a consumed execution cannot replay its edit')
let s:request = s:LensRequest()
call s:LensReply(s:request, s:buf_a, ['Delayed edit'], 1)
call simplecc#CodeLensRun()
let s:execution = s:Executed()[-1]
let s:edit_event.id = s:execution.id
let s:edit_event.generation = s:execution.generation
call setline(1, 'let preserved = compute();')
call s:Call('OnBackendEvent', s:edit_event)
call assert_equal('fn user_kept() {}', getline(2), 'changed source drops a delayed command edit')
let s:request = s:LensRequest()
call s:LensReply(s:request, s:buf_a, ['Superseded edit'], 1)
call simplecc#CodeLensRun()
let s:execution = s:Executed()[-1]
let s:edit_event.id = s:execution.id
let s:edit_event.generation = s:execution.generation
call s:LensRequest()
call s:Call('OnBackendEvent', s:edit_event)
call assert_equal('fn user_kept() {}', getline(2), 'replacing the lens query retires pending command edits')

" Unresolved server lenses may have null/missing/empty titles. They remain
" actionable through a labelled resolve menu instead of an invalid popup.
let s:request = s:LensRequest()
call s:LensReply(s:request, s:buf_a, [v:null, ''], 1)
call simplecc#CodeLensRun()
let s:menu = popup_list()[0]
call assert_equal(['(resolve code lens)', '(resolve code lens)'],
      \ getbufline(winbufnr(s:menu), 1, '$'))
call popup_close(s:menu, -1)

" A stalled daemon cannot grow the pending execution map without bound.
" An error consumes its request identity and releases one execution slot.
let s:request = s:LensRequest()
call s:LensReply(s:request, s:buf_a, ['Pending'], 1)
let s:before_count = len(s:Executed())
for s:i in range(1, 16)
  call simplecc#CodeLensRun()
endfor
call assert_equal(s:before_count + 16, len(s:Executed()))
let s:message = execute('call simplecc#CodeLensRun()')
call assert_match('Too many CodeLens commands are pending', s:message)
call assert_equal(s:before_count + 16, len(s:Executed()))
let s:failed = s:Executed()[-1]
call s:Call('OnBackendEvent', {'type': 'error', 'id': s:failed.id, 'message': 'fixture error'})
call simplecc#CodeLensRun()
call assert_equal(s:before_count + 17, len(s:Executed()), 'an error releases one pending slot')

" Virtual diagnostics use the same invalidation event when added or removed.
let g:simplecc_virtual_diag = 1
let s:before_events = g:decoration_events
call s:Call('DisplayVirtualDiag', s:buf_a, [{'line': 0, 'severity': 1, 'message': 'check this'}])
call assert_equal(s:before_events + 1, g:decoration_events)
call s:Call('DisplayVirtualDiag', s:buf_a, [])
call assert_equal(s:before_events + 2, g:decoration_events)

let s:pending = s:LensRequest()
call simplecc#Stop()
call assert_equal([], s:Props(s:buf_a, 'SimpleCCCodeLens'))
call s:LensReply(s:pending, s:buf_a, ['After stop'], 1)
call assert_equal([], s:Props(s:buf_a, 'SimpleCCCodeLens'), 'shutdown retires pending decoration replies')
call s:Wait("g:simplecc_status ==# ''", 2000)
call popup_clear()
call delete(s:daemon)
call delete(s:trace)
call delete(s:file_a)
call delete(s:file_b)
if !empty(v:errors)
  call writefile(v:errors, s:root .. '/test/decorations-errors.log')
  cquit
endif
qall!
