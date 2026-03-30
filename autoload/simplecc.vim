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
# Kill timer for daemon force-kill
var s_kill_timer: number = 0
# Diagnostics float popup
var s_diag_popup: number = 0
# Progress tracking
var s_progress_tokens: dict<dict<any>> = {}
var s_spinner_timer: number = 0
var s_spinner_idx: number = 0
var s_spinner_frames: list<string> = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏']
# Semantic tokens auto state
var s_semtok_timer: number = 0
var s_semtok_has_full: dict<bool> = {}
var s_semtok_range_mode: bool = false
var s_semtok_modifier_cache: dict<bool> = {}
# Workspace symbol live search state
var s_ws_input: string = ''
var s_ws_popup: number = 0
var s_ws_results_popup: number = 0
var s_ws_timer: number = 0
var s_ws_results: list<dict<any>> = []
var s_ws_live: bool = false
# Code lens cache for execution
var s_code_lens_cache: list<dict<any>> = []
# Snippet state
var s_snippet_active: bool = false
var s_snippet_tabstops: list<dict<any>> = []
var s_snippet_idx: number = -1
# Incremental sync state
var s_pending_changes: dict<list<dict<any>>> = {}
var s_listener_ids: dict<number> = {}
# Inlay hints version tracking
var s_inlay_request_version: dict<number> = {}

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
        if s_kill_timer > 0
          timer_stop(s_kill_timer)
          s_kill_timer = 0
        endif
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

  elseif ev.type ==# 'completionResolve'
    OnCompletionResolve(ev)

  elseif ev.type ==# 'codeLensExecute'
    OnCodeLensExecute(ev)

  elseif ev.type ==# 'typeHierarchyPrepare'
    OnTypeHierarchyPrepare(ev)

  elseif ev.type ==# 'supertypes'
    OnSupertypesResult(ev)

  elseif ev.type ==# 'subtypes'
    OnSubtypesResult(ev)

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
  # Register listener for incremental sync
  RegisterListener(bufnr)
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
  listener_flush(bufnr('%'))
  var version = DocVersion(uri)
  # Use incremental changes if available
  if has_key(s_pending_changes, uri) && !empty(s_pending_changes[uri])
    var changes = s_pending_changes[uri]
    s_pending_changes[uri] = []
    Send({
      type: 'textDocument/didChange',
      id: NextId(),
      uri: uri,
      version: version,
      changes: changes,
    })
  else
    var text = join(getline(1, '$'), "\n") .. "\n"
    Send({
      type: 'textDocument/didChange',
      id: NextId(),
      uri: uri,
      version: version,
      text: text,
    })
  endif
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
  RequestSemanticTokensDebounced()
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
  RequestSemanticTokensDebounced()
  # F12: Auto pull diagnostics if enabled
  if g:simplecc_pull_diagnostics
    PullDiagnostics()
  endif
enddef

export def OnBufClose()
  if !s_initialized
    return
  endif
  var bnr = bufnr('%')
  var uri = BufUri(bnr)
  if uri ==# 'file://'
    return
  endif
  UnregisterListener(bnr)
  Send({type: 'textDocument/didClose', id: NextId(), uri: uri})
  if has_key(s_doc_versions, uri)
    remove(s_doc_versions, uri)
  endif
  if has_key(s_diagnostics, uri)
    remove(s_diagnostics, uri)
  endif
  if has_key(s_pending_changes, uri)
    remove(s_pending_changes, uri)
  endif
  if has_key(s_semtok_has_full, uri)
    remove(s_semtok_has_full, uri)
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
    # F3: Re-request inlay hints after changes
    RequestInlayHintsDebounced()
    # F2: Auto semantic tokens
    RequestSemanticTokensDebounced()
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
  if !s_initialized
    return
  endif
  var info = complete_info(['selected', 'items'])
  var sel = get(info, 'selected', -1)
  if sel < 0
    return
  endif
  var items = get(info, 'items', [])
  if sel >= len(items)
    return
  endif
  var ci = items[sel]
  var ud = get(ci, 'user_data', {})
  if type(ud) != v:t_dict
    return
  endif
  var idx = get(ud, 'index', -1)
  if idx < 0
    return
  endif
  Send({
    type: 'completionItem/resolve',
    id: NextId(),
    languageId: BufFt(),
    index: idx,
  })
enddef

def OnCompletionResolve(ev: dict<any>)
  var item = get(ev, 'item', {})
  if empty(item)
    return
  endif
  var detail = get(item, 'detail', '')
  var doc = get(item, 'documentation', '')
  var text = detail
  if doc !=# ''
    text = text !=# '' ? text .. "\n\n" .. doc : doc
  endif
  if text ==# '' || !pumvisible()
    return
  endif
  # Show resolved info in a popup near the completion menu
  if s_hover_popup > 0
    popup_close(s_hover_popup)
  endif
  var lines = split(text, "\n")
  s_hover_popup = popup_create(lines, {
    border: [1, 1, 1, 1],
    borderchars: ['─', '│', '─', '│', '╭', '╮', '╯', '╰'],
    padding: [0, 1, 0, 1],
    maxwidth: 60,
    maxheight: 15,
    pos: 'topleft',
    line: 'cursor-1',
    col: 'cursor+40',
    moved: 'any',
    highlight: 'Normal',
    borderhighlight: ['SimpleCCFloatBorder'],
  })
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
  var idx = 0
  for item in items
    var word = get(item, 'insert_text', get(item, 'label', ''))
    var is_snippet = get(item, 'is_snippet', false)
    # For snippet items, show the label as word rather than raw snippet text
    if is_snippet
      word = get(item, 'label', word)
    endif
    var ci: dict<any> = {
      word: word,
      abbr: get(item, 'label', ''),
      menu: get(item, 'kind', '') .. (is_snippet ? ' ~' : ''),
      dup: 1,
      user_data: {index: idx, is_snippet: is_snippet, snippet_text: is_snippet ? get(item, 'insert_text', '') : ''},
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
    idx += 1
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
  # Filter by minimum severity level
  var min_sev = get(g:, 'simplecc_diag_min_severity', 4)
  items = filter(copy(items), (_, v) => get(v, 'severity', 3) <= min_sev)
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
      # Force kill if still running after 3 seconds
      s_kill_timer = timer_start(3000, (_) => {
        s_kill_timer = 0
        if s_job != null_job && job_status(s_job) ==# 'run'
          job_stop(s_job, 'kill')
          Log('daemon force-killed')
        endif
      })
    endif
    s_running = false
    s_initialized = false
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
  # Live search mode: update popup instead of quickfix
  if s_ws_live && s_ws_popup > 0
    s_ws_results = symbols
    UpdateWsResults()
    return
  endif
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
var s_inlay_cache: list<any> = []
var s_inlay_cache_bufnr: number = -1

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
  if empty(hints)
    return
  endif
  # Cache for later restoration
  s_inlay_cache = hints
  s_inlay_cache_bufnr = bufnr('%')
  ApplyInlayHints(hints, bufnr('%'))
enddef

def ApplyInlayHints(hints: list<any>, bnr: number)
  ClearInlayHints()
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

def RestoreInlayHints()
  if empty(s_inlay_cache) || !s_inlay_enabled || !g:simplecc_inlay_hints
    return
  endif
  var bnr = bufnr('%')
  if bnr != s_inlay_cache_bufnr
    return
  endif
  # Check if hints are still displayed by looking for a prop on a cached hint line
  var check_line = get(s_inlay_cache[0], 'line', 0) + 1
  var existing: list<any> = []
  try
    existing = check_line > 0 ? prop_list(check_line, {bufnr: bnr, type: 'SimpleCCInlay'}) : []
  catch
    return
  endtry
  if empty(existing)
    # Hints were lost, restore from cache
    ApplyInlayHints(s_inlay_cache, bnr)
  endif
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
  var uri = BufUri()
  if line('$') > g:simplecc_semtok_range_threshold
    # Large file: use range request for visible area + buffer
    var top = max([0, line('w0') - 1 - 100])
    var bot = min([line('$') - 1, line('w$') - 1 + 100])
    s_semtok_range_mode = true
    Send({
      type: 'textDocument/semanticTokens/range',
      id: NextId(),
      uri: uri,
      languageId: BufFt(),
      startLine: top,
      startCharacter: 0,
      endLine: bot,
      endCharacter: 0,
    })
  elseif has_key(s_semtok_has_full, uri) && s_semtok_has_full[uri]
    # Subsequent request: use delta
    s_semtok_range_mode = false
    Send({
      type: 'textDocument/semanticTokens/delta',
      id: NextId(),
      uri: uri,
      languageId: BufFt(),
    })
  else
    # First request: full
    s_semtok_range_mode = false
    Send({
      type: 'textDocument/semanticTokens',
      id: NextId(),
      uri: uri,
      languageId: BufFt(),
    })
  endif
enddef

def EnsureModifierHighlight(base_hl: string, mod_suffix: string): string
  var combined = base_hl .. mod_suffix
  if has_key(s_semtok_modifier_cache, combined)
    return combined
  endif
  # Get base highlight attributes
  var base_info = hlget(base_hl, true)
  var mod_hl = 'SimpleCCSemantic' .. mod_suffix
  var mod_info = hlget(mod_hl, true)
  if !empty(base_info) && !empty(mod_info)
    var base = base_info[0]
    var mattr = mod_info[0]
    # Merge: use base colors + modifier gui/cterm attributes
    var gui_attr = get(mattr, 'gui', {})
    var cterm_attr = get(mattr, 'cterm', {})
    var def: dict<any> = {}
    # Inherit link target's colors via base highlight
    var base_gui = get(base, 'gui', {})
    var base_cterm = get(base, 'cterm', {})
    if has_key(base, 'guifg')
      def.guifg = base.guifg
    endif
    if has_key(base, 'ctermfg')
      def.ctermfg = base.ctermfg
    endif
    # Apply modifier style attributes
    def.gui = gui_attr
    def.cterm = cterm_attr
    hlset([extend({name: combined}, def)])
  else
    # Fallback: just link to the modifier-only group
    execute 'highlight default link ' .. combined .. ' ' .. mod_hl
  endif
  s_semtok_modifier_cache[combined] = true
  return combined
enddef

def ResolveSemanticHighlight(ttype: string, mods: list<any>): list<any>
  # Capitalize first letter for highlight group name
  if empty(ttype)
    return ['SimpleCCSemanticVariable', 'SimpleCCSemanticVariable']
  endif
  var hl_suffix = toupper(ttype[0]) .. ttype[1 :]
  var base_hl = 'SimpleCCSemantic' .. hl_suffix
  var ptype = base_hl
  var hl_group = base_hl

  if !empty(mods)
    # Check modifiers in priority order
    var mod_priority = ['deprecated', 'readonly', 'static', 'defaultLibrary', 'declaration']
    for mod in mod_priority
      if index(mods, mod) >= 0
        var mod_suffix = toupper(mod[0]) .. mod[1 :]
        hl_group = EnsureModifierHighlight(base_hl, mod_suffix)
        ptype = base_hl .. mod_suffix
        break
      endif
    endfor
  endif

  return [ptype, hl_group]
enddef

def OnSemanticTokens(ev: dict<any>)
  var tokens = get(ev, 'tokens', [])
  if empty(tokens)
    echo 'No semantic tokens'
    return
  endif
  var bnr = bufnr('%')
  var uri = BufUri()
  var prio = g:simplecc_semtok_priority

  if s_semtok_range_mode
    # Range mode: only clear props in visible region
    var top = max([1, line('w0') - 100])
    var bot = min([line('$'), line('w$') + 100])
    for tt in ['Namespace', 'Type', 'Class', 'Enum', 'Interface', 'Struct',
        'TypeParameter', 'Parameter', 'Variable', 'Property', 'EnumMember',
        'Function', 'Method', 'Macro', 'Keyword', 'Comment', 'String',
        'Number', 'Operator', 'Decorator']
      var ptype = 'SimpleCCSemantic' .. tt
      try
        prop_type_add(ptype, {bufnr: bnr, highlight: ptype, priority: prio})
      catch
      endtry
      for lnum in range(top, bot)
        try
          prop_remove({type: ptype, bufnr: bnr, lnum: lnum})
        catch
        endtry
      endfor
    endfor
  else
    # Full/delta mode: clear all props
    for tt in ['Namespace', 'Type', 'Class', 'Enum', 'Interface', 'Struct',
        'TypeParameter', 'Parameter', 'Variable', 'Property', 'EnumMember',
        'Function', 'Method', 'Macro', 'Keyword', 'Comment', 'String',
        'Number', 'Operator', 'Decorator']
      var ptype = 'SimpleCCSemantic' .. tt
      try
        prop_type_add(ptype, {bufnr: bnr, highlight: ptype, priority: prio})
      catch
      endtry
      try
        prop_remove({type: ptype, bufnr: bnr, all: true})
      catch
      endtry
    endfor
    # Mark that full tokens have been received for delta support
    s_semtok_has_full[uri] = true
  endif

  for t in tokens
    var lnum = get(t, 'line', 0) + 1
    var col = get(t, 'start', 0) + 1
    var length = get(t, 'length', 0)
    var ttype = get(t, 'token_type', '')
    var mods: list<any> = get(t, 'modifiers', [])
    var resolved = ResolveSemanticHighlight(ttype, mods)
    var ptype: string = resolved[0]
    var hl_group: string = resolved[1]
    if lnum > 0 && col > 0 && length > 0
      try
        prop_type_add(ptype, {bufnr: bnr, highlight: hl_group, priority: prio})
      catch
      endtry
      try
        prop_add(lnum, col, {type: ptype, length: length, bufnr: bnr})
      catch
      endtry
    endif
  endfor
  Log(printf('applied %d semantic tokens', len(tokens)))
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
  # Cache for execution
  s_code_lens_cache = lenses
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

def UpdateProgressStatus()
  if empty(s_progress_tokens)
    return
  endif
  var frame = s_spinner_frames[s_spinner_idx % len(s_spinner_frames)]
  s_spinner_idx += 1
  var parts: list<string> = []
  for [key, info] in items(s_progress_tokens)
    var text = get(info, 'title', '')
    var msg = get(info, 'message', '')
    var pct_raw = get(info, 'percentage', -1)
    var pct = type(pct_raw) == v:t_number ? pct_raw : -1
    if msg !=# ''
      text = msg
    endif
    if pct >= 0
      text ..= ' ' .. pct .. '%'
    endif
    add(parts, get(info, 'server', '') .. ': ' .. text)
  endfor
  g:simplecc_status = frame .. ' ' .. join(parts, ' | ')
  redrawstatus
enddef

def OnProgress(ev: dict<any>)
  var kind = get(ev, 'kind', '')
  var title = get(ev, 'title', '')
  var msg = get(ev, 'message', '')
  var pct_raw = get(ev, 'percentage', -1)
  var pct = type(pct_raw) == v:t_number ? pct_raw : -1
  var server = get(ev, 'server', '')
  var token = get(ev, 'token', '')

  if kind ==# 'begin'
    s_progress_tokens[token] = {server: server, title: title, message: msg, percentage: pct}
    if s_spinner_timer == 0
      s_spinner_idx = 0
      s_spinner_timer = timer_start(100, (_) => {
        UpdateProgressStatus()
      }, {repeat: -1})
    endif
  elseif kind ==# 'report'
    if has_key(s_progress_tokens, token)
      if msg !=# ''
        s_progress_tokens[token].message = msg
      endif
      if pct >= 0
        s_progress_tokens[token].percentage = pct
      endif
    endif
  elseif kind ==# 'end'
    if has_key(s_progress_tokens, token)
      remove(s_progress_tokens, token)
    endif
    if empty(s_progress_tokens)
      if s_spinner_timer > 0
        timer_stop(s_spinner_timer)
        s_spinner_timer = 0
      endif
      g:simplecc_status = server
      redrawstatus
    endif
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
  # Group diagnostics by line, sorted by severity
  var line_diags: dict<list<dict<any>>> = {}
  for item in items
    var lnum = get(item, 'line', 0) + 1
    var key = string(lnum)
    if !has_key(line_diags, key)
      line_diags[key] = []
    endif
    add(line_diags[key], item)
  endfor
  var max_per_line = get(g:, 'simplecc_diag_max_per_line', 3)
  for [key, diags] in items(line_diags)
    var lnum = str2nr(key)
    # Sort by severity (error first)
    sort(diags, (a, b) => get(a, 'severity', 3) - get(b, 'severity', 3))
    var shown = diags[: max_per_line - 1]
    var msgs: list<string> = []
    for d in shown
      var msg = substitute(get(d, 'message', ''), "\n", ' ', 'g')
      if len(msg) > 60
        msg = msg[: 57] .. '...'
      endif
      add(msgs, msg)
    endfor
    if len(diags) > max_per_line
      add(msgs, printf('+%d more', len(diags) - max_per_line))
    endif
    var best_sev = get(shown[0], 'severity', 3)
    var hl = best_sev <= 1 ? 'SimpleCCVirtualDiagError' : 'SimpleCCVirtualDiagWarn'
    try
      prop_type_add('SimpleCCVirtualDiag', {bufnr: bufnr, highlight: hl})
    catch
    endtry
    if lnum > 0 && lnum <= getbufinfo(bufnr)[0].linecount
      try
        prop_add(lnum, 0, {type: 'SimpleCCVirtualDiag', text: '  ' .. join(msgs, ' | '), text_align: 'after', bufnr: bufnr})
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
  # Restore inlay hints if they were lost (e.g. from async redraws)
  RestoreInlayHints()
  # Clear stale selection ranges
  s_selection_ranges = []
  # Show diagnostic float if enabled
  if g:simplecc_diag_float
    ShowDiagFloat()
  endif
enddef

def ShowDiagFloat()
  if s_diag_popup > 0
    popup_close(s_diag_popup)
    s_diag_popup = 0
  endif
  var uri = BufUri()
  var items = get(s_diagnostics, uri, [])
  if empty(items)
    return
  endif
  var cur_line = line('.') - 1
  var line_items = filter(copy(items), (_, v) => get(v, 'line', -1) == cur_line)
  if empty(line_items)
    return
  endif
  var lines: list<string> = []
  for item in line_items
    add(lines, DiagMessage(item))
  endfor
  s_diag_popup = popup_atcursor(lines, {
    border: [1, 1, 1, 1],
    borderchars: ['─', '│', '─', '│', '╭', '╮', '╯', '╰'],
    padding: [0, 1, 0, 1],
    moved: 'any',
    maxwidth: 80,
    highlight: 'Normal',
    borderhighlight: ['SimpleCCFloatBorder'],
  })
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
# Code Lens Execution (F7)
# ═════════════════════════════════════════════════════════

export def CodeLensRun()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif
  if empty(s_code_lens_cache)
    echo '[SimpleCC] No code lenses cached. Run :SimpleCCCodeLens first.'
    return
  endif
  var cur_line = line('.') - 1
  # Find lenses on or near current line
  var nearby: list<dict<any>> = []
  for l in s_code_lens_cache
    if abs(get(l, 'line', -999) - cur_line) <= 1
      add(nearby, l)
    endif
  endfor
  if empty(nearby)
    # Fall back to all lenses
    nearby = s_code_lens_cache
  endif
  if len(nearby) == 1
    DoCodeLensExecute(nearby[0])
  else
    var titles = mapnew(nearby, (_, l) => get(l, 'command_title', '(untitled)'))
    popup_menu(titles, {
      title: ' Run Code Lens ',
      border: [1, 1, 1, 1],
      borderchars: ['─', '│', '─', '│', '╭', '╮', '╯', '╰'],
      padding: [0, 1, 0, 1],
      callback: (_, idx) => {
        if idx > 0
          DoCodeLensExecute(nearby[idx - 1])
        endif
      },
    })
  endif
enddef

def DoCodeLensExecute(lens: dict<any>)
  var idx = get(lens, 'index', -1)
  if idx < 0
    return
  endif
  Send({
    type: 'codeLens/execute',
    id: NextId(),
    languageId: BufFt(),
    index: idx,
  })
enddef

def OnCodeLensExecute(ev: dict<any>)
  # If there's a workspace edit, it will come as applyEdit
  Log('code lens executed')
enddef

# ═════════════════════════════════════════════════════════
# Pull Diagnostics (F12)
# ═════════════════════════════════════════════════════════

export def PullDiagnostics()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif
  var uri = BufUri()
  if uri ==# 'file://'
    return
  endif
  Send({
    type: 'textDocument/pullDiagnostics',
    id: NextId(),
    uri: uri,
    languageId: BufFt(),
  })
enddef

# ═════════════════════════════════════════════════════════
# Type Hierarchy (F13)
# ═════════════════════════════════════════════════════════

var s_type_hierarchy_items: list<any> = []

export def Supertypes()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif
  s_type_hierarchy_items = []
  Send({
    type: 'textDocument/prepareTypeHierarchy',
    id: NextId(),
    uri: BufUri(),
    languageId: BufFt(),
    line: line('.') - 1,
    character: col('.') - 1,
  })
  b:simplecc_type_direction = 'supertypes'
enddef

export def Subtypes()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif
  s_type_hierarchy_items = []
  Send({
    type: 'textDocument/prepareTypeHierarchy',
    id: NextId(),
    uri: BufUri(),
    languageId: BufFt(),
    line: line('.') - 1,
    character: col('.') - 1,
  })
  b:simplecc_type_direction = 'subtypes'
enddef

def OnTypeHierarchyPrepare(ev: dict<any>)
  var items = get(ev, 'items', [])
  if empty(items)
    echo 'No type hierarchy item found'
    return
  endif
  s_type_hierarchy_items = items
  var item = items[0]
  var raw = get(item, 'raw', {})
  var direction = get(b:, 'simplecc_type_direction', 'supertypes')
  var req_type = 'typeHierarchy/' .. direction
  Send({
    type: req_type,
    id: NextId(),
    languageId: BufFt(),
    item: raw,
  })
enddef

def OnSupertypesResult(ev: dict<any>)
  var items = get(ev, 'items', [])
  if empty(items)
    echo 'No supertypes'
    return
  endif
  TypeHierarchyToQuickfix(items, 'Supertypes')
enddef

def OnSubtypesResult(ev: dict<any>)
  var items = get(ev, 'items', [])
  if empty(items)
    echo 'No subtypes'
    return
  endif
  TypeHierarchyToQuickfix(items, 'Subtypes')
enddef

def TypeHierarchyToQuickfix(items: list<any>, title: string)
  var qf_items: list<dict<any>> = []
  for item in items
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
# Workspace Symbol Live Search (F6)
# ═════════════════════════════════════════════════════════

export def WorkspaceSymbolLive()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif
  s_ws_input = ''
  s_ws_results = []
  s_ws_live = true

  s_ws_popup = popup_create(['Type to search workspace symbols...'], {
    title: ' Workspace Symbol ',
    border: [1, 1, 1, 1],
    borderchars: ['─', '│', '─', '│', '╭', '╮', '╯', '╰'],
    padding: [0, 1, 0, 1],
    pos: 'center',
    minwidth: 60,
    maxwidth: 80,
    minheight: 1,
    maxheight: 20,
    filter: (id, key) => WsSymbolFilter(id, key),
    callback: (id, result) => {
      s_ws_live = false
      if s_ws_results_popup > 0
        popup_close(s_ws_results_popup)
        s_ws_results_popup = 0
      endif
    },
  })
enddef

def WsSymbolFilter(id: number, key: string): bool
  if key ==# "\<Esc>"
    popup_close(id, -1)
    return true
  endif
  if key ==# "\<CR>"
    # Jump to selected result
    if !empty(s_ws_results)
      var sel = 0  # First result
      if sel < len(s_ws_results)
        popup_close(id, sel)
        var item = s_ws_results[sel]
        var uri = get(item, 'detail', get(item, 'uri', ''))
        var fpath = substitute(uri, '^file://', '', '')
        if fpath !=# '' && filereadable(fpath)
          execute 'edit ' .. fpath
          cursor(get(item, 'line', 0) + 1, get(item, 'character', 0) + 1)
          normal! zz
        endif
      endif
    endif
    return true
  endif
  if key ==# "\<BS>"
    if len(s_ws_input) > 0
      s_ws_input = s_ws_input[: -2]
    endif
  elseif len(key) == 1 && key =~ '[[:print:]]'
    s_ws_input ..= key
  else
    return false
  endif
  # Update popup content to show query
  popup_settext(id, ['> ' .. s_ws_input])
  # Debounced request
  if s_ws_timer > 0
    timer_stop(s_ws_timer)
  endif
  if len(s_ws_input) >= 2
    s_ws_timer = timer_start(300, (_) => {
      Send({
        type: 'workspace/symbol',
        id: NextId(),
        languageId: BufFt(),
        query: s_ws_input,
      })
    })
  endif
  return true
enddef

def UpdateWsResults()
  if !s_ws_live || s_ws_popup == 0
    return
  endif
  var lines = ['> ' .. s_ws_input, '']
  for item in s_ws_results[: 19]
    var kind = get(item, 'kind', '')
    var name = get(item, 'name', '')
    var detail = get(item, 'detail', '')
    var fpath = substitute(detail, '^file://', '', '')
    var short = fnamemodify(fpath, ':t')
    add(lines, printf('  [%s] %s  %s', kind, name, short))
  endfor
  if empty(s_ws_results)
    add(lines, '  (no results)')
  endif
  popup_settext(s_ws_popup, lines)
enddef

# ═════════════════════════════════════════════════════════
# Snippet Support (F9)
# ═════════════════════════════════════════════════════════

export def OnCompleteDone()
  if !s_initialized
    return
  endif
  var ci = v:completed_item
  if empty(ci)
    return
  endif
  var ud = get(ci, 'user_data', {})
  if type(ud) != v:t_dict
    return
  endif
  var is_snippet = get(ud, 'is_snippet', false)
  if !is_snippet
    return
  endif
  var snippet_text = get(ud, 'snippet_text', '')
  if snippet_text ==# ''
    return
  endif
  ExpandSnippet(ci, snippet_text)
enddef

def ExpandSnippet(ci: dict<any>, snippet: string)
  # Parse snippet: extract $N, ${N:placeholder}, $0
  var tabstops: list<dict<any>> = []
  var expanded = ''
  var i = 0
  var slen = len(snippet)
  while i < slen
    if snippet[i] ==# '$'
      if i + 1 < slen && snippet[i + 1] ==# '{'
        # ${N:placeholder} or ${N}
        var j = i + 2
        var num_str = ''
        while j < slen && snippet[j] =~ '\d'
          num_str ..= snippet[j]
          j += 1
        endwhile
        var placeholder = ''
        if j < slen && snippet[j] ==# ':'
          j += 1
          var depth = 1
          while j < slen && depth > 0
            if snippet[j] ==# '}'
              depth -= 1
              if depth == 0
                break
              endif
            elseif snippet[j] ==# '$' && j + 1 < slen && snippet[j + 1] ==# '{'
              depth += 1
            endif
            placeholder ..= snippet[j]
            j += 1
          endwhile
        elseif j < slen && snippet[j] ==# '}'
          # ${N} without placeholder
        endif
        if j < slen && snippet[j] ==# '}'
          j += 1
        endif
        add(tabstops, {num: str2nr(num_str), start: len(expanded), text: placeholder})
        expanded ..= placeholder
        i = j
      elseif i + 1 < slen && snippet[i + 1] =~ '\d'
        # $N
        var j = i + 1
        var num_str = ''
        while j < slen && snippet[j] =~ '\d'
          num_str ..= snippet[j]
          j += 1
        endwhile
        add(tabstops, {num: str2nr(num_str), start: len(expanded), text: ''})
        i = j
      else
        expanded ..= snippet[i]
        i += 1
      endif
    else
      expanded ..= snippet[i]
      i += 1
    endif
  endwhile

  if empty(tabstops)
    return
  endif

  # Sort tabstops: $1, $2, ... $0 last
  sort(tabstops, (a, b) => {
    if a.num == 0
      return 1
    endif
    if b.num == 0
      return -1
    endif
    return a.num - b.num
  })

  # Replace the completed word with expanded snippet text
  var word = get(ci, 'word', '')
  var lnum = line('.')
  var cur_col = col('.')
  var line_text = getline(lnum)
  var word_start = cur_col - len(word) - 1
  if word_start < 0
    word_start = 0
  endif
  var new_line = line_text[: word_start - 1] .. expanded .. line_text[cur_col - 1 :]
  setline(lnum, new_line)

  # Setup tabstop navigation
  s_snippet_active = true
  s_snippet_tabstops = []
  for ts in tabstops
    add(s_snippet_tabstops, {
      lnum: lnum,
      col: word_start + ts.start + 1,
      end_col: word_start + ts.start + len(ts.text) + 1,
      text: ts.text,
    })
  endfor
  s_snippet_idx = 0

  # Set buffer-local mappings for tab navigation
  inoremap <buffer> <Tab> <Cmd>call simplecc#SnippetNext()<CR>
  inoremap <buffer> <S-Tab> <Cmd>call simplecc#SnippetPrev()<CR>
  snoremap <buffer> <Tab> <Cmd>call simplecc#SnippetNext()<CR>
  snoremap <buffer> <S-Tab> <Cmd>call simplecc#SnippetPrev()<CR>

  # Jump to first tabstop
  SnippetJump()
enddef

export def SnippetNext()
  if !s_snippet_active || empty(s_snippet_tabstops)
    SnippetFinish()
    return
  endif
  s_snippet_idx += 1
  if s_snippet_idx >= len(s_snippet_tabstops)
    SnippetFinish()
    return
  endif
  SnippetJump()
enddef

export def SnippetPrev()
  if !s_snippet_active || empty(s_snippet_tabstops)
    return
  endif
  if s_snippet_idx > 0
    s_snippet_idx -= 1
  endif
  SnippetJump()
enddef

def SnippetJump()
  if s_snippet_idx >= len(s_snippet_tabstops)
    SnippetFinish()
    return
  endif
  var ts = s_snippet_tabstops[s_snippet_idx]
  cursor(ts.lnum, ts.col)
  if ts.text !=# '' && ts.end_col > ts.col
    # Select the placeholder
    execute printf("normal! v%dl\<C-g>", ts.end_col - ts.col - 1)
  endif
enddef

def SnippetFinish()
  s_snippet_active = false
  s_snippet_tabstops = []
  s_snippet_idx = -1
  try
    iunmap <buffer> <Tab>
    iunmap <buffer> <S-Tab>
    sunmap <buffer> <Tab>
    sunmap <buffer> <S-Tab>
  catch
  endtry
enddef

# ═════════════════════════════════════════════════════════
# Incremental Document Sync (F1)
# ═════════════════════════════════════════════════════════

def RegisterListener(bufnr: number)
  var key = string(bufnr)
  if has_key(s_listener_ids, key)
    return
  endif
  var lid = listener_add((bnr, start, end, added, changes) => {
    OnBufferChange(bnr, start, end, added)
  }, bufnr)
  s_listener_ids[key] = lid
enddef

def UnregisterListener(bufnr: number)
  var key = string(bufnr)
  if has_key(s_listener_ids, key)
    listener_remove(s_listener_ids[key])
    remove(s_listener_ids, key)
  endif
enddef

def OnBufferChange(bufnr: number, start: number, end: number, added: number)
  var uri = BufUri(bufnr)
  if !has_key(s_pending_changes, uri)
    s_pending_changes[uri] = []
  endif
  # start is 1-based first changed line, end is 1-based line after last changed (before)
  # added is lines added (negative means removed)
  var new_end = start + (end - start) + added
  if new_end < start
    new_end = start
  endif
  var text_lines = getbufline(bufnr, start, new_end - 1)
  var text = join(text_lines, "\n")
  if !empty(text_lines)
    text ..= "\n"
  endif
  add(s_pending_changes[uri], {
    range: {
      start: {line: start - 1, character: 0},
      end: {line: end - 1, character: 0},
    },
    text: text,
  })
enddef

# ═════════════════════════════════════════════════════════
# Auto Semantic Tokens (F2)
# ═════════════════════════════════════════════════════════

def RequestSemanticTokensDebounced()
  if !g:simplecc_semantic_tokens
    return
  endif
  if s_semtok_timer > 0
    timer_stop(s_semtok_timer)
  endif
  s_semtok_timer = timer_start(1000, (_) => {
    SemanticTokens()
  })
enddef

export def OnWinScrolled()
  if !s_initialized || !g:simplecc_semantic_tokens
    return
  endif
  if line('$') > g:simplecc_semtok_range_threshold
    RequestSemanticTokensDebounced()
  endif
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
