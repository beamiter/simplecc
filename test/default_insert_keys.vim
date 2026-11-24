" Exercise the standalone default insert maps (not the combined vimrc).
set nocompatible encoding=utf-8 nomore
set noautoindent nosmartindent nocindent noexpandtab
execute 'set runtimepath^=' .. fnameescape(fnamemodify(expand('<sfile>:p'), ':h:h'))
let g:simplecc_auto_start = 0
runtime plugin/simplecc.vim

let s:expected = {
      \ '<Tab>': 'tab',
      \ '<S-Tab>': 'shift-tab',
      \ '<Down>': 'down',
      \ '<Up>': 'up',
      \ '<CR>': 'enter',
      \ }
for [s:key, s:target] in items(s:expected)
  let s:map = maparg(s:key, 'i', 0, 1)
  call assert_equal('<Plug>(simplecc-select-' .. s:target .. ')', get(s:map, 'rhs', ''), s:key)
  call assert_equal(0, get(s:map, 'noremap', -1), s:key .. ' must expand its <Plug> mapping')
endfor

" Keep Vim's own key behavior when the popup is closed. Feeding these as
" typed, remappable keys catches an accidental space that exact metadata also
" detects, and shows that deleting the maps is not a valid fix.
call setline(1, 'abc')
call feedkeys("A\<CR>x\<Esc>", 'xt')
call assert_equal(['abc', 'x'], getline(1, '$'), 'Enter must not insert a space')
silent %delete _
call setline(1, 'abc')
call feedkeys("A\<Tab>x\<Esc>", 'xt')
call assert_equal(["abc\tx"], getline(1, '$'), 'Tab must not insert a space')
silent %delete _
call setline(1, 'abc')
call feedkeys("A\<S-Tab>x\<Esc>", 'xt')
let s:shift_tab_mapped = getline(1, '$')
iunmap <S-Tab>
silent %delete _
call setline(1, 'abc')
call feedkeys("A\<S-Tab>x\<Esc>", 'xt')
call assert_equal(getline(1, '$'), s:shift_tab_mapped, 'Shift-Tab must preserve native fallback')

silent %delete _
call setline(1, ['one', 'two'])
call cursor(1, 1)
call feedkeys("i\<Down>x\<Esc>", 'xt')
call assert_equal(['one', 'xtwo'], getline(1, '$'), 'Down must not insert a space')
silent %delete _
call setline(1, ['one', 'two'])
call cursor(2, 1)
call feedkeys("i\<Up>y\<Esc>", 'xt')
call assert_equal(['yone', 'two'], getline(1, '$'), 'Up must not insert a space')

if !empty(v:errors)
  call writefile(v:errors, '/dev/stderr')
  cquit 1
endif
qa!
