" Buffer-word completion: correctness and the scan bound.
"
" This path runs on every keystroke in insert mode. It used to read the whole
" buffer and split every line into keywords; when the prefix matched nothing
" the early exit never fired, so a 60k-line file cost roughly a second per
" character typed -- which is exactly what happens while typing a new
" identifier. The scan is now bounded and pre-filtered with a substring test.
"
" Run:  vim -Nu NONE -n -i NONE -es -S test/buffer_words.vim

set nocompatible
set nomore

let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)
call delete(s:root .. '/test/buffer-words-errors.log')

let g:simplecc_daemon_path = '/nonexistent/simplecc-daemon'
let g:simplecc_no_default_maps = 1
runtime plugin/simplecc.vim

function! s:Collect(prefix, existing, limit) abort
  let l:sid = getscriptinfo({'name': 'autoload/simplecc.vim'})[0].sid
  return call(function(printf('<SNR>%d_CollectBufferWords', l:sid)),
        \ [a:prefix, a:existing, a:limit])
endfunction

function! s:Words(result) abort
  return map(copy(a:result), {_, v -> v.word})
endfunction

" ---------------------------------------------------------------- fixture ---

enew
setlocal buftype=nofile
let s:lines = []
for s:i in range(4000)
  call add(s:lines, printf('    let filler_%d = value_%d;', s:i, s:i))
endfor
" Words placed at known distances from the cursor, which sits at line 2000.
let s:lines[1999] = '    let cursorline_marker = 1;'
" beta sits 3 lines below the cursor, alpha 20 lines above, so the
" distance ordering is unambiguous in either direction.
let s:lines[1979] = '    let nearbyword_alpha = 1;'
let s:lines[2002] = '    let nearbyword_beta = 2;'
let s:lines[10]   = '    let distantword_gamma = 3;'
let s:lines[3990] = '    let distantword_delta = 4;'
call setline(1, s:lines)
call cursor(2000, 1)

" --------------------------------------------------------------- matching ---

let s:near = s:Words(s:Collect('nearbyword', {}, 20))
call assert_true(index(s:near, 'nearbyword_alpha') >= 0, 'a word just above the cursor is found')
call assert_true(index(s:near, 'nearbyword_beta') >= 0, 'a word just below the cursor is found')

" Results are ordered by distance from the cursor.
call assert_equal('nearbyword_beta', s:near[0], 'the nearest match comes first')

" A prefix matching nothing must come back empty rather than scanning forever.
call assert_equal([], s:Collect('nosuchprefix', {}, 20), 'a non-matching prefix yields nothing')

" Words no longer than the prefix are skipped -- this is what drops the word
" the user is in the middle of typing.
call assert_equal([], s:Collect('cursorline_marker', {}, 20),
      \ 'a word equal to the prefix is not offered back')

" Already-offered words (from the language server) are never duplicated.
let s:dup = s:Words(s:Collect('nearbyword', {'nearbyword_alpha': v:true}, 20))
call assert_false(index(s:dup, 'nearbyword_alpha') >= 0, 'server words are not duplicated')
call assert_true(index(s:dup, 'nearbyword_beta') >= 0, 'other matches still come through')

" The limit is honoured.
call assert_equal(3, len(s:Collect('filler_', {}, 3)), 'the item limit is respected')
call assert_equal([], s:Collect('filler_', {}, 0), 'a zero limit collects nothing')
call assert_equal([], s:Collect('', {}, 20), 'an empty prefix collects nothing')

" ------------------------------------------------------------- scan bound ---

" With a tight cap, a word far from the cursor is out of the window; with a
" cap wide enough to reach it, it comes back. This is what keeps the cost
" bounded no matter how large the buffer is.
let g:simplecc_complete_buffer_max_lines = 40
call assert_equal([], s:Collect('distantword', {}, 20),
      \ 'a distant word is outside a tight scan window')
call assert_true(index(s:Words(s:Collect('nearbyword', {}, 20)), 'nearbyword_beta') >= 0,
      \ 'a nearby word is still found with a tight window')

let g:simplecc_complete_buffer_max_lines = 100000
let s:wide = s:Words(s:Collect('distantword', {}, 20))
call assert_true(index(s:wide, 'distantword_gamma') >= 0, 'a wide window reaches distant words')
call assert_true(index(s:wide, 'distantword_delta') >= 0, 'a wide window reaches both directions')

" The bound must actually bound: scanning with a tight cap has to be
" dramatically cheaper than scanning the whole buffer, even when nothing
" matches and the early exit never fires.
let g:simplecc_complete_buffer_max_lines = 100000
let s:t = reltime()
for s:i in range(3)
  call s:Collect('nosuchprefix', {}, 20)
endfor
let s:unbounded = reltimefloat(reltime(s:t))

let g:simplecc_complete_buffer_max_lines = 200
let s:t = reltime()
for s:i in range(3)
  call s:Collect('nosuchprefix', {}, 20)
endfor
let s:bounded = reltimefloat(reltime(s:t))

call assert_true(s:bounded * 4 < s:unbounded,
      \ printf('the scan cap must bound the work (bounded %.4fs vs unbounded %.4fs)',
      \        s:bounded, s:unbounded))

unlet g:simplecc_complete_buffer_max_lines

if len(v:errors)
  call writefile(v:errors, s:root .. '/test/buffer-words-errors.log')
  for s:e in v:errors
    echomsg s:e
  endfor
  cquit
endif
qall!
