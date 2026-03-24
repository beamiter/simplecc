vim9script

# ─────────────────────────────────────────────────────────
# simplecc autoload - Backend communication & LSP UI
# ─────────────────────────────────────────────────────────

# ═════════════════════════════════════════════════════════
# Backend (daemon) communication
# ═════════════════════════════════════════════════════════

var s_job: job = null_job
var s_running: bool = false
var s_initialized: bool = false
var s_next_id: number = 0
var s_cbs: dict<func> = {}
var s_root: string = ''
var s_log: list<string> = []

# Diagnostics state per URI
var s_diagnostics: dict<list<dict<any>>> = {}
# Document versions
var s_doc_versions: dict<number> = {}
# Change timer for debouncing
var s_change_timer: number = 0
# Completion timer
var s_comp_timer: number = 0
# Completion state
var s_comp_id: number = 0
var s_comp_requesting: bool = false
# Signature help popup
var s_sig_popup: number = 0
# Hover popup
var s_hover_popup: number = 0

def NextId(): number
  s_next_id += 1
  return s_next_id
enddef

def Log(msg: string)
  add(s_log, strftime('%H:%M:%S') .. ' ' .. msg)
  if len(s_log) > 500
    s_log = s_log[-300 :]
  endif
enddef

export def ShowLog()
  new
  setlocal buftype=nofile bufhidden=wipe noswapfile
  setline(1, s_log)
  setlocal nomodifiable
  normal! G
enddef

def FindBackend(): string
  var path = get(g:, 'simplecc_daemon_path', '')
  if path !=# '' && executable(path)
    return path
  endif
  # Search in runtimepath/lib/
  for dir in split(&runtimepath, ',')
    var p = dir .. '/lib/simplecc-daemon'
    if executable(p)
      return p
    endif
  endfor
  return ''
enddef

def IsRunning(): bool
  return s_running && s_job != null_job && job_status(s_job) ==# 'run'
enddef

def EnsureBackend(): bool
  if IsRunning()
    return true
  endif
  var exe = FindBackend()
  if exe ==# '' || !executable(exe)
    echohl ErrorMsg
    echom '[SimpleCC] daemon not found. Run install.sh or set g:simplecc_daemon_path.'
    echohl None
    return false
  endif

  try
    s_job = job_start([exe], {
      in_io: 'pipe',
      out_mode: 'nl',
      out_cb: (ch, line) => {
        OnBackendEvent(line)
      },
      err_mode: 'nl',
      err_cb: (ch, line) => {
        Log('stderr: ' .. line)
      },
      exit_cb: (ch, code) => {
        s_running = false
        s_initialized = false
        s_job = null_job
        s_cbs = {}
        Log('daemon exited with code ' .. string(code))
        g:simplecc_status = ''
      },
      stoponexit: 'term'
    })
  catch
    s_job = null_job
    s_running = false
    echohl ErrorMsg
    echom '[SimpleCC] job_start failed: ' .. v:exception
    echohl None
    return false
  endtry

  if job_status(s_job) !=# 'run'
    s_running = false
    return false
  endif

  s_running = true
  Log('daemon started')
  return true
enddef

def Send(req: dict<any>)
  if !IsRunning()
    return
  endif
  try
    var json = json_encode(req) .. "\n"
    ch_sendraw(s_job, json)
  catch
    Log('Send error: ' .. v:exception)
  endtry
enddef

def SendWithCb(req: dict<any>, Cb: func)
  var id = get(req, 'id', 0)
  if id > 0
    s_cbs[id] = Cb
  endif
  Send(req)
enddef

def OnBackendEvent(line: string)
  if line ==# ''
    return
  endif
  var ev: any
  try
    ev = json_decode(line)
  catch
    Log('JSON decode error: ' .. v:exception)
    return
  endtry

  if type(ev) != v:t_dict || !has_key(ev, 'type')
    return
  endif

  var id = get(ev, 'id', 0)

  if ev.type ==# 'initialized'
    s_initialized = true
    g:simplecc_status = 'ready'
    Log('initialized')
    # Open all current buffers
    for b in getbufinfo({'buflisted': 1, 'bufloaded': 1})
      if b.name !=# '' && filereadable(b.name)
        SendDidOpen(b.bufnr)
      endif
    endfor

  elseif ev.type ==# 'completion'
    OnCompletion(ev)

  elseif ev.type ==# 'hover'
    OnHover(ev)

  elseif ev.type ==# 'definition'
    OnDefinition(ev)

  elseif ev.type ==# 'references'
    OnReferences(ev)

  elseif ev.type ==# 'codeAction'
    OnCodeAction(ev)

  elseif ev.type ==# 'formatting'
    OnFormatting(ev)

  elseif ev.type ==# 'rename' || ev.type ==# 'applyEdit'
    OnApplyEdit(ev)

  elseif ev.type ==# 'signatureHelp'
    OnSignatureHelp(ev)

  elseif ev.type ==# 'diagnostics'
    OnDiagnostics(ev)

  elseif ev.type ==# 'serverStatus'
    OnServerStatus(ev)

  elseif ev.type ==# 'implementation'
    OnImplementation(ev)

  elseif ev.type ==# 'typeDefinition'
    OnTypeDefinition(ev)

  elseif ev.type ==# 'documentSymbol'
    OnDocumentSymbol(ev)

  elseif ev.type ==# 'workspaceSymbol'
    OnWorkspaceSymbol(ev)

  elseif ev.type ==# 'documentHighlight'
    OnDocumentHighlightResult(ev)

  elseif ev.type ==# 'inlayHint'
    OnInlayHints(ev)

  elseif ev.type ==# 'callHierarchyPrepare'
    OnCallHierarchyPrepare(ev)

  elseif ev.type ==# 'incomingCalls'
    OnIncomingCallsResult(ev)

  elseif ev.type ==# 'outgoingCalls'
    OnOutgoingCallsResult(ev)

  elseif ev.type ==# 'selectionRange'
    OnSelectionRange(ev)

  elseif ev.type ==# 'semanticTokens'
    OnSemanticTokens(ev)

  elseif ev.type ==# 'codeLens'
    OnCodeLens(ev)

  elseif ev.type ==# 'foldingRange'
    OnFoldingRange(ev)

  elseif ev.type ==# 'linkedEditingRange'
    OnLinkedEditingRange(ev)

  elseif ev.type ==# 'progress'
    OnProgress(ev)

  elseif ev.type ==# 'installProgress'
    OnInstallProgress(ev)

  elseif ev.type ==# 'installResult'
    OnInstallResult(ev)

  elseif ev.type ==# 'installableServers'
    OnInstallableServers(ev)

  elseif ev.type ==# 'showMessage'
    var lvl = get(ev, 'level', 'info')
    var msg = get(ev, 'message', '')
    if lvl ==# 'error'
      echohl ErrorMsg | echom '[LSP] ' .. msg | echohl None
    else
      echo '[LSP] ' .. msg
    endif

  elseif ev.type ==# 'log'
    Log('[' .. get(ev, 'server', '') .. '] ' .. get(ev, 'message', ''))

  elseif ev.type ==# 'error'
    Log('error(id=' .. string(id) .. '): ' .. get(ev, 'message', ''))

  elseif ev.type ==# 'shutdown'
    Log('shutdown ack')
  endif

  # Fire callback if registered
  if id > 0 && has_key(s_cbs, id)
    try
      s_cbs[id](ev)
    catch
    endtry
    remove(s_cbs, id)
  endif
enddef

# ═════════════════════════════════════════════════════════
# Document sync
# ═════════════════════════════════════════════════════════

def BufUri(bufnr: number = 0): string
  var nr = bufnr == 0 ? bufnr('%') : bufnr
  var name = fnamemodify(bufname(nr), ':p')
  return 'file://' .. name
enddef

def BufFt(bufnr: number = 0): string
  var nr = bufnr == 0 ? bufnr('%') : bufnr
  return getbufvar(nr, '&filetype', '')
enddef

def DocVersion(uri: string): number
  if !has_key(s_doc_versions, uri)
    s_doc_versions[uri] = 0
  endif
  s_doc_versions[uri] += 1
  return s_doc_versions[uri]
enddef

def SendDidOpen(bufnr: number)
  if !s_initialized
    return
  endif
  var uri = BufUri(bufnr)
  var ft = BufFt(bufnr)
  if ft ==# '' || uri ==# 'file://'
    return
  endif
  var text = join(getbufline(bufnr, 1, '$'), "\n") .. "\n"
  var version = DocVersion(uri)
  Send({
    type: 'textDocument/didOpen',
    id: NextId(),
    uri: uri,
    languageId: ft,
    version: version,
    text: text,
  })
enddef

def SendDidChange()
  if !s_initialized
    return
  endif
  var uri = BufUri()
  var ft = BufFt()
  if ft ==# '' || uri ==# 'file://'
    return
  endif
  var text = join(getline(1, '$'), "\n") .. "\n"
  var version = DocVersion(uri)
  Send({
    type: 'textDocument/didChange',
    id: NextId(),
    uri: uri,
    version: version,
    text: text,
  })
enddef

export def OnBufOpen()
  if !s_initialized
    return
  endif
  var ft = &filetype
  if ft ==# '' || bufname('%') ==# ''
    return
  endif
  SendDidOpen(bufnr('%'))
  RequestInlayHintsDebounced()
enddef

export def OnBufSave()
  if !s_initialized
    return
  endif
  var uri = BufUri()
  if uri ==# 'file://'
    return
  endif
  var text = join(getline(1, '$'), "\n") .. "\n"
  Send({type: 'textDocument/didSave', id: NextId(), uri: uri, text: text})
  RequestInlayHintsDebounced()
enddef

export def OnBufClose()
  if !s_initialized
    return
  endif
  var uri = BufUri()
  if uri ==# 'file://'
    return
  endif
  Send({type: 'textDocument/didClose', id: NextId(), uri: uri})
  if has_key(s_doc_versions, uri)
    remove(s_doc_versions, uri)
  endif
  if has_key(s_diagnostics, uri)
    remove(s_diagnostics, uri)
  endif
enddef

export def OnTextChanged()
  if !s_initialized
    return
  endif
  if s_change_timer > 0
    timer_stop(s_change_timer)
  endif
  s_change_timer = timer_start(200, (_) => {
    SendDidChange()
  })
enddef

# ═════════════════════════════════════════════════════════
# Completion
# ═════════════════════════════════════════════════════════

export def OnCursorMovedI()
  if !s_initialized || !g:simplecc_auto_complete
    return
  endif
  TriggerCompletion()
enddef

export def OnInsertLeave()
  s_comp_requesting = false
  CloseSignaturePopup()
enddef

export def OnCompleteChanged()
  # Could show documentation for selected item in preview
enddef

def TriggerCompletion()
  if s_comp_timer > 0
    timer_stop(s_comp_timer)
  endif
  s_comp_timer = timer_start(g:simplecc_complete_delay, (_) => {
    RequestCompletion()
  })
enddef

def RequestCompletion()
  var ft = BufFt()
  if ft ==# '' || pumvisible()
    return
  endif

  # Check context: only complete after word chars or trigger chars
  var col = col('.')
  if col <= 1
    return
  endif
  var line = getline('.')
  var before = col > 1 ? line[: col - 2] : ''
  if before ==# '' || before =~ '\s$'
    return
  endif

  var uri = BufUri()
  var lnum = line('.') - 1
  var cchar = col - 1

  var id = NextId()
  s_comp_id = id
  s_comp_requesting = true

  Send({
    type: 'textDocument/completion',
    id: id,
    uri: uri,
    languageId: ft,
    line: lnum,
    character: cchar,
  })
enddef

def OnCompletion(ev: dict<any>)
  if !s_comp_requesting
    return
  endif
  var id = get(ev, 'id', 0)
  if id != s_comp_id
    return
  endif
  s_comp_requesting = false

  var items = get(ev, 'items', [])
  if empty(items)
    return
  endif

  # Find start column for completion
  var col = col('.')
  var line = getline('.')
  var start = col - 1
  while start > 0 && line[start - 1] =~ '\w'
    start -= 1
  endwhile

  # Build Vim complete items
  var complete_items: list<dict<any>> = []
  for item in items
    var ci: dict<any> = {
      word: get(item, 'insert_text', get(item, 'label', '')),
      abbr: get(item, 'label', ''),
      menu: get(item, 'kind', ''),
      dup: 1,
    }
    var detail = get(item, 'detail', '')
    if detail !=# ''
      ci.info = detail
    endif
    var doc = get(item, 'documentation', '')
    if doc !=# ''
      ci.info = get(ci, 'info', '') !=# '' ? ci.info .. "\n\n" .. doc : doc
    endif
    add(complete_items, ci)
  endfor

  if mode() ==# 'i'
    complete(start + 1, complete_items)
  endif
enddef

# ═════════════════════════════════════════════════════════
# Hover
# ═════════════════════════════════════════════════════════

export def Hover()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif

  var id = NextId()
  Send({
    type: 'textDocument/hover',
    id: id,
    uri: BufUri(),
    languageId: BufFt(),
    line: line('.') - 1,
    character: col('.') - 1,
  })
enddef

def OnHover(ev: dict<any>)
  var contents = get(ev, 'contents', '')
  if contents ==# '' || type(contents) == v:t_none
    echo 'No hover information'
    return
  endif

  # Close previous
  if s_hover_popup > 0
    popup_close(s_hover_popup)
  endif

  var lines = split(contents, "\n")
  s_hover_popup = popup_atcursor(lines, {
    border: [1, 1, 1, 1],
    borderchars: ['─', '│', '─', '│', '╭', '╮', '╯', '╰'],
    padding: [0, 1, 0, 1],
    maxwidth: 80,
    maxheight: 20,
    close: 'click',
    moved: 'any',
  })

  # Markdown-like syntax for the popup buffer
  var winbuf = winbufnr(s_hover_popup)
  setbufvar(winbuf, '&filetype', 'markdown')
enddef

# ═════════════════════════════════════════════════════════
# Go to Definition
# ═════════════════════════════════════════════════════════

export def Definition()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif

  var id = NextId()
  Send({
    type: 'textDocument/definition',
    id: id,
    uri: BufUri(),
    languageId: BufFt(),
    line: line('.') - 1,
    character: col('.') - 1,
  })
enddef

def OnDefinition(ev: dict<any>)
  var locs = get(ev, 'locations', [])
  if empty(locs)
    echo 'No definition found'
    return
  endif

  if len(locs) == 1
    JumpToLocation(locs[0])
  else
    LocationsToQuickfix(locs, 'Definition')
  endif
enddef

# ═════════════════════════════════════════════════════════
# References
# ═════════════════════════════════════════════════════════

export def References()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif

  var id = NextId()
  Send({
    type: 'textDocument/references',
    id: id,
    uri: BufUri(),
    languageId: BufFt(),
    line: line('.') - 1,
    character: col('.') - 1,
  })
enddef

def OnReferences(ev: dict<any>)
  var locs = get(ev, 'locations', [])
  if empty(locs)
    echo 'No references found'
    return
  endif

  LocationsToQuickfix(locs, 'References')
enddef

# ═════════════════════════════════════════════════════════
# Code Action
# ═════════════════════════════════════════════════════════

export def CodeAction()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif

  var id = NextId()
  var lnum = line('.') - 1
  var cchar = col('.') - 1
  Send({
    type: 'textDocument/codeAction',
    id: id,
    uri: BufUri(),
    languageId: BufFt(),
    line: lnum,
    character: cchar,
    end_line: lnum,
    end_character: cchar,
    diagnostics: [],
  })
enddef

var s_pending_actions: list<dict<any>> = []
var s_action_ft: string = ''

def OnCodeAction(ev: dict<any>)
  var actions = get(ev, 'actions', [])
  if empty(actions)
    echo 'No code actions available'
    return
  endif

  s_pending_actions = actions
  s_action_ft = BufFt()

  # Show in popup menu
  var items: list<string> = []
  for a in actions
    var kind = get(a, 'kind', '')
    var title = get(a, 'title', '')
    if kind !=# ''
      add(items, '[' .. kind .. '] ' .. title)
    else
      add(items, title)
    endif
  endfor

  popup_menu(items, {
    title: ' Code Actions ',
    border: [1, 1, 1, 1],
    borderchars: ['─', '│', '─', '│', '╭', '╮', '╯', '╰'],
    padding: [0, 1, 0, 1],
    callback: OnActionSelected,
  })
enddef

def OnActionSelected(id: number, result: number)
  if result <= 0
    return
  endif
  var idx = result - 1
  if idx >= len(s_pending_actions)
    return
  endif

  var action = s_pending_actions[idx]
  Send({
    type: 'textDocument/executeAction',
    id: NextId(),
    languageId: s_action_ft,
    index: get(action, 'index', 0),
  })
enddef

# ═════════════════════════════════════════════════════════
# Formatting
# ═════════════════════════════════════════════════════════

export def Format()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif

  var id = NextId()
  Send({
    type: 'textDocument/formatting',
    id: id,
    uri: BufUri(),
    languageId: BufFt(),
    tab_size: &tabstop,
    insert_spaces: &expandtab,
  })
enddef

def OnFormatting(ev: dict<any>)
  var edits = get(ev, 'edits', [])
  if empty(edits)
    echo 'No formatting changes'
    return
  endif
  ApplyTextEdits(bufnr('%'), edits)
  echo printf('Applied %d edits', len(edits))
enddef

# ═════════════════════════════════════════════════════════
# Rename
# ═════════════════════════════════════════════════════════

export def Rename()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif

  var word = expand('<cword>')
  var new_name = input('Rename to: ', word)
  if new_name ==# '' || new_name ==# word
    return
  endif

  var id = NextId()
  Send({
    type: 'textDocument/rename',
    id: id,
    uri: BufUri(),
    languageId: BufFt(),
    line: line('.') - 1,
    character: col('.') - 1,
    newName: new_name,
  })
enddef

# ═════════════════════════════════════════════════════════
# Signature Help
# ═════════════════════════════════════════════════════════

export def SignatureHelp()
  if !s_initialized
    return
  endif

  var id = NextId()
  Send({
    type: 'textDocument/signatureHelp',
    id: id,
    uri: BufUri(),
    languageId: BufFt(),
    line: line('.') - 1,
    character: col('.') - 1,
  })
enddef

def OnSignatureHelp(ev: dict<any>)
  var sigs = get(ev, 'signatures', [])
  if type(sigs) != v:t_list || empty(sigs)
    CloseSignaturePopup()
    return
  endif

  var sig = sigs[0]
  var label = get(sig, 'label', '')
  var lines = [label]

  var params = get(sig, 'parameters', [])
  var active = get(sig, 'active_parameter', -1)
  if active >= 0 && active < len(params)
    var plabel = get(params[active], 'label', '')
    var pdoc = get(params[active], 'documentation', '')
    if pdoc !=# ''
      add(lines, '')
      add(lines, plabel .. ': ' .. pdoc)
    endif
  endif

  var doc = get(sig, 'documentation', '')
  if doc !=# ''
    add(lines, '')
    extend(lines, split(doc, "\n"))
  endif

  CloseSignaturePopup()
  s_sig_popup = popup_atcursor(lines, {
    border: [1, 1, 1, 1],
    borderchars: ['─', '│', '─', '│', '╭', '╮', '╯', '╰'],
    padding: [0, 1, 0, 1],
    maxwidth: 80,
    maxheight: 10,
    pos: 'botleft',
    line: 'cursor-1',
    moved: [0, 0, 0],
    close: 'none',
  })
enddef

def CloseSignaturePopup()
  if s_sig_popup > 0
    popup_close(s_sig_popup)
    s_sig_popup = 0
  endif
enddef

# ═════════════════════════════════════════════════════════
# Diagnostics
# ═════════════════════════════════════════════════════════

def OnDiagnostics(ev: dict<any>)
  var uri = get(ev, 'uri', '')
  var items = get(ev, 'items', [])
  s_diagnostics[uri] = items
  DisplayDiagnostics(uri)
enddef

def DisplayDiagnostics(uri: string)
  # Find buffer
  var fpath = substitute(uri, '^file://', '', '')
  var bufnr = bufnr(fpath)
  if bufnr < 0
    return
  endif

  # Clear old signs
  sign_unplace('simplecc', {buffer: bufnr})

  var items = get(s_diagnostics, uri, [])
  var sign_id = 1

  for item in items
    var sev = get(item, 'severity', 3)
    var sname = 'SimpleCCInfo'
    if sev == 1
      sname = 'SimpleCCError'
    elseif sev == 2
      sname = 'SimpleCCWarn'
    elseif sev == 4
      sname = 'SimpleCCHint'
    endif

    var lnum = get(item, 'line', 0) + 1
    if lnum > 0
      try
        sign_place(sign_id, 'simplecc', sname, bufnr, {lnum: lnum})
      catch
      endtry
      sign_id += 1
    endif
  endfor

  # Update text properties for underlines
  UpdateDiagHighlights(bufnr, items)

  # Virtual text diagnostics
  DisplayVirtualDiag(bufnr, items)
enddef

def UpdateDiagHighlights(bufnr: number, items: list<dict<any>>)
  # Remove old highlights
  var hl_types = ['SimpleCCErrorHL', 'SimpleCCWarnHL', 'SimpleCCInfoHL', 'SimpleCCHintHL']
  for ht in hl_types
    try
      prop_type_add(ht, {bufnr: bufnr, highlight: ht, priority: 100})
    catch
      # Already exists
    endtry
    try
      prop_remove({type: ht, bufnr: bufnr, all: true})
    catch
    endtry
  endfor

  for item in items
    var sev = get(item, 'severity', 3)
    var ht = 'SimpleCCInfoHL'
    if sev == 1
      ht = 'SimpleCCErrorHL'
    elseif sev == 2
      ht = 'SimpleCCWarnHL'
    elseif sev == 4
      ht = 'SimpleCCHintHL'
    endif

    var sl = get(item, 'line', 0) + 1
    var sc = get(item, 'character', 0) + 1
    var el = get(item, 'end_line', get(item, 'line', 0)) + 1
    var ec = get(item, 'end_character', 0) + 1
    if sl > 0 && sc > 0 && el > 0 && ec > 0
      try
        if sl == el
          prop_add(sl, sc, {type: ht, end_col: ec, bufnr: bufnr})
        else
          prop_add(sl, sc, {type: ht, end_lnum: el, end_col: ec, bufnr: bufnr})
        endif
      catch
      endtry
    endif
  endfor
enddef

export def DiagList()
  var uri = BufUri()
  var items = get(s_diagnostics, uri, [])
  if empty(items)
    echo 'No diagnostics'
    return
  endif

  var qf_items: list<dict<any>> = []
  var fpath = substitute(uri, '^file://', '', '')
  for item in items
    var sev_text = 'I'
    var sev = get(item, 'severity', 3)
    if sev == 1
      sev_text = 'E'
    elseif sev == 2
      sev_text = 'W'
    elseif sev == 4
      sev_text = 'H'
    endif
    add(qf_items, {
      filename: fpath,
      lnum: get(item, 'line', 0) + 1,
      col: get(item, 'character', 0) + 1,
      text: get(item, 'message', ''),
      type: sev_text,
    })
  endfor

  setloclist(0, qf_items)
  lopen
enddef

export def DiagNext()
  var uri = BufUri()
  var items = get(s_diagnostics, uri, [])
  if empty(items)
    echo 'No diagnostics'
    return
  endif

  var cur_line = line('.') - 1
  for item in sort(copy(items), (a, b) => a.line - b.line)
    if item.line > cur_line
      cursor(item.line + 1, item.character + 1)
      echo DiagMessage(item)
      return
    endif
  endfor
  # Wrap around
  var first = items[0]
  cursor(first.line + 1, first.character + 1)
  echo DiagMessage(first)
enddef

export def DiagPrev()
  var uri = BufUri()
  var items = get(s_diagnostics, uri, [])
  if empty(items)
    echo 'No diagnostics'
    return
  endif

  var cur_line = line('.') - 1
  var sorted = sort(copy(items), (a, b) => b.line - a.line)
  for item in sorted
    if item.line < cur_line
      cursor(item.line + 1, item.character + 1)
      echo DiagMessage(item)
      return
    endif
  endfor
  # Wrap around
  var last = sorted[0]
  cursor(last.line + 1, last.character + 1)
  echo DiagMessage(last)
enddef

def DiagMessage(item: dict<any>): string
  var sev = get(item, 'severity', 3)
  var prefix = sev == 1 ? 'Error' : sev == 2 ? 'Warning' : sev == 4 ? 'Hint' : 'Info'
  var src = get(item, 'source', '')
  var code = get(item, 'code', '')
  var tag = src !=# '' ? src : ''
  if code !=# ''
    tag ..= tag !=# '' ? '(' .. code .. ')' : code
  endif
  return printf('[%s%s] %s', prefix, tag !=# '' ? ' ' .. tag : '', item.message)
enddef

# ═════════════════════════════════════════════════════════
# Server status
# ═════════════════════════════════════════════════════════

def OnServerStatus(ev: dict<any>)
  var server = get(ev, 'server', '')
  var status = get(ev, 'status', '')
  var msg = get(ev, 'message', '')

  Log(printf('server %s: %s%s', server, status, msg !=# '' ? ' - ' .. msg : ''))

  if status ==# 'running'
    g:simplecc_status = server
  elseif status ==# 'notFound' && get(ev, 'installable', false)
    if g:simplecc_auto_install
      DoInstall(server)
    else
      echohl WarningMsg
      echom printf('[SimpleCC] %s not found. Run :SimpleCCInstall %s to install.', server, server)
      echohl None
    endif
  elseif status ==# 'error'
    echohl ErrorMsg
    echom printf('[SimpleCC] %s: %s', server, msg)
    echohl None
  endif
enddef

# ═════════════════════════════════════════════════════════
# Apply edits
# ═════════════════════════════════════════════════════════

def OnApplyEdit(ev: dict<any>)
  var edit = get(ev, 'edit', {})
  if type(edit) != v:t_dict
    return
  endif

  var changes = get(edit, 'changes', [])
  if empty(changes)
    return
  endif

  var total_edits = 0
  for file_edit in changes
    var uri = get(file_edit, 'uri', '')
    var edits = get(file_edit, 'edits', [])
    var fpath = substitute(uri, '^file://', '', '')
    var bnr = bufnr(fpath)

    if bnr < 0
      # Open the file
      execute 'edit ' .. fnameescape(fpath)
      bnr = bufnr(fpath)
    endif

    if bnr >= 0
      ApplyTextEdits(bnr, edits)
      total_edits += len(edits)
    endif
  endfor

  echo printf('Applied %d edits across %d files', total_edits, len(changes))
enddef

def ApplyTextEdits(bufnr: number, edits: list<dict<any>>)
  # Sort edits in reverse order to avoid offset issues
  var sorted = sort(copy(edits), (a, b) => {
    if a.line != b.line
      return b.line - a.line
    endif
    return b.character - a.character
  })

  for edit in sorted
    var sl = get(edit, 'line', 0) + 1
    var sc = get(edit, 'character', 0) + 1
    var el = get(edit, 'end_line', get(edit, 'line', 0)) + 1
    var ec = get(edit, 'end_character', 0) + 1
    var new_text = get(edit, 'new_text', '')
    var new_lines = split(new_text, "\n", true)

    if empty(new_lines)
      new_lines = ['']
    endif

    # Get existing text
    var lines = getbufline(bufnr, sl, el)
    if empty(lines)
      continue
    endif

    # Build the replacement
    var prefix = sl > 0 && !empty(lines) ? lines[0][: sc - 2] : ''
    var suffix = !empty(lines) ? lines[-1][ec - 1 :] : ''

    new_lines[0] = prefix .. new_lines[0]
    new_lines[-1] = new_lines[-1] .. suffix

    # Replace lines in buffer
    if el >= sl
      deletebufline(bufnr, sl, el)
    endif
    if empty(new_lines) || (len(new_lines) == 1 && new_lines[0] ==# '')
      # Delete only
    else
      appendbufline(bufnr, sl - 1, new_lines)
    endif
  endfor
enddef

# ═════════════════════════════════════════════════════════
# Location helpers
# ═════════════════════════════════════════════════════════

def JumpToLocation(loc: dict<any>)
  var uri = get(loc, 'uri', '')
  var lnum = get(loc, 'line', 0) + 1
  var cchar = get(loc, 'character', 0) + 1
  var fpath = substitute(uri, '^file://', '', '')

  # Push to tagstack
  var cur_item = {'bufnr': bufnr('%'), 'from': getpos('.'), 'tagname': expand('<cword>')}
  try
    settagstack(winnr(), {'items': [cur_item]}, 'a')
  catch
  endtry

  if fpath !=# expand('%:p')
    execute 'edit ' .. fnameescape(fpath)
  endif
  cursor(lnum, cchar)
  normal! zz
enddef

def LocationsToQuickfix(locs: list<dict<any>>, title: string)
  var qf_items: list<dict<any>> = []
  for loc in locs
    var uri = get(loc, 'uri', '')
    var fpath = substitute(uri, '^file://', '', '')
    add(qf_items, {
      filename: fpath,
      lnum: get(loc, 'line', 0) + 1,
      col: get(loc, 'character', 0) + 1,
      text: title,
    })
  endfor
  setqflist(qf_items)
  copen
enddef

# ═════════════════════════════════════════════════════════
# Public API
# ═════════════════════════════════════════════════════════

export def Start()
  if !EnsureBackend()
    return
  endif
  # Detect project root
  s_root = FindProjectRoot()
  Log('project root: ' .. s_root)

  var id = NextId()
  var config_path = get(g:, 'simplecc_config_path', '')
  Send({
    type: 'initialize',
    id: id,
    root: s_root,
    config_path: config_path,
  })
enddef

export def Stop()
  if !IsRunning()
    return
  endif
  Send({type: 'shutdown', id: NextId()})
  timer_start(500, (_) => {
    if s_job != null_job
      job_stop(s_job)
    endif
    s_running = false
    s_initialized = false
    s_job = null_job
    g:simplecc_status = ''
  })
enddef

export def Restart()
  if IsRunning()
    Stop()
    timer_start(1000, (_) => {
      Start()
    })
  else
    Start()
  endif
enddef

export def Status()
  if !IsRunning()
    echo '[SimpleCC] not running'
    return
  endif
  if !s_initialized
    echo '[SimpleCC] starting...'
    return
  endif
  echo printf('[SimpleCC] running | root: %s | server: %s', s_root, g:simplecc_status)
enddef

export def OpenConfig()
  var config_path = get(g:, 'simplecc_config_path', '')
  if config_path !=# '' && filereadable(config_path)
    execute 'edit ' .. fnameescape(config_path)
    return
  endif
  # Try project root
  var root = FindProjectRoot()
  var project_config = root .. '/simplecc.json'
  if filereadable(project_config)
    execute 'edit ' .. fnameescape(project_config)
    return
  endif
  # Create new
  execute 'edit ' .. fnameescape(project_config)
  if line('$') == 1 && getline(1) ==# ''
    setline(1, [
      '{',
      '  "languageServers": {',
      '    "example": {',
      '      "command": "language-server",',
      '      "args": ["--stdio"],',
      '      "filetypes": ["filetype"],',
      '      "rootPatterns": ["marker-file"]',
      '    }',
      '  }',
      '}',
    ])
    setlocal filetype=json
  endif
enddef

# ═════════════════════════════════════════════════════════
# Go to Implementation
# ═════════════════════════════════════════════════════════

export def Implementation()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif
  var id = NextId()
  Send({
    type: 'textDocument/implementation',
    id: id,
    uri: BufUri(),
    languageId: BufFt(),
    line: line('.') - 1,
    character: col('.') - 1,
  })
enddef

def OnImplementation(ev: dict<any>)
  var locs = get(ev, 'locations', [])
  if empty(locs)
    echo 'No implementation found'
    return
  endif
  if len(locs) == 1
    JumpToLocation(locs[0])
  else
    LocationsToQuickfix(locs, 'Implementation')
  endif
enddef

# ═════════════════════════════════════════════════════════
# Go to Type Definition
# ═════════════════════════════════════════════════════════

export def TypeDefinition()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif
  var id = NextId()
  Send({
    type: 'textDocument/typeDefinition',
    id: id,
    uri: BufUri(),
    languageId: BufFt(),
    line: line('.') - 1,
    character: col('.') - 1,
  })
enddef

def OnTypeDefinition(ev: dict<any>)
  var locs = get(ev, 'locations', [])
  if empty(locs)
    echo 'No type definition found'
    return
  endif
  if len(locs) == 1
    JumpToLocation(locs[0])
  else
    LocationsToQuickfix(locs, 'TypeDefinition')
  endif
enddef

# ═════════════════════════════════════════════════════════
# Document Symbol (Outline)
# ═════════════════════════════════════════════════════════

export def DocumentSymbol()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif
  Send({
    type: 'textDocument/documentSymbol',
    id: NextId(),
    uri: BufUri(),
    languageId: BufFt(),
  })
enddef

def OnDocumentSymbol(ev: dict<any>)
  var symbols = get(ev, 'symbols', [])
  if empty(symbols)
    echo 'No symbols found'
    return
  endif
  var qf_items: list<dict<any>> = []
  FlattenSymbols(symbols, qf_items, 0)
  setloclist(0, qf_items)
  lopen
enddef

def FlattenSymbols(symbols: list<any>, items: list<dict<any>>, depth: number)
  var indent = repeat('  ', depth)
  for s in symbols
    var kind = get(s, 'kind', '')
    var name = get(s, 'name', '')
    add(items, {
      filename: expand('%:p'),
      lnum: get(s, 'line', 0) + 1,
      col: get(s, 'character', 0) + 1,
      text: printf('%s[%s] %s', indent, kind, name),
    })
    var children = get(s, 'children', [])
    if !empty(children)
      FlattenSymbols(children, items, depth + 1)
    endif
  endfor
enddef

# ═════════════════════════════════════════════════════════
# Workspace Symbol
# ═════════════════════════════════════════════════════════

export def WorkspaceSymbol(query: string = '')
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif
  var q = query
  if q ==# ''
    q = input('Symbol query: ')
    if q ==# ''
      return
    endif
  endif
  Send({
    type: 'workspace/symbol',
    id: NextId(),
    languageId: BufFt(),
    query: q,
  })
enddef

def OnWorkspaceSymbol(ev: dict<any>)
  var symbols = get(ev, 'symbols', [])
  if empty(symbols)
    echo 'No symbols found'
    return
  endif
  var qf_items: list<dict<any>> = []
  for s in symbols
    var uri = get(s, 'detail', '')
    var fpath = substitute(uri, '^file://', '', '')
    add(qf_items, {
      filename: fpath,
      lnum: get(s, 'line', 0) + 1,
      col: get(s, 'character', 0) + 1,
      text: printf('[%s] %s', get(s, 'kind', ''), get(s, 'name', '')),
    })
  endfor
  setqflist(qf_items)
  copen
enddef

# ═════════════════════════════════════════════════════════
# Document Highlight
# ═════════════════════════════════════════════════════════

var s_dochighlight_ids: list<number> = []

export def DocumentHighlight()
  if !s_initialized
    return
  endif
  Send({
    type: 'textDocument/documentHighlight',
    id: NextId(),
    uri: BufUri(),
    languageId: BufFt(),
    line: line('.') - 1,
    character: col('.') - 1,
  })
enddef

export def DocumentHighlightClear()
  for mid in s_dochighlight_ids
    try
      matchdelete(mid)
    catch
    endtry
  endfor
  s_dochighlight_ids = []
enddef

def OnDocumentHighlightResult(ev: dict<any>)
  DocumentHighlightClear()
  var highlights = get(ev, 'highlights', [])
  if empty(highlights)
    return
  endif
  for h in highlights
    var kind = get(h, 'kind', 'text')
    var hl_group = 'SimpleCCDocHighlightText'
    if kind ==# 'read'
      hl_group = 'SimpleCCDocHighlightRead'
    elseif kind ==# 'write'
      hl_group = 'SimpleCCDocHighlightWrite'
    endif
    var sl = get(h, 'line', 0) + 1
    var sc = get(h, 'character', 0) + 1
    var el = get(h, 'end_line', 0) + 1
    var ec = get(h, 'end_character', 0) + 1
    try
      var mid = matchadd(hl_group, '\%' .. sl .. 'l\%' .. sc .. 'c\_.*\%' .. el .. 'l\%' .. ec .. 'c')
      add(s_dochighlight_ids, mid)
    catch
    endtry
  endfor
enddef

# ═════════════════════════════════════════════════════════
# Inlay Hints
# ═════════════════════════════════════════════════════════

var s_inlay_enabled: bool = true
var s_inlay_timer: number = 0

export def InlayHintsToggle()
  s_inlay_enabled = !s_inlay_enabled
  if s_inlay_enabled
    echo '[SimpleCC] Inlay hints ON'
    RequestInlayHints()
  else
    echo '[SimpleCC] Inlay hints OFF'
    ClearInlayHints()
  endif
enddef

def RequestInlayHintsDebounced()
  if s_inlay_timer > 0
    timer_stop(s_inlay_timer)
  endif
  s_inlay_timer = timer_start(500, (_) => {
    RequestInlayHints()
  })
enddef

def RequestInlayHints()
  if !s_initialized || !s_inlay_enabled || !g:simplecc_inlay_hints
    return
  endif
  var ft = BufFt()
  if ft ==# ''
    return
  endif
  Send({
    type: 'textDocument/inlayHint',
    id: NextId(),
    uri: BufUri(),
    languageId: ft,
    startLine: 0,
    endLine: line('$'),
  })
enddef

def ClearInlayHints()
  try
    prop_type_add('SimpleCCInlay', {bufnr: bufnr('%'), highlight: 'SimpleCCInlayHint'})
  catch
  endtry
  try
    prop_remove({type: 'SimpleCCInlay', bufnr: bufnr('%'), all: true})
  catch
  endtry
enddef

def OnInlayHints(ev: dict<any>)
  if !s_inlay_enabled
    return
  endif
  var hints = get(ev, 'hints', [])
  # Don't clear old hints if response is empty (server still loading)
  if empty(hints)
    return
  endif
  ClearInlayHints()
  var bnr = bufnr('%')
  try
    prop_type_add('SimpleCCInlay', {bufnr: bnr, highlight: 'SimpleCCInlayHint'})
  catch
  endtry
  for h in hints
    var lnum = get(h, 'line', 0) + 1
    var col = get(h, 'character', 0) + 1
    var label = get(h, 'label', '')
    var pad_l = get(h, 'padding_left', false)
    var pad_r = get(h, 'padding_right', false)
    var text = (pad_l ? ' ' : '') .. label .. (pad_r ? ' ' : '')
    if lnum > 0 && lnum <= line('$')
      try
        prop_add(lnum, col, {type: 'SimpleCCInlay', text: text, bufnr: bnr})
      catch
      endtry
    endif
  endfor
enddef

# ═════════════════════════════════════════════════════════
# Call Hierarchy
# ═════════════════════════════════════════════════════════

var s_call_hierarchy_items: list<any> = []

export def IncomingCalls()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif
  s_call_hierarchy_items = []
  Send({
    type: 'textDocument/prepareCallHierarchy',
    id: NextId(),
    uri: BufUri(),
    languageId: BufFt(),
    line: line('.') - 1,
    character: col('.') - 1,
  })
  # Store direction for when prepare result arrives
  b:simplecc_call_direction = 'incoming'
enddef

export def OutgoingCalls()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif
  s_call_hierarchy_items = []
  Send({
    type: 'textDocument/prepareCallHierarchy',
    id: NextId(),
    uri: BufUri(),
    languageId: BufFt(),
    line: line('.') - 1,
    character: col('.') - 1,
  })
  b:simplecc_call_direction = 'outgoing'
enddef

def OnCallHierarchyPrepare(ev: dict<any>)
  var items = get(ev, 'items', [])
  if empty(items)
    echo 'No call hierarchy item found'
    return
  endif
  s_call_hierarchy_items = items
  var item = items[0]
  var raw = get(item, 'raw', {})
  var direction = get(b:, 'simplecc_call_direction', 'incoming')
  var req_type = direction ==# 'incoming' ? 'callHierarchy/incomingCalls' : 'callHierarchy/outgoingCalls'
  Send({
    type: req_type,
    id: NextId(),
    languageId: BufFt(),
    item: raw,
  })
enddef

def OnIncomingCallsResult(ev: dict<any>)
  var calls = get(ev, 'calls', [])
  if empty(calls)
    echo 'No incoming calls'
    return
  endif
  var qf_items: list<dict<any>> = []
  for c in calls
    var item = get(c, 'item', {})
    var uri = get(item, 'uri', '')
    var fpath = substitute(uri, '^file://', '', '')
    add(qf_items, {
      filename: fpath,
      lnum: get(item, 'line', 0) + 1,
      col: get(item, 'character', 0) + 1,
      text: printf('[%s] %s', get(item, 'kind', ''), get(item, 'name', '')),
    })
  endfor
  setqflist(qf_items)
  copen
enddef

def OnOutgoingCallsResult(ev: dict<any>)
  var calls = get(ev, 'calls', [])
  if empty(calls)
    echo 'No outgoing calls'
    return
  endif
  var qf_items: list<dict<any>> = []
  for c in calls
    var item = get(c, 'item', {})
    var uri = get(item, 'uri', '')
    var fpath = substitute(uri, '^file://', '', '')
    add(qf_items, {
      filename: fpath,
      lnum: get(item, 'line', 0) + 1,
      col: get(item, 'character', 0) + 1,
      text: printf('[%s] %s', get(item, 'kind', ''), get(item, 'name', '')),
    })
  endfor
  setqflist(qf_items)
  copen
enddef

# ═════════════════════════════════════════════════════════
# Selection Range (Smart Expand/Shrink)
# ═════════════════════════════════════════════════════════

var s_selection_ranges: list<any> = []
var s_selection_idx: number = 0

export def SelectionExpand()
  if !s_initialized
    return
  endif
  if empty(s_selection_ranges)
    Send({
      type: 'textDocument/selectionRange',
      id: NextId(),
      uri: BufUri(),
      languageId: BufFt(),
      positions: [{line: line('.') - 1, character: col('.') - 1}],
    })
    return
  endif
  # Expand to next parent
  if s_selection_idx > 0
    s_selection_idx -= 1
  endif
  ApplySelectionRange()
enddef

export def SelectionShrink()
  if empty(s_selection_ranges)
    return
  endif
  if s_selection_idx < len(s_selection_ranges) - 1
    s_selection_idx += 1
  endif
  ApplySelectionRange()
enddef

def OnSelectionRange(ev: dict<any>)
  var ranges = get(ev, 'ranges', [])
  if empty(ranges)
    return
  endif
  # Flatten the nested selection range into a list (outermost first)
  s_selection_ranges = []
  var r = ranges[0]
  while type(r) == v:t_dict
    add(s_selection_ranges, r)
    r = get(r, 'parent', {})
  endwhile
  # Reverse so innermost is first
  reverse(s_selection_ranges)
  s_selection_idx = 0
  ApplySelectionRange()
enddef

def ApplySelectionRange()
  if empty(s_selection_ranges)
    return
  endif
  var r = s_selection_ranges[s_selection_idx]
  var sl = get(r, 'line', 0) + 1
  var sc = get(r, 'character', 0) + 1
  var el = get(r, 'end_line', 0) + 1
  var ec = get(r, 'end_character', 0)
  normal! v
  cursor(sl, sc)
  normal! o
  cursor(el, ec)
enddef

# ═════════════════════════════════════════════════════════
# Semantic Tokens
# ═════════════════════════════════════════════════════════

export def SemanticTokens()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif
  Send({
    type: 'textDocument/semanticTokens',
    id: NextId(),
    uri: BufUri(),
    languageId: BufFt(),
  })
enddef

def OnSemanticTokens(ev: dict<any>)
  var tokens = get(ev, 'tokens', [])
  if empty(tokens)
    echo 'No semantic tokens'
    return
  endif
  var bnr = bufnr('%')
  # Clear old semantic highlights
  for tt in ['Namespace', 'Type', 'Class', 'Enum', 'Interface', 'Struct',
      'TypeParameter', 'Parameter', 'Variable', 'Property', 'EnumMember',
      'Function', 'Method', 'Macro', 'Keyword', 'Comment', 'String',
      'Number', 'Operator', 'Decorator']
    var ptype = 'SimpleCCSemantic' .. tt
    try
      prop_type_add(ptype, {bufnr: bnr, highlight: 'SimpleCCSemantic' .. tt})
    catch
    endtry
    try
      prop_remove({type: ptype, bufnr: bnr, all: true})
    catch
    endtry
  endfor

  for t in tokens
    var lnum = get(t, 'line', 0) + 1
    var col = get(t, 'start', 0) + 1
    var length = get(t, 'length', 0)
    var ttype = get(t, 'token_type', '')
    # Capitalize first letter for highlight group name
    var hl_suffix = toupper(ttype[0]) .. ttype[1 :]
    var ptype = 'SimpleCCSemantic' .. hl_suffix
    if lnum > 0 && col > 0 && length > 0
      try
        prop_type_add(ptype, {bufnr: bnr, highlight: ptype})
      catch
      endtry
      try
        prop_add(lnum, col, {type: ptype, length: length, bufnr: bnr})
      catch
      endtry
    endif
  endfor
  echo printf('[SimpleCC] Applied %d semantic tokens', len(tokens))
enddef

# ═════════════════════════════════════════════════════════
# Code Lens
# ═════════════════════════════════════════════════════════

export def CodeLens()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif
  Send({
    type: 'textDocument/codeLens',
    id: NextId(),
    uri: BufUri(),
    languageId: BufFt(),
  })
enddef

def OnCodeLens(ev: dict<any>)
  var lenses = get(ev, 'lenses', [])
  if empty(lenses)
    echo 'No code lenses'
    return
  endif
  var bnr = bufnr('%')
  try
    prop_type_add('SimpleCCCodeLens', {bufnr: bnr, highlight: 'Comment'})
  catch
  endtry
  try
    prop_remove({type: 'SimpleCCCodeLens', bufnr: bnr, all: true})
  catch
  endtry
  for l in lenses
    var lnum = get(l, 'line', 0) + 1
    var title = get(l, 'command_title', '')
    if title !=# '' && lnum > 0 && lnum <= line('$')
      try
        prop_add(lnum, 0, {type: 'SimpleCCCodeLens', text: '  ' .. title, text_align: 'after', bufnr: bnr})
      catch
      endtry
    endif
  endfor
  echo printf('[SimpleCC] %d code lenses', len(lenses))
enddef

# ═════════════════════════════════════════════════════════
# Folding Range
# ═════════════════════════════════════════════════════════

export def FoldingRange()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif
  Send({
    type: 'textDocument/foldingRange',
    id: NextId(),
    uri: BufUri(),
    languageId: BufFt(),
  })
enddef

def OnFoldingRange(ev: dict<any>)
  var ranges = get(ev, 'ranges', [])
  if empty(ranges)
    echo 'No folding ranges'
    return
  endif
  setlocal foldmethod=manual
  normal! zE
  for r in ranges
    var sl = get(r, 'start_line', 0) + 1
    var el = get(r, 'end_line', 0) + 1
    if el > sl
      try
        execute printf('%d,%dfold', sl, el)
      catch
      endtry
    endif
  endfor
  echo printf('[SimpleCC] Created %d folds', len(ranges))
enddef

# ═════════════════════════════════════════════════════════
# Linked Editing Range
# ═════════════════════════════════════════════════════════

def OnLinkedEditingRange(ev: dict<any>)
  # Placeholder for linked editing support
  var result = get(ev, 'result', {})
  if type(result) != v:t_dict
    return
  endif
  var ranges = get(result, 'ranges', [])
  if empty(ranges)
    return
  endif
  echo printf('[SimpleCC] %d linked ranges', len(ranges))
enddef

# ═════════════════════════════════════════════════════════
# Progress
# ═════════════════════════════════════════════════════════

def OnProgress(ev: dict<any>)
  var kind = get(ev, 'kind', '')
  var title = get(ev, 'title', '')
  var msg = get(ev, 'message', '')
  var pct = get(ev, 'percentage', -1)
  var server = get(ev, 'server', '')

  if kind ==# 'begin'
    g:simplecc_status = printf('%s: %s', server, title)
  elseif kind ==# 'report'
    if pct >= 0
      g:simplecc_status = printf('%s: %s %d%%', server, msg !=# '' ? msg : title, pct)
    elseif msg !=# ''
      g:simplecc_status = printf('%s: %s', server, msg)
    endif
  elseif kind ==# 'end'
    g:simplecc_status = server
    if msg !=# ''
      echo printf('[SimpleCC] %s', msg)
    endif
  endif
enddef

# ═════════════════════════════════════════════════════════
# Virtual Text Diagnostics
# ═════════════════════════════════════════════════════════

def DisplayVirtualDiag(bufnr: number, items: list<dict<any>>)
  if !g:simplecc_virtual_diag
    return
  endif
  try
    prop_type_add('SimpleCCVirtualDiag', {bufnr: bufnr, highlight: 'SimpleCCVirtualDiagError'})
  catch
  endtry
  try
    prop_remove({type: 'SimpleCCVirtualDiag', bufnr: bufnr, all: true})
  catch
  endtry
  # Show one diagnostic per line (highest severity)
  var line_diags: dict<dict<any>> = {}
  for item in items
    var lnum = get(item, 'line', 0) + 1
    var key = string(lnum)
    var sev = get(item, 'severity', 3)
    if !has_key(line_diags, key) || sev < get(line_diags[key], 'severity', 99)
      line_diags[key] = item
    endif
  endfor
  for [key, item] in items(line_diags)
    var lnum = str2nr(key)
    var sev = get(item, 'severity', 3)
    var msg = get(item, 'message', '')
    # Truncate long messages
    if len(msg) > 60
      msg = msg[: 57] .. '...'
    endif
    # Replace newlines
    msg = substitute(msg, "\n", ' ', 'g')
    var hl = sev <= 1 ? 'SimpleCCVirtualDiagError' : 'SimpleCCVirtualDiagWarn'
    try
      prop_type_add('SimpleCCVirtualDiag', {bufnr: bufnr, highlight: hl})
    catch
    endtry
    if lnum > 0 && lnum <= getbufinfo(bufnr)[0].linecount
      try
        prop_add(lnum, 0, {type: 'SimpleCCVirtualDiag', text: '  ' .. msg, text_align: 'after', bufnr: bufnr})
      catch
      endtry
    endif
  endfor
enddef

# ═════════════════════════════════════════════════════════
# CursorHold handler (inlay hints refresh)
# ═════════════════════════════════════════════════════════

export def OnCursorHold()
  if !s_initialized
    return
  endif
  # Clear stale selection ranges
  s_selection_ranges = []
enddef

# ═════════════════════════════════════════════════════════
# Server install
# ═════════════════════════════════════════════════════════

export def InstallServer(name: string = '')
  if !IsRunning()
    if !EnsureBackend()
      return
    endif
  endif

  var server = name
  if server ==# ''
    var servers = ['rust-analyzer', 'clangd', 'pyright', 'lua-language-server', 'gopls']
    popup_menu(servers, {
      title: ' Install Language Server ',
      border: [1, 1, 1, 1],
      borderchars: ['─', '│', '─', '│', '╭', '╮', '╯', '╰'],
      padding: [0, 1, 0, 1],
      callback: (_, idx) => {
        if idx > 0
          DoInstall(servers[idx - 1])
        endif
      },
    })
    return
  endif

  DoInstall(server)
enddef

def DoInstall(server: string)
  echom printf('[SimpleCC] Installing %s...', server)
  Send({
    type: 'server/install',
    id: NextId(),
    server: server,
  })
enddef

export def ListServers()
  if !IsRunning()
    echom '[SimpleCC] not running'
    return
  endif
  Send({
    type: 'server/listInstallable',
    id: NextId(),
  })
enddef

def OnInstallProgress(ev: dict<any>)
  var server = get(ev, 'server', '')
  var stage = get(ev, 'stage', '')
  var pct = get(ev, 'percent', 0)
  echo printf('[SimpleCC] %s: %s %d%%', server, stage, pct)
  redrawstatus
enddef

def OnInstallResult(ev: dict<any>)
  var server = get(ev, 'server', '')
  var status = get(ev, 'status', '')
  if status ==# 'ok'
    var path = get(ev, 'path', '')
    echohl ModeMsg
    echom printf('[SimpleCC] %s installed successfully: %s', server, path)
    echohl None
    # Re-trigger buffer open to start the server
    var ft = &filetype
    if ft !=# '' && bufname('%') !=# ''
      SendDidOpen(bufnr('%'))
    endif
  else
    echohl ErrorMsg
    echom printf('[SimpleCC] %s install failed: %s', server, get(ev, 'message', ''))
    echohl None
  endif
enddef

def OnInstallableServers(ev: dict<any>)
  var servers = get(ev, 'servers', [])
  if empty(servers)
    echo '[SimpleCC] No installable servers'
    return
  endif
  var lines = ['Language Servers:', '']
  for s in servers
    var name = get(s, 'name', '')
    var installed = get(s, 'installed', false)
    var path = get(s, 'path', '')
    if installed
      add(lines, printf('  ✓ %s  (%s)', name, path))
    else
      add(lines, printf('  ✗ %s  (not installed)', name))
    endif
  endfor

  new
  setlocal buftype=nofile bufhidden=wipe noswapfile
  setline(1, lines)
  setlocal nomodifiable
enddef

# ═════════════════════════════════════════════════════════
# Helpers
# ═════════════════════════════════════════════════════════

def FindProjectRoot(): string
  var markers = ['.git', 'Cargo.toml', 'package.json', 'go.mod', 'pyproject.toml',
                 'Makefile', 'CMakeLists.txt', '.hg', '.svn']
  var dir = expand('%:p:h')
  if dir ==# ''
    dir = getcwd()
  endif

  var prev = ''
  while dir !=# prev
    for m in markers
      if isdirectory(dir .. '/' .. m) || filereadable(dir .. '/' .. m)
        return dir
      endif
    endfor
    prev = dir
    dir = fnamemodify(dir, ':h')
  endwhile

  return getcwd()
enddef
