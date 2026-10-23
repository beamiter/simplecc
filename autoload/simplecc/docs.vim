" Documentation rendering stays independent of the transport and native host.
function! simplecc#docs#Decorate(id, source, spans) abort
  let bnr = winbufnr(a:id)
  if bnr < 0 | return | endif
  call setbufvar(bnr, 'simplecc_doc_source', copy(a:source))
  call setbufvar(bnr, 'simplecc_doc_spans', deepcopy(a:spans))
  call setbufvar(bnr, 'simplecc_doc_tabs', {'tabstop': &l:tabstop, 'vartabstop': &l:vartabstop})
  let rendered = getbufline(bnr, 1, '$')
  let preflowed = rendered !=# a:source
  " Project source byte offsets onto preflowed display text, including tabs.
  let rows = []
  let r = 0
  let col = 1
  for line in a:source
    call add(rows, [r + 1, col])
    let expanded = ''
    let byte = 0
    for ch in split(line, '\zs')
      let text = preflowed && ch ==# "\t" ? repeat(' ', strdisplaywidth(expanded .. ch) - strdisplaywidth(expanded)) : ch
      let expanded .= text
      for part in split(text, '\zs')
        if r < len(rendered) && col > strlen(rendered[r])
          let r += 1 | let col = 1
        endif
        for span in a:spans
          if span.line == len(rows) && byte >= span.start && byte < span.end && r < len(rendered)
            if empty(prop_type_get('SimpleCCActiveParameter', {'bufnr': bnr}))
              call prop_type_add('SimpleCCActiveParameter', {'bufnr': bnr, 'highlight': 'SimpleCCActiveParameter'})
            endif
            call prop_add(r + 1, col, {'bufnr': bnr, 'type': 'SimpleCCActiveParameter', 'length': strlen(part)})
          endif
        endfor
        let col += strlen(part)
      endfor
      let byte += strlen(ch)
    endfor
    let r += 1 | let col = 1
  endfor
  " Only shipped, explicitly supported syntax files are sourced. Unknown
  " languages retain literal text; server strings never become Ex commands.
  let aliases = {'py': 'python', 'python': 'python', 'rs': 'rust', 'rust': 'rust', 'c': 'c', 'cpp': 'cpp', 'js': 'javascript', 'javascript': 'javascript', 'ts': 'typescript', 'typescript': 'typescript', 'json': 'json', 'sh': 'sh', 'bash': 'sh', 'vim': 'vim', 'lua': 'lua', 'go': 'go'}
  let fence = -1
  let lang = ''
  for i in range(len(a:source))
    if a:source[i] =~# '^\s*```'
      if fence < 0
        let fence = i
        let lang = get(aliases, matchstr(a:source[i], '^\s*```\s*\zs\w\+'), '')
      else
        if lang !=# '' && i > fence + 1 && !empty(globpath(&runtimepath, 'syntax/' .. lang .. '.vim', 1, 1))
          let cluster = 'SimpleCCDoc' .. lang
          call win_execute(a:id, 'unlet! b:current_syntax | syntax include @' .. cluster .. ' syntax/' .. lang .. '.vim')
          let first = rows[fence + 1][0]
          let last = rows[i][0]
          call win_execute(a:id, 'syntax region SimpleCCDocFence start="\%' .. first .. 'l^" end="\%' .. last .. 'l^" keepend contains=@' .. cluster)
        endif
        let fence = -1
      endif
    endif
  endfor
endfunction

function! simplecc#docs#Pin(id) abort
  let bnr = winbufnr(a:id)
  if bnr < 0 | return 0 | endif
  let source = getbufvar(bnr, 'simplecc_doc_source', [])
  if empty(source) | return 0 | endif
  let spans = getbufvar(bnr, 'simplecc_doc_spans', [])
  let tabs = getbufvar(bnr, 'simplecc_doc_tabs', {'tabstop': &l:tabstop, 'vartabstop': &l:vartabstop})
  let origin = bufnr('%')
  let origin_name = bufname(origin)
  " No file path or server text is executed. A scratch split preserves the
  " source buffer, undo history, marks and registers while allowing /, y, etc.
  belowright new
  setlocal buftype=nofile bufhidden=wipe noswapfile nobuflisted
  let &l:tabstop = tabs.tabstop
  let &l:vartabstop = tabs.vartabstop
  call setline(1, source)
  let b:simplecc_doc_origin = {'bufnr': origin, 'name': origin_name}
  setlocal nomodifiable readonly nomodified
  call simplecc#docs#Decorate(win_getid(), source, spans)
  return win_getid()
endfunction
