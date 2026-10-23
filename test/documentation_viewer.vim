set nocompatible encoding=utf-8 nomore
let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set rtp^=' .. fnameescape(s:root)
let g:simplecc_auto_start = 0
let g:simplecc_no_default_maps = 1
runtime plugin/simplecc.vim
runtime autoload/simplecc.vim
let s:sid = getscriptinfo({'name': 'autoload/simplecc.vim'})[0].sid
function! s:Signature(sigs) abort
  call call(function(printf('<SNR>%d_RememberPopupRequest', s:sid)), ['signature', 1])
  call call(function(printf('<SNR>%d_OnSignatureHelp', s:sid)), [{'id': 1, 'signatures': a:sigs}])
  return filter(popup_list(), {_, id -> getwinvar(id, 'simplecc_popup_kind', '') ==# 'signature'})[0]
endfunction
new
setlocal filetype=rust
call setline(1, 'source text')
let s:origin = bufnr('%')
let s:win = win_getid()
let s:undo = undotree()
let s:reg = getreginfo('"')
let s:label = '函数(参数😀,	参数😀)'
let s:start = strridx(s:label, '参数😀')
let s:sig = {'label': s:label, 'parameters': [{'label': '参数😀'}, {'label': '参数😀', 'byte_range': [s:start, s:start + strlen('参数😀')]}], 'active_parameter': 1, 'overload_index': 2, 'overload_count': 4, 'documentation': "Example\n```python\ndef greeting():\n    return 'hello'\n```\n```nonexistent_lang\nliteral\n```"}
for s:native in [0, 1]
  let g:jterm_vim9_render_protocol = 7
  let g:jterm_vim9_popup_cards = s:native ? v:true : v:false
  set columns=24
  let s:id = s:Signature([s:sig])
  let s:bnr = winbufnr(s:id)
  let s:props = []
  let s:highlighted = ''
  for s:ln in range(1, len(getbufline(s:bnr, 1, '$')))
    let s:lineprops = prop_list(s:ln, {'bufnr': s:bnr})
    call extend(s:props, s:lineprops)
    for s:prop in s:lineprops
      let s:highlighted .= strpart(getbufline(s:bnr, s:ln)[0], s:prop.col - 1, s:prop.length)
    endfor
  endfor
  call assert_equal('参数😀', s:highlighted, 'only the second repeated parameter is highlighted across wraps and tab expansion')
  call assert_equal(s:win, win_getid(), 'passive popup preserves focus')
  let s:pinned = simplecc#DocPin()
  call assert_true(s:pinned > 0)
  call assert_equal(s:label, getline(1), 'pin retains unwrapped source')
  call assert_equal('Overload 3 / 4', getline(2))
  call assert_equal(0, &l:modifiable)
  call assert_equal(1, &l:readonly)
  call assert_equal('nofile', &l:buftype)
  call assert_equal(s:origin, b:simplecc_doc_origin.bufnr)
  call assert_equal('pythonStatement', synIDattr(synID(6, 1, 1), 'name'), 'fenced Python syntax')
  call assert_equal('', synIDattr(synID(10, 1, 1), 'name'), 'unknown syntax stays plain')
  let @/ = 'greeting'
  call assert_equal(6, search(@/, 'w'), 'real buffer supports search')
  call assert_equal(s:reg, getreginfo('"'), 'pin does not change registers')
  close
  call win_gotoid(s:win)
  call assert_equal(s:win, win_getid())
  call assert_equal(s:undo.seq_last, undotree().seq_last, 'origin undo unchanged')
  call assert_equal('source text', getline(1))
endfor
" Older daemon repeated labels are resolved in parameter order.
call remove(s:sig.parameters[1], 'byte_range')
let s:id = s:Signature([s:sig])
let s:source_spans = getbufvar(winbufnr(s:id), 'simplecc_doc_spans')
call assert_equal(s:start, s:source_spans[0].start)
call simplecc#PopupClose()
call assert_equal(0, simplecc#DocPin())
if !empty(v:errors)
  call writefile(v:errors, s:root .. '/test/documentation-viewer-errors.log')
  cquit
endif
qa!
