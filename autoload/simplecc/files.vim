vim9script

# User initiated tree renames are deliberately separate from server resource
# operations. A preview never modifies source buffers or writes a file.
var pending: dict<any> = {}
var preview_buf = -1

def Fail(message: string)
  echohl ErrorMsg
  echom '[SimpleCC] ' .. message
  echohl None
enddef

def Request(req: dict<any>): dict<any>
  var result: dict<any> = {done: false, event: {}}
  simplecc#core#Request(req, (ev) => {
    result.event = ev
    result.done = true
  }, 5500)
  var start = reltime()
  while !result.done && reltimefloat(reltime(start)) < 6.0
    sleep 10m
  endwhile
  if !result.done || get(result.event, 'type', '') !=# 'fileRename'
    throw 'SimpleCC: ' .. get(result.event, 'message', 'rename request timed out; operation canceled')
  endif
  return result.event
enddef

def Exchange(state: dict<any>, phase: string): dict<any>
  if !simplecc#core#IsRunning() || simplecc#core#Health().starts != state.starts
    throw 'SimpleCC: daemon connection changed; rename canceled'
  endif
  return Request({type: 'workspace/fileRename', old: state.old_uri,
    new: state.new_uri, directory: state.directory, phase: phase,
    identity: get(state, 'identity', '')})
enddef

def RemoteRead(path: string): list<string>
  var result: dict<any> = {done: false, ok: false, body: []}
  g:SimpleRemoteReadFileLines(path, (ok, body) => {
    result.ok = ok
    result.body = body
    result.done = true
  })
  var start = reltime()
  while !result.done && reltimefloat(reltime(start)) < 6.0
    sleep 10m
  endwhile
  if !result.done || !result.ok
    throw 'SimpleCC: remote read failed or timed out: ' .. path
  endif
  return result.body
enddef

def Snapshot(path: string): dict<any>
  var remote = path =~# '^remote://'
  if !remote && (!filereadable(path) || getftype(path) !=# 'file')
    throw 'SimpleCC: rename preview requires a regular file: ' .. path
  endif
  # bufnr(string) falls back to a filename pattern when no literal name is
  # present: an unopened imports[1].py could otherwise edit imports1.py.
  var bnr = -1
  var absolute = remote ? path : simplify(fnamemodify(path, ':p'))
  var resolved = remote ? '' : resolve(absolute)
  var alias = -1
  for info in getbufinfo()
    if remote
      if get(getbufvar(info.bufnr, 'vimrc_remote', {}), 'uri', '') ==# path
          || info.name ==# path
        bnr = info.bufnr
        break
      endif
    elseif info.name !=# '' && info.name !~# '^remote://'
      var candidate = simplify(fnamemodify(info.name, ':p'))
      if candidate ==# absolute
        bnr = info.bufnr
        break
      endif
      if alias < 0 && resolve(candidate) ==# resolved
        alias = info.bufnr
      endif
    endif
  endfor
  if bnr < 0 && !remote
    bnr = alias
  endif
  if bnr < 0
    bnr = bufadd(path)
  endif
  bufload(bnr)
  if remote
    var start = reltime()
    while !empty(getbufvar(bnr, 'vimrc_remote_read', {})) && reltimefloat(reltime(start)) < 6.0
      sleep 10m
    endwhile
    var metadata = getbufvar(bnr, 'vimrc_remote', {})
    if !empty(getbufvar(bnr, 'vimrc_remote_read', {}))
        || get(metadata, 'generation', -2) != get(get(g:, 'simpleremote_workspace', {}), 'id', -1)
      throw 'SimpleCC: remote file is not loaded in the active connection: ' .. path
    endif
  endif
  if getbufvar(bnr, '&modified') || !getbufvar(bnr, '&modifiable') || getbufvar(bnr, '&readonly')
    throw 'SimpleCC: save changes / unlock buffer before renaming: ' .. path
  endif
  var disk = remote ? RemoteRead(path) : readfile(path, 'b')
  var disk_lines = copy(disk)
  if len(disk_lines) > 1 && disk_lines[-1] ==# ''
    remove(disk_lines, -1)
  endif
  if empty(disk_lines)
    disk_lines = ['']
  endif
  if getbufvar(bnr, '&fileformat') ==# 'dos'
    disk_lines = mapnew(disk_lines, (_, text) => substitute(text, '\r$', '', ''))
  endif
  if disk_lines != getbufline(bnr, 1, '$')
    throw 'SimpleCC: buffer differs from file on disk; reload before rename preview: ' .. path
  endif
  return {path: path, bnr: bnr, remote: remote, before: getbufline(bnr, 1, '$'),
    disk: disk, tick: getbufvar(bnr, 'changedtick')}
enddef

def Guard(state: dict<any>)
  if state.old =~# '^remote://'
    if get(get(g:, 'simpleremote_workspace', {}), 'id', -1) != state.remote_id
      throw 'SimpleCC: remote connection changed; rename canceled'
    endif
    for info in getbufinfo()
      var path = get(getbufvar(info.bufnr, 'vimrc_remote', {}), 'uri', '')
      var write = getbufvar(info.bufnr, 'vimrc_remote_write_state', {})
      if (path ==# state.old || (state.directory && stridx(path, state.old .. '/') == 0))
          && (getbufvar(info.bufnr, '&modified') || get(write, 'status', '') ==# 'pending')
        throw 'SimpleCC: remote source has unsaved changes or a pending write; operation canceled'
      endif
    endfor
    if !state.directory && has_key(state, 'source_disk') && RemoteRead(state.old) != state.source_disk
      throw 'SimpleCC: remote source changed during preview; rename canceled'
    endif
  else
    if getftype(state.old) !=# state.source_type || getftime(state.old) != state.source_time
        || (state.source_type ==# 'file' && readfile(state.old, 'b') != state.source_disk)
        || getftype(state.new) !=# ''
      throw 'SimpleCC: source or target changed during rename preview; operation canceled'
    endif
    for info in getbufinfo()
      var path = fnamemodify(info.name, ':p')->substitute('/$', '', '')
      if (path ==# state.old || (state.directory && stridx(path, state.old .. '/') == 0))
          && getbufvar(info.bufnr, '&modified')
        throw 'SimpleCC: source has unsaved changes; operation canceled'
      endif
    endfor
  endif
  for item in state.items
    if !bufexists(item.bnr) || getbufvar(item.bnr, 'changedtick') != item.tick
        || getbufvar(item.bnr, '&modified') || !getbufvar(item.bnr, '&modifiable')
        || getbufvar(item.bnr, '&readonly')
        || (item.remote ? RemoteRead(item.path) : readfile(item.path, 'b')) != item.disk
      throw 'SimpleCC: document changed during rename preview: ' .. item.path
    endif
  endfor
enddef

def PrepareItems(edits: list<any>): list<dict<any>>
  var by_path: dict<any> = {}
  for edit in edits
    for operation in get(edit, 'operations', [])
      if get(operation, 'kind', '') !=# 'edit'
        throw 'SimpleCC: rename preview cannot safely combine resource operations; operation canceled'
      endif
    endfor
    for change in get(edit, 'changes', [])
      var path = simplecc#UriToPath(change.uri)
      if has_key(by_path, path)
        throw 'SimpleCC: multiple servers edited the same file; operation canceled: ' .. path
      endif
      var item = Snapshot(path)
      # Apply to a hidden scratch buffer, validating every position first.
      var last_end = [-1, -1]
      for textedit in sort(copy(change.edits), (a, b) => a.line == b.line ? a.character - b.character : a.line - b.line)
        var sl = textedit.line
        var el = textedit.end_line
        if sl < 0 || el < sl || el >= len(item.before)
            || (sl == el && textedit.end_character < textedit.character)
            || simplecc#ByteOffsetToUtf16(item.before[sl], strlen(item.before[sl])) < textedit.character
            || simplecc#ByteOffsetToUtf16(item.before[el], strlen(item.before[el])) < textedit.end_character
            || simplecc#ByteOffsetToUtf16(item.before[sl], simplecc#Utf16ToByteOffset(item.before[sl], textedit.character)) != textedit.character
            || simplecc#ByteOffsetToUtf16(item.before[el], simplecc#Utf16ToByteOffset(item.before[el], textedit.end_character)) != textedit.end_character
            || sl < last_end[0] || (sl == last_end[0] && textedit.character < last_end[1])
          throw 'SimpleCC: invalid or overlapping rename edits: ' .. path
        endif
        last_end = [el, textedit.end_character]
      endfor
      var scratch = bufadd('')
      bufload(scratch)
      setbufvar(scratch, '&buftype', 'nofile')
      setbufline(scratch, 1, item.before)
      try
        simplecc#ApplyTextEdits(scratch, change.edits)
        item.after = getbufline(scratch, 1, '$')
      finally
        execute 'silent! bwipeout! ' .. scratch
      endtry
      if item.before != item.after
        by_path[path] = item
      endif
    endfor
  endfor
  return values(by_path)
enddef

def PreviewLines(state: dict<any>): list<string>
  var lines = ['Rename: ' .. state.old, '     -> ' .. state.new,
    'a: apply rename and imports edits   q: cancel',
    'Imports edits remain modified in buffers; save them normally.', '']
  for item in state.items
    add(lines, '--- ' .. item.path)
    add(lines, '+++ ' .. item.path)
    var first = 0
    while first < min([len(item.before), len(item.after)]) && item.before[first] ==# item.after[first]
      first += 1
    endwhile
    var ending = 0
    while ending < min([len(item.before), len(item.after)]) - first
        && item.before[-1 - ending] ==# item.after[-1 - ending]
      ending += 1
    endwhile
    add(lines, printf('@@ -%d,%d +%d,%d @@', first + 1, len(item.before) - first - ending,
      first + 1, len(item.after) - first - ending))
    for text in item.before[first : len(item.before) - ending - 1]
      add(lines, '-' .. text)
    endfor
    for text in item.after[first : len(item.after) - ending - 1]
      add(lines, '+' .. text)
    endfor
    add(lines, '')
  endfor
  return lines
enddef

export def Cancel()
  pending = {}
  if preview_buf > 0 && bufexists(preview_buf)
    execute 'silent! bwipeout! ' .. preview_buf
  endif
  preview_buf = -1
enddef

export def Apply()
  if empty(pending)
    return
  endif
  var state = pending
  pending = {}
  try
    Guard(state)
    if state.connected
      Exchange(state, 'check')
      Guard(state)
    endif
    # Close preview before moving: it can otherwise steal the tree's focus.
    if preview_buf > 0 && bufexists(preview_buf)
      execute 'silent! bwipeout! ' .. preview_buf
    endif
    preview_buf = -1
    if !state.Move()
      throw 'SimpleCC: file operation failed; imports edits were not applied'
    endif
    for item in state.items
      if !bufexists(item.bnr) || getbufline(item.bnr, 1, '$') != item.before
          || getbufvar(item.bnr, '&modified') || !getbufvar(item.bnr, '&modifiable')
          || getbufvar(item.bnr, '&readonly')
        throw 'SimpleCC: rename succeeded but a buffer changed during file-operation hooks; imports were not applied: ' .. item.path
      endif
    endfor
    for item in state.items
      if setbufline(item.bnr, 1, item.after) != 0
        throw 'SimpleCC: rename succeeded but imports buffer update failed: ' .. item.path
      endif
      setbufvar(item.bnr, '&buflisted', 1)
      if len(getbufline(item.bnr, 1, '$')) > len(item.after)
        deletebufline(item.bnr, len(item.after) + 1, '$')
      endif
    endfor
    if state.connected
      Exchange(state, 'did')
    endif
    if !empty(state.items)
      echom '[SimpleCC] renamed; imports edits are ready in ' .. len(state.items) .. ' modified buffers'
    endif
  catch
    Fail(v:exception)
  endtry
enddef

export def Begin(old: string, new: string, directory: bool, Move: func): bool
  if !empty(pending)
    Fail('another rename preview is open; apply or cancel it first')
    return false
  endif
  var state: dict<any> = {old: old, new: new, directory: directory, Move: Move, items: [], connected: false}
  try
    var use_backend = simplecc#core#IsRunning()
    if use_backend && !simplecc#FileRenameAvailable()
      if confirm('[SimpleCC] Update the backend for imports-aware rename. Rename without updating imports?', "&Rename only\n&Cancel", 2) != 1
        return false
      endif
      use_backend = false
      echom '[SimpleCC] legacy backend: imports will not be updated for this rename'
    endif
    if old =~# '^remote://'
      # Ordinary provider renames retain their own dirty/root/connection guards
      # and must not acquire a dependency on the new imports preview reader.
      if !use_backend
        return Move() ? true : false
      endif
      state.remote_id = get(get(g:, 'simpleremote_workspace', {}), 'id', -1)
      if state.remote_id < 0
        throw 'SimpleCC: remote rename integration requires an active SimpleRemote workspace'
      endif
    else
      state.old = fnamemodify(old, ':p')->substitute('/$', '', '')
      state.new = fnamemodify(new, ':p')->substitute('/$', '', '')
      state.source_type = getftype(state.old)
      state.source_time = getftime(state.old)
      state.source_disk = state.source_type ==# 'file' ? readfile(state.old, 'b') : []
      Guard(state)
    endif
    if use_backend
      state.connected = true
      state.starts = simplecc#core#Health().starts
      state.old_uri = simplecc#PathToUri(old)
      state.new_uri = simplecc#PathToUri(new)
      var reply = Exchange(state, 'prepare')
      state.identity = reply.identity
      var edits = get(reply, 'edits', [])
      if old =~# '^remote://' && !empty(edits)
        if !exists('*g:SimpleRemoteReadFileLines')
          if confirm('[SimpleCC] Update SimpleRemote for safe imports previews. Rename without updating imports?', "&Rename only\n&Cancel", 2) != 1
            return false
          endif
          echom '[SimpleCC] legacy SimpleRemote: imports will not be updated for this rename'
          return Move() ? true : false
        endif
        state.source_disk = directory ? [] : RemoteRead(old)
        Guard(state)
      endif
      state.items = PrepareItems(edits)
    endif
    pending = state
    if empty(state.items)
      Apply()
      return true
    endif
    execute 'botright new'
    preview_buf = bufnr('%')
    setlocal buftype=nofile bufhidden=wipe noswapfile nobuflisted filetype=diff
    setline(1, PreviewLines(state))
    setlocal nomodifiable readonly
    nnoremap <silent><buffer> a <Cmd>call simplecc#files#Apply()<CR>
    nnoremap <silent><buffer> q <Cmd>call simplecc#files#Cancel()<CR>
    nnoremap <silent><buffer> <Esc> <Cmd>call simplecc#files#Cancel()<CR>
    autocmd BufWipeout <buffer> call simplecc#files#PreviewClosed()
    return true
  catch
    pending = {}
    Fail(v:exception)
    return false
  endtry
enddef

export def PreviewClosed()
  pending = {}
  preview_buf = -1
enddef
