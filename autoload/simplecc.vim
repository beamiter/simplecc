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
var s_initializing: bool = false
var s_stopping: bool = false
var s_restart_pending: bool = false
var s_job_generation: number = 0
var s_initialize_id: number = 0
var s_next_id: number = 0
var s_cbs: dict<func> = {}
var s_root: string = ''
var s_julia_environment: string = ''
var s_log: list<string> = []
# Servers the user declined to install this session (avoids re-prompting)
var s_declined_installs: dict<bool> = {}

# Diagnostics state per URI
var s_diagnostics: dict<list<dict<any>>> = {}
# Document versions
var s_doc_versions: dict<number> = {}
# Last Vim changedtick known to have been queued to the daemon
var s_doc_changedticks: dict<number> = {}
# Change timer for debouncing
var s_change_timer: number = 0
# Completion timer
var s_comp_timer: number = 0
# Completion state
var s_comp_id: number = 0
var s_comp_generation: number = 0
var s_comp_requesting: bool = false
var s_comp_bufnr: number = -1
var s_comp_changedtick: number = -1
var s_comp_line: number = -1
var s_comp_col: number = -1
var s_comp_start_col: number = 0
var s_comp_original_line: string = ''
# Completion item resolve debounce / stale-response protection
var s_comp_resolve_timer: number = 0
var s_comp_resolve_id: number = 0
var s_comp_resolve_key: string = ''
var s_comp_resolve_request_key: string = ''
var s_comp_resolve_requested: dict<bool> = {}
var s_comp_resolved_items: dict<dict<any>> = {}
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
# Window/buffer context for asynchronous location requests.  LSP replies can
# arrive after the user has moved to another split, so navigation must not use
# whichever window happens to be current when the reply is handled.
var s_navigation_contexts: dict<dict<any>> = {}
# Completion preview state - for real-time preview of selected completion
var s_comp_preview_start_line: number = 0
var s_comp_preview_start_col: number = 0
var s_comp_preview_orig_text: string = ''

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

def OnBackendExit(generation: number, code: number)
  # A stopped daemon may exit after a replacement has already started.  Never
  # let that stale callback reset the replacement job's state.
  if generation != s_job_generation
    Log(printf('stale daemon generation %d exited with code %d', generation, code))
    return
  endif
  var restart = s_restart_pending
  s_restart_pending = false
  s_running = false
  s_initialized = false
  s_initializing = false
  s_stopping = false
  s_initialize_id = 0
  s_job = null_job
  s_cbs = {}
  s_navigation_contexts = {}
  if s_kill_timer > 0
    timer_stop(s_kill_timer)
    s_kill_timer = 0
  endif
  Log('daemon exited with code ' .. string(code))
  g:simplecc_status = ''
  if restart
    timer_start(0, (_) => Start())
  endif
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

  s_job_generation += 1
  var generation = s_job_generation
  try
    s_job = job_start([exe], {
      in_io: 'pipe',
      out_mode: 'nl',
      out_cb: (ch, line) => {
        if generation == s_job_generation
          OnBackendEvent(line)
        endif
      },
      err_mode: 'nl',
      err_cb: (ch, line) => {
        if generation == s_job_generation
          Log('stderr: ' .. line)
        endif
      },
      exit_cb: (ch, code) => {
        OnBackendExit(generation, code)
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
  s_stopping = false
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

def OnJuliaEnvironmentActivation(ev: dict<any>)
  if get(ev, 'type', '') !=# 'error'
    return
  endif
  echohl ErrorMsg
  echom '[SimpleCC] Failed to activate Julia environment: ' .. get(ev, 'message', 'unknown error')
  echohl None
enddef

def OnJuliaLanguageServerRefresh(ev: dict<any>)
  if get(ev, 'type', '') !=# 'error'
    return
  endif
  echohl ErrorMsg
  echom '[SimpleCC] Failed to refresh Julia language server: ' .. get(ev, 'message', 'unknown error')
  echohl None
enddef

def OnConfigurationReload(ev: dict<any>)
  if get(ev, 'type', '') !=# 'error'
    return
  endif
  echohl ErrorMsg
  echom '[SimpleCC] Failed to reload configuration: ' .. get(ev, 'message', 'unknown error')
  echohl None
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
    s_initializing = false
    s_initialize_id = 0
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

  elseif ev.type ==# 'juliaEnvironment'
    s_julia_environment = get(ev, 'path', '')
    Log('Julia environment activated: ' .. s_julia_environment)
    echo '[SimpleCC] Julia environment: ' .. s_julia_environment

  elseif ev.type ==# 'juliaRefreshed'
    Log('Julia language server symbol-cache refresh requested')
    echo '[SimpleCC] Julia language server refresh requested'

  elseif ev.type ==# 'configurationReloaded'
    var count = get(ev, 'servers', 0)
    Log(printf('configuration reloaded for %d running server(s)', count))
    echo printf('[SimpleCC] Configuration reloaded (%d running server%s updated)',
        count, count == 1 ? '' : 's')

  elseif ev.type ==# 'log'
    Log('[' .. get(ev, 'server', '') .. '] ' .. get(ev, 'message', ''))

  elseif ev.type ==# 'error'
    if id > 0 && id == s_initialize_id
      s_initializing = false
      s_initialize_id = 0
      echohl ErrorMsg
      echom '[SimpleCC] initialization failed: ' .. get(ev, 'message', 'unknown error')
      echohl None
    endif
    if id > 0 && id == s_comp_id
      s_comp_requesting = false
    endif
    if id == s_comp_resolve_id
      if s_comp_resolve_request_key !=# ''
            && has_key(s_comp_resolve_requested, s_comp_resolve_request_key)
        remove(s_comp_resolve_requested, s_comp_resolve_request_key)
      endif
      s_comp_resolve_id = 0
      s_comp_resolve_request_key = ''
    endif
    if id > 0 && has_key(s_navigation_contexts, string(id))
      remove(s_navigation_contexts, string(id))
    endif
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

export def ByteOffsetToUtf16(text: string, byte_offset: number): number
  var target = max([0, min([byte_offset, strlen(text)])])
  var byte_index = 0
  var char_index = 0
  var utf16_offset = 0
  var char_count = strchars(text)
  while char_index < char_count
    var char = strcharpart(text, char_index, 1)
    var char_bytes = strlen(char)
    if byte_index + char_bytes > target
      break
    endif
    utf16_offset += char2nr(char) >= 0x10000 ? 2 : 1
    byte_index += char_bytes
    char_index += 1
  endwhile
  return utf16_offset
enddef

export def Utf16ToByteOffset(text: string, utf16_offset: number): number
  var target = max([0, utf16_offset])
  var byte_offset = 0
  var utf16_index = 0
  var char_index = 0
  var char_count = strchars(text)
  while char_index < char_count
    var char = strcharpart(text, char_index, 1)
    var char_units = char2nr(char) >= 0x10000 ? 2 : 1
    # A position in the middle of a surrogate pair is invalid LSP input. Clamp
    # it to the beginning of that codepoint instead of splitting UTF-8 bytes.
    if utf16_index + char_units > target
      break
    endif
    utf16_index += char_units
    byte_offset += strlen(char)
    char_index += 1
  endwhile
  return byte_offset
enddef

def PercentEncodePath(path: string): string
  var encoded = ''
  var i = 0
  # str2list() returns Unicode codepoints under UTF-8, while URI escaping is
  # defined over the encoded bytes.  Read one byte at a time with strpart().
  while i < strlen(path)
    var byte = char2nr(strpart(path, i, 1))
    if (byte >= char2nr('A') && byte <= char2nr('Z'))
          || (byte >= char2nr('a') && byte <= char2nr('z'))
          || (byte >= char2nr('0') && byte <= char2nr('9'))
          || byte == char2nr('-') || byte == char2nr('.')
          || byte == char2nr('_') || byte == char2nr('~')
          || byte == char2nr('/') || byte == char2nr(':')
      encoded ..= nr2char(byte)
    else
      encoded ..= printf('%%%02X', byte)
    endif
    i += 1
  endwhile
  return encoded
enddef

def HexNibble(byte: number): number
  if byte >= char2nr('0') && byte <= char2nr('9')
    return byte - char2nr('0')
  endif
  if byte >= char2nr('A') && byte <= char2nr('F')
    return byte - char2nr('A') + 10
  endif
  if byte >= char2nr('a') && byte <= char2nr('f')
    return byte - char2nr('a') + 10
  endif
  return -1
enddef

def DecodeUtf8Bytes(bytes: list<number>): string
  var decoded = ''
  var i = 0
  while i < len(bytes)
    var first = bytes[i]
    if first < 0x80
      decoded ..= nr2char(first)
      i += 1
      continue
    endif

    var width = 0
    var codepoint = 0
    var minimum = 0
    if first >= 0xC2 && first <= 0xDF
      width = 2
      codepoint = and(first, 0x1F)
      minimum = 0x80
    elseif first >= 0xE0 && first <= 0xEF
      width = 3
      codepoint = and(first, 0x0F)
      minimum = 0x800
    elseif first >= 0xF0 && first <= 0xF4
      width = 4
      codepoint = and(first, 0x07)
      minimum = 0x10000
    else
      decoded ..= nr2char(0xFFFD)
      i += 1
      continue
    endif

    if i + width > len(bytes)
      decoded ..= nr2char(0xFFFD)
      i += 1
      continue
    endif
    var valid = true
    for offset in range(1, width - 1)
      var continuation = bytes[i + offset]
      if continuation < 0x80 || continuation > 0xBF
        valid = false
        break
      endif
      codepoint = codepoint * 64 + and(continuation, 0x3F)
    endfor
    if !valid || codepoint < minimum || codepoint > 0x10FFFF
          || (codepoint >= 0xD800 && codepoint <= 0xDFFF)
      decoded ..= nr2char(0xFFFD)
      i += 1
      continue
    endif
    decoded ..= nr2char(codepoint)
    i += width
  endwhile
  return decoded
enddef

def PercentDecodePath(encoded: string): string
  var bytes: list<number> = []
  var i = 0
  while i < strlen(encoded)
    var byte = char2nr(strpart(encoded, i, 1))
    if byte == char2nr('%') && i + 2 < strlen(encoded)
      var high = HexNibble(char2nr(strpart(encoded, i + 1, 1)))
      var low = HexNibble(char2nr(strpart(encoded, i + 2, 1)))
      if high >= 0 && low >= 0
        add(bytes, high * 16 + low)
        i += 3
        continue
      endif
    endif
    add(bytes, byte)
    i += 1
  endwhile
  return DecodeUtf8Bytes(bytes)
enddef

export def PathToUri(path: string): string
  if path ==# ''
    return 'file://'
  endif
  var absolute = fnamemodify(path, ':p')
  if has('win32') || has('win64')
    absolute = substitute(absolute, '\\', '/', 'g')
    if absolute =~# '^\a:/'
      absolute = '/' .. absolute
    endif
  endif
  if absolute =~# '^//[^/]'
    return 'file://' .. PercentEncodePath(strpart(absolute, 2))
  endif
  return 'file://' .. PercentEncodePath(absolute)
enddef

export def UriToPath(uri: string): string
  # Some daemon events intentionally carry an already-decoded filesystem path.
  if uri !~? '^file://'
    return uri
  endif
  var encoded = strpart(uri, strlen('file://'))
  if encoded =~? '^localhost/'
    encoded = strpart(encoded, strlen('localhost'))
  elseif encoded !=# '' && strpart(encoded, 0, 1) !=# '/'
    # Preserve a non-local URI authority as a UNC-style path.
    encoded = '//' .. encoded
  endif
  var path = PercentDecodePath(encoded)
  if (has('win32') || has('win64')) && path =~# '^/\a:/'
    path = strpart(path, 1)
  endif
  return path
enddef

def BufUri(buffer: number = 0): string
  var nr = buffer == 0 ? bufnr('%') : buffer
  return PathToUri(bufname(nr))
enddef

def CursorUtf16(): number
  return ByteOffsetToUtf16(getline('.'), col('.') - 1)
enddef

def Utf16LineColumn(line_text: string, utf16_offset: number): number
  return Utf16ToByteOffset(line_text, utf16_offset) + 1
enddef

def Utf16EndCursorColumn(line_text: string, utf16_offset: number): number
  var end_byte = Utf16ToByteOffset(line_text, utf16_offset)
  if end_byte <= 0
    return 1
  endif
  var prefix = strpart(line_text, 0, end_byte)
  var last_char = strcharpart(prefix, strchars(prefix) - 1, 1)
  return end_byte - strlen(last_char) + 1
enddef

def PathLine(path: string, lnum: number): string
  if path ==# '' || lnum <= 0
    return ''
  endif
  var bnr = bufnr(path)
  if bnr >= 0 && bufloaded(bnr)
    return get(getbufline(bnr, lnum), 0, '')
  endif
  if filereadable(path)
    return get(readfile(path, '', lnum), lnum - 1, '')
  endif
  return ''
enddef

def UriUtf16Column(uri: string, lnum: number, utf16_offset: number): number
  var path = UriToPath(uri)
  return Utf16LineColumn(PathLine(path, lnum), utf16_offset)
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
  Log(printf('SendDidOpen called: bufnr=%d', bufnr))
  if !s_initialized
    Log('SendDidOpen: not initialized')
    return
  endif
  var uri = BufUri(bufnr)
  var ft = BufFt(bufnr)
  Log(printf('SendDidOpen: uri=%s, ft=%s', uri, ft))
  if ft ==# '' || uri ==# 'file://'
    Log(printf('SendDidOpen: empty ft or uri, ft=%s, uri=%s', ft, uri))
    return
  endif
  var text = join(getbufline(bufnr, 1, '$'), "\n") .. "\n"
  var version = DocVersion(uri)
  Log(printf('SendDidOpen: sending didOpen, uri=%s, ft=%s, version=%d, text_len=%d', uri, ft, version, len(text)))
  Send({
    type: 'textDocument/didOpen',
    id: NextId(),
    uri: uri,
    languageId: ft,
    version: version,
    text: text,
  })
  # Clear any pending changes since we just sent the full document
  if has_key(s_pending_changes, uri)
    s_pending_changes[uri] = []
  endif
  # Register listener for incremental sync
  RegisterListener(bufnr)
  s_doc_changedticks[uri] = getbufvar(bufnr, 'changedtick')
enddef

def EnsureDocumentOpened(bufnr: number = 0)
  var bnr = bufnr == 0 ? bufnr('%') : bufnr
  var uri = BufUri(bnr)
  var ft = BufFt(bnr)
  # Check if document is already opened
  if has_key(s_doc_versions, uri)
    Log(printf('EnsureDocumentOpened: already opened, uri=%s', uri))
    return
  endif
  # Check if valid buffer
  if ft ==# '' || uri ==# 'file://'
    Log(printf('EnsureDocumentOpened: invalid buffer, ft=%s, uri=%s', ft, uri))
    return
  endif
  # Send didOpen now
  Log(printf('EnsureDocumentOpened: sending didOpen for uri=%s', uri))
  SendDidOpen(bnr)
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
  # Ensure document is opened before sending changes
  if !has_key(s_doc_versions, uri)
    Log(printf('SendDidChange: document not opened yet, sending didOpen first, uri=%s', uri))
    SendDidOpen(bufnr('%'))
    return
  endif
  listener_flush(bufnr('%'))
  var version = DocVersion(uri)
  # For newly opened documents (version <= 2), always send full text to avoid
  # incremental sync issues when large content is pasted into an empty file
  var use_incremental = has_key(s_pending_changes, uri) && !empty(s_pending_changes[uri]) && version > 2
  if use_incremental
    var changes = s_pending_changes[uri]
    s_pending_changes[uri] = []
    Log(printf('SendDidChange: incremental, uri=%s, version=%d, changes=%d', uri, version, len(changes)))
    Send({
      type: 'textDocument/didChange',
      id: NextId(),
      uri: uri,
      version: version,
      changes: changes,
    })
  else
    var text = join(getline(1, '$'), "\n") .. "\n"
    # Clear pending changes since we're sending full text
    if has_key(s_pending_changes, uri)
      s_pending_changes[uri] = []
    endif
    Log(printf('SendDidChange: full text, uri=%s, version=%d, text_len=%d', uri, version, len(text)))
    Send({
      type: 'textDocument/didChange',
      id: NextId(),
      uri: uri,
      version: version,
      text: text,
    })
  endif
  s_doc_changedticks[uri] = b:changedtick
enddef

export def OnBufOpen()
  Log(printf('OnBufOpen called: buf=%s, ft=%s, name=%s', bufnr('%'), &filetype, bufname('%')))
  if !s_initialized
    Log('OnBufOpen: not initialized')
    return
  endif
  var ft = &filetype
  if ft ==# '' || bufname('%') ==# ''
    Log(printf('OnBufOpen: empty ft or bufname, ft=%s, name=%s', ft, bufname('%')))
    return
  endif
  var uri = BufUri()
  # Avoid sending duplicate didOpen for the same document
  if has_key(s_doc_versions, uri)
    Log(printf('OnBufOpen: document already opened, uri=%s', uri))
    return
  endif
  Log(printf('OnBufOpen: calling SendDidOpen, uri=%s', uri))
  SendDidOpen(bufnr('%'))
  RequestInlayHintsDebounced()
  RequestSemanticTokensDebounced()
enddef

export def OnBufEnter()
  if !s_initialized || BufFt() ==# '' || BufUri() ==# 'file://'
    return
  endif
  # Inlay hints are pull-based. Re-requesting on buffer entry also makes live
  # Julia configuration changes visible after returning from simplecc.json.
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
  if IsActiveConfigBuffer()
    ReloadConfiguration()
  endif
  RequestInlayHintsDebounced()
  RequestSemanticTokensDebounced()
  # F12: Auto pull diagnostics if enabled
  if g:simplecc_pull_diagnostics
    PullDiagnostics()
  endif
enddef

export def OnBufClose(buffer: number = 0)
  var bnr = buffer > 0 ? buffer : bufnr('%')
  var uri = BufUri(bnr)
  UnregisterListener(bnr)
  if uri ==# 'file://'
    return
  endif
  if s_initialized && has_key(s_doc_versions, uri)
    Send({type: 'textDocument/didClose', id: NextId(), uri: uri})
  endif
  if has_key(s_doc_versions, uri)
    remove(s_doc_versions, uri)
  endif
  if has_key(s_diagnostics, uri)
    remove(s_diagnostics, uri)
  endif
  if has_key(s_pending_changes, uri)
    remove(s_pending_changes, uri)
  endif
  if has_key(s_doc_changedticks, uri)
    remove(s_doc_changedticks, uri)
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
  s_change_timer = timer_start(g:simplecc_change_delay, (_) => {
    s_change_timer = 0
    SendDidChange()
    # F3: Re-request inlay hints after changes
    RequestInlayHintsDebounced()
    # F2: Auto semantic tokens
    RequestSemanticTokensDebounced()
  })

  # TextChangedI is a more accurate completion trigger than relying only on
  # CursorMovedI. TriggerCompletion itself snapshots the cursor and changedtick,
  # so duplicate events collapse into one request.
  if mode() =~# '^i' && g:simplecc_auto_complete && !pumvisible()
    TriggerCompletion()
  endif
enddef

# ═════════════════════════════════════════════════════════
# Completion
# ═════════════════════════════════════════════════════════

export def OnCursorMovedI()
  if !s_initialized || !g:simplecc_auto_complete
    return
  endif
  # Don't trigger completion if menu is open - prevent interference
  if pumvisible()
    return
  endif
  TriggerCompletion()
enddef

export def OnInsertLeave()
  if s_comp_timer > 0
    timer_stop(s_comp_timer)
    s_comp_timer = 0
  endif
  if s_comp_resolve_timer > 0
    timer_stop(s_comp_resolve_timer)
    s_comp_resolve_timer = 0
  endif
  s_comp_requesting = false
  s_comp_resolve_id = 0
  s_comp_resolve_key = ''
  s_comp_resolve_request_key = ''
  s_comp_resolve_requested = {}
  s_comp_resolved_items = {}
  s_comp_original_line = ''
  CloseSignaturePopup()
  # Clear completion preview state
  s_comp_preview_start_line = 0
  s_comp_preview_start_col = 0
  s_comp_preview_orig_text = ''
enddef

export def OnInsertCharPre()
  if !pumvisible()
    return
  endif

  var key = v:char
  # Keep filtering while entering an identifier. Whitespace and punctuation
  # close the old menu; TextChangedI can then request a context-triggered menu.
  if key =~# '\k'
    return
  endif

  feedkeys("\<C-e>", 'n')
enddef

def ResolveCompletionItem(key: string, generation: number, item_index: number, ft: string)
  s_comp_resolve_timer = 0
  if !pumvisible() || s_comp_resolve_key !=# key
    return
  endif
  var resolve_id = NextId()
  s_comp_resolve_id = resolve_id
  s_comp_resolve_request_key = key
  s_comp_resolve_requested[key] = true
  Send({
    type: 'completionItem/resolve',
    id: resolve_id,
    languageId: ft,
    generation: generation,
    index: item_index,
  })
enddef

def ShowCompletionDocumentation(item: dict<any>)
  var detail = get(item, 'detail', '')
  var doc = get(item, 'documentation', '')
  var text = detail
  if doc !=# ''
    text = text !=# '' ? text .. "\n\n" .. doc : doc
  endif

  if s_hover_popup > 0
    popup_close(s_hover_popup)
    s_hover_popup = 0
  endif
  if text ==# '' || !pumvisible()
    return
  endif

  s_hover_popup = popup_create(split(text, "\n"), {
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

export def OnCompleteChanged()
  if !s_initialized
    return
  endif

  if s_comp_resolve_timer > 0
    timer_stop(s_comp_resolve_timer)
    s_comp_resolve_timer = 0
  endif

  var info = complete_info(['selected', 'items'])
  var sel = get(info, 'selected', -1)
  if sel < 0
    s_comp_resolve_key = ''
    return
  endif
  var items = get(info, 'items', [])
  if sel >= len(items)
    return
  endif

  var ci = items[sel]
  var ud = get(ci, 'user_data', {})
  if type(ud) == v:t_dict
    var generation = get(ud, 'generation', 0)
    var item_index = get(ud, 'index', -1)
    if generation > 0 && item_index >= 0
      var key = printf('%d:%d', generation, item_index)
      s_comp_resolve_key = key
      if has_key(s_comp_resolved_items, key)
        ShowCompletionDocumentation(s_comp_resolved_items[key])
        return
      endif
      if get(s_comp_resolve_requested, key, false)
        return
      endif

      var ft = BufFt()
      s_comp_resolve_timer = timer_start(
        g:simplecc_complete_resolve_delay,
        (_) => ResolveCompletionItem(key, generation, item_index, ft))
    endif
  endif
enddef

def OnCompletionResolve(ev: dict<any>)
  if get(ev, 'id', 0) != s_comp_resolve_id
    return
  endif
  var key = s_comp_resolve_request_key
  s_comp_resolve_id = 0
  s_comp_resolve_request_key = ''
  var item = get(ev, 'item', {})
  if empty(item)
    return
  endif
  if key !=# ''
    s_comp_resolved_items[key] = item
  endif
  # Selection may have changed while the server was resolving the old item.
  # Cache every valid response, but only display the currently selected one.
  if key ==# s_comp_resolve_key
    ShowCompletionDocumentation(item)
  endif
enddef

def TriggerCompletion()
  if s_comp_timer > 0
    timer_stop(s_comp_timer)
  endif

  var bnr = bufnr('%')
  var tick = b:changedtick
  var lnum = line('.')
  var ccol = col('.')
  s_comp_timer = timer_start(g:simplecc_complete_delay, (_) => {
    s_comp_timer = 0
    if mode() !~# '^i' || bufnr('%') != bnr || b:changedtick != tick
          || line('.') != lnum || col('.') != ccol
      return
    endif
    RequestCompletion(false)
  })
enddef

export def TriggerCompletionManual()
  if s_comp_timer > 0
    timer_stop(s_comp_timer)
  endif
  RequestCompletion(true)
enddef

export def SelectTabKey(): string
  if pumvisible()
    # Menu is open, navigate/select in menu
    return "\<C-n>"
  endif
  var byte_col = col('.') - 1
  var before = strpart(getline('.'), 0, byte_col)
  if !s_initialized || BufFt() ==# '' || byte_col <= 0
        || before ==# '' || before =~# '\s$'
    return "\<Tab>"
  endif
  # Menu is closed and the cursor is in a completion context.
  TriggerCompletionManual()
  return ''
enddef

export def SelectShiftTabKey(): string
  if pumvisible()
    # Menu is open, move to previous item
    return "\<C-p>"
  else
    return "\<S-Tab>"
  endif
enddef

export def SelectDownKey(): string
  if pumvisible()
    # Menu is open, move down
    return "\<C-n>"
  else
    # Menu is closed, move cursor down normally
    return "\<Down>"
  endif
enddef

export def SelectUpKey(): string
  if pumvisible()
    # Menu is open, move up
    return "\<C-p>"
  else
    # Menu is closed, move cursor up normally
    return "\<Up>"
  endif
enddef

export def SelectEnterKey(): string
  if !pumvisible()
    return "\<CR>"
  endif

  var selected = get(complete_info(['selected']), 'selected', -1)
  # With completeopt=noselect, Enter must still insert a newline when the user
  # has not explicitly selected a candidate.
  return selected >= 0 ? "\<C-y>" : "\<C-e>\<CR>"
enddef

def SyncDocumentForCompletion()
  if s_change_timer > 0
    timer_stop(s_change_timer)
    s_change_timer = 0
  endif

  var uri = BufUri()
  if uri !=# 'file://' && get(s_doc_changedticks, uri, -1) != b:changedtick
    SendDidChange()
  endif
enddef

def RequestCompletion(manual: bool = false)
  var ft = BufFt()
  if ft ==# '' || pumvisible()
    return
  endif

  var ccol = col('.')
  if ccol <= 1
    return
  endif
  var line_text = getline('.')
  var before = line_text[: ccol - 2]
  if before ==# '' || before =~ '\s$'
    return
  endif

  # Work in Vim byte columns, matching col() and complete(). Use 'iskeyword'
  # instead of \w so language-specific identifier characters are respected.
  var start = ccol - 1
  while start > 0 && line_text[start - 1] =~# '\k'
    start -= 1
  endwhile
  var prefix = start < ccol - 1 ? line_text[start : ccol - 2] : ''
  var is_trigger = start > 0
        && line_text[start - 1] =~# '[^[:alnum:]_[:space:]]'
  if strchars(prefix) < g:simplecc_complete_min_chars && !is_trigger
    return
  endif
  var trigger_character = !manual && is_trigger ? line_text[start - 1] : ''
  var trigger_kind = manual
        ? 1
        : (trigger_character !=# '' ? 2 : 3)

  # Queue the latest buffer text before the completion request. Both messages
  # use the same Vim -> daemon channel, eliminating the stale-text window on
  # the editor side.
  SyncDocumentForCompletion()

  var uri = BufUri()
  var lnum = line('.') - 1
  var cchar = ByteOffsetToUtf16(line_text, ccol - 1)

  var id = NextId()
  s_comp_id = id
  s_comp_requesting = true
  s_comp_bufnr = bufnr('%')
  s_comp_changedtick = b:changedtick
  s_comp_line = line('.')
  s_comp_col = ccol
  s_comp_start_col = start
  s_comp_original_line = line_text
  s_comp_resolve_id = 0
  s_comp_resolve_key = ''
  s_comp_resolve_request_key = ''
  s_comp_resolve_requested = {}
  s_comp_resolved_items = {}

  var max_items = max([1, g:simplecc_complete_max_items])
  Send({
    type: 'textDocument/completion',
    id: id,
    uri: uri,
    languageId: ft,
    line: lnum,
    character: cchar,
    maxItems: max_items,
    triggerKind: trigger_kind,
    triggerCharacter: trigger_character,
  })
enddef

# Collect keyword tokens from open buffers that start with `prefix`, skipping
# anything already offered by the language server (`existing`). Lines are visited
# outward from the cursor so nearby, more relevant identifiers surface first and
# the scan stops as soon as `limit` candidates are found. Other loaded buffers of
# the same filetype are scanned after the current one for cross-file matches.
def CollectBufferWords(prefix: string, existing: dict<bool>, limit: number): list<dict<any>>
  var out: list<dict<any>> = []
  if limit <= 0 || prefix ==# ''
    return out
  endif
  var ic = &ignorecase
  var lower_prefix = tolower(prefix)
  var plen = strlen(prefix)
  var seen: dict<bool> = {}
  var cur = line('.')

  # Build the list of (bufnr, lnum) to visit. Current buffer first, ordered by
  # distance from the cursor; then other loaded same-filetype buffers top-down.
  var cur_buf = bufnr('%')
  var cur_ft = &filetype
  var total = line('$')
  var sources: list<list<number>> = [[cur_buf, cur]]
  var d = 1
  while cur - d >= 1 || cur + d <= total
    if cur - d >= 1
      add(sources, [cur_buf, cur - d])
    endif
    if cur + d <= total
      add(sources, [cur_buf, cur + d])
    endif
    d += 1
  endwhile
  for b in getbufinfo({'buflisted': 1, 'bufloaded': 1})
    if b.bufnr == cur_buf || getbufvar(b.bufnr, '&filetype', '') !=# cur_ft
      continue
    endif
    for lnum in range(1, b.linecount)
      add(sources, [b.bufnr, lnum])
    endfor
  endfor

  var last_buf = -1
  var buf_lines: list<string> = []
  for src in sources
    if len(out) >= limit
      break
    endif
    if src[0] != last_buf
      buf_lines = getbufline(src[0], 1, '$')
      last_buf = src[0]
    endif
    var text = get(buf_lines, src[1] - 1, '')
    for w in split(text, '\%(\k\)\@!.')
      if len(out) >= limit
        break
      endif
      # Skip words no longer than the prefix (this also drops the word the user
      # is currently typing).
      if strlen(w) <= plen
        continue
      endif
      if ic ? stridx(tolower(w), lower_prefix) != 0 : stridx(w, prefix) != 0
        continue
      endif
      var lw = ic ? tolower(w) : w
      if has_key(seen, lw) || has_key(existing, lw)
        continue
      endif
      seen[lw] = true
      add(out, {
        word: w,
        abbr: w,
        menu: 'buf',
        dup: 1,
        icase: ic ? 1 : 0,
        user_data: {source: 'buffer'},
      })
    endfor
  endfor
  return out
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

  # generation <= 0 means the server offered no completion (no capability or no
  # server for this filetype). Buffer-word completion can still fire below.
  var generation = get(ev, 'generation', 0)
  s_comp_generation = generation

  # A response is only valid for the exact editor snapshot that requested it.
  if mode() !~# '^i' || bufnr('%') != s_comp_bufnr
        || b:changedtick != s_comp_changedtick
        || line('.') != s_comp_line || col('.') != s_comp_col
    return
  endif

  var items = generation > 0 ? get(ev, 'items', []) : []

  var line_text = getline('.')
  var start = s_comp_start_col

  # Build Vim complete items
  var complete_items: list<dict<any>> = []
  # Words already offered by the server, so buffer completion never duplicates
  # them. Keyed with the same case-folding used when matching buffer words.
  var ic = &ignorecase
  var existing: dict<bool> = {}
  var idx = 0
  var max_items = max([1, g:simplecc_complete_max_items])
  for item in items
    if idx >= max_items
      break
    endif
    var word = get(item, 'insert_text', get(item, 'label', ''))
    var is_snippet = get(item, 'is_snippet', false)
    # For snippet items, show the label as word rather than raw snippet text
    if is_snippet
      word = get(item, 'label', word)
    endif
    var item_index = get(item, 'index', idx)
    var additional_edits = get(item, 'additional_text_edits', [])
    var ci: dict<any> = {
      word: word,
      abbr: get(item, 'label', ''),
      menu: get(item, 'kind', '') .. (is_snippet ? ' ~' : ''),
      dup: 1,
      icase: &ignorecase ? 1 : 0,
      user_data: {
        generation: generation,
        index: item_index,
        is_snippet: is_snippet,
        snippet_text: is_snippet ? get(item, 'insert_text', '') : '',
        text_edit: get(item, 'text_edit', {}),
        additional_text_edits: additional_edits,
        commit_characters: get(item, 'commit_characters', []),
      },
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
    if word !=# ''
      existing[ic ? tolower(word) : word] = true
    endif
    idx += 1
  endfor

  # Supplement server results with keyword matches from open buffers. This also
  # provides completion before the server is ready or in files with no server.
  if g:simplecc_complete_buffer_words
    var prefix = start < s_comp_col - 1 ? line_text[start : s_comp_col - 2] : ''
    var buf_limit = max([0, g:simplecc_complete_buffer_max_items])
    extend(complete_items, CollectBufferWords(prefix, existing, buf_limit))
  endif

  if empty(complete_items)
    return
  endif

  if mode() ==# 'i'
    # Save completion start position for preview
    var current_line_nr = line('.')
    s_comp_preview_start_line = current_line_nr - 1
    s_comp_preview_start_col = start
    s_comp_preview_orig_text = start < s_comp_col - 1
          ? line_text[start : s_comp_col - 2] : ''

    # Save original completeopt and configure
    var saved_completeopt = &completeopt
    set completeopt=menu,menuone,noselect,noinsert
    complete(start + 1, complete_items)
    # Restore after complete() returns
    &completeopt = saved_completeopt
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
    character: CursorUtf16(),
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

def RememberNavigationContext(id: number)
  s_navigation_contexts[string(id)] = {
    winid: win_getid(),
    bufnr: bufnr('%'),
  }
enddef

def TakeNavigationContext(ev: dict<any>): dict<any>
  var key = string(get(ev, 'id', 0))
  if has_key(s_navigation_contexts, key)
    return remove(s_navigation_contexts, key)
  endif
  return {winid: win_getid(), bufnr: bufnr('%')}
enddef

def GoToNavigationWindow(context: dict<any>): bool
  var winid = get(context, 'winid', 0)
  if winid > 0 && win_gotoid(winid)
    return true
  endif

  # If the original split was closed while the request was pending, prefer
  # another window showing the request buffer before falling back to the
  # user's current window.
  var bufnr = get(context, 'bufnr', 0)
  if bufnr > 0
    var wins = win_findbuf(bufnr)
    if !empty(wins) && win_gotoid(wins[0])
      return true
    endif
  endif
  return &buftype !=# 'quickfix'
enddef

export def Definition()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif

  var id = NextId()
  RememberNavigationContext(id)
  Send({
    type: 'textDocument/definition',
    id: id,
    uri: BufUri(),
    languageId: BufFt(),
    line: line('.') - 1,
    character: CursorUtf16(),
    symbol: expand('<cword>'),
  })
enddef

def OnDefinition(ev: dict<any>)
  var context = TakeNavigationContext(ev)
  var locs = get(ev, 'locations', [])
  if empty(locs)
    echo 'No definition found'
    return
  endif

  if len(locs) == 1
    JumpToLocation(locs[0], context)
  else
    LocationsToQuickfix(locs, 'Definition', context)
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
  RememberNavigationContext(id)
  Send({
    type: 'textDocument/references',
    id: id,
    uri: BufUri(),
    languageId: BufFt(),
    line: line('.') - 1,
    character: CursorUtf16(),
  })
enddef

def OnReferences(ev: dict<any>)
  var context = TakeNavigationContext(ev)
  var locs = get(ev, 'locations', [])
  if empty(locs)
    echo 'No references found'
    return
  endif

  LocationsToQuickfix(locs, 'References', context)
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
  var cchar = CursorUtf16()
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
  var uri = BufUri()
  var ft = BufFt()
  Log(printf('Format called: uri=%s, ft=%s, has_version=%d', uri, ft, has_key(s_doc_versions, uri)))
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif

  # Ensure document is opened before formatting
  EnsureDocumentOpened()

  var id = NextId()
  Log(printf('Format: sending formatting request, id=%d', id))
  Send({
    type: 'textDocument/formatting',
    id: id,
    uri: uri,
    languageId: ft,
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
  # Log edits for debugging
  for edit in edits
    Log(printf('Format edit: line %d-%d, char %d-%d: %s', get(edit, 'line', 0), get(edit, 'end_line', 0), get(edit, 'character', 0), get(edit, 'end_character', 0), json_encode(edit)))
  endfor
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
    character: CursorUtf16(),
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
    character: CursorUtf16(),
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
  var fpath = UriToPath(uri)
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
    var el = get(item, 'end_line', get(item, 'line', 0)) + 1
    var start_line = get(getbufline(bufnr, sl), 0, '')
    var end_line = get(getbufline(bufnr, el), 0, '')
    var sc = Utf16LineColumn(start_line, get(item, 'character', 0))
    var ec = Utf16LineColumn(end_line, get(item, 'end_character', 0))
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
  var fpath = UriToPath(uri)
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
      col: Utf16LineColumn(
          getline(get(item, 'line', 0) + 1), get(item, 'character', 0)),
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
      cursor(item.line + 1,
          Utf16LineColumn(getline(item.line + 1), get(item, 'character', 0)))
      echo DiagMessage(item)
      return
    endif
  endfor
  # Wrap around
  var first = items[0]
  cursor(first.line + 1,
      Utf16LineColumn(getline(first.line + 1), get(first, 'character', 0)))
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
      cursor(item.line + 1,
          Utf16LineColumn(getline(item.line + 1), get(item, 'character', 0)))
      echo DiagMessage(item)
      return
    endif
  endfor
  # Wrap around
  var last = sorted[0]
  cursor(last.line + 1,
      Utf16LineColumn(getline(last.line + 1), get(last, 'character', 0)))
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
    elseif !get(s_declined_installs, server, false)
      var prompt = msg !=# ''
        ? printf("[SimpleCC] %s\nInstall %s now?", msg, server)
        : printf('[SimpleCC] %s is not installed. Install it now?', server)
      if confirm(prompt, "&Yes\n&No", 1) == 1
        DoInstall(server)
      else
        s_declined_installs[server] = true
        echohl WarningMsg
        echom printf('[SimpleCC] Skipped. Run :SimpleCCInstall %s to install later.', server)
        echohl None
      endif
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
    var fpath = UriToPath(uri)
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

export def ApplyTextEdits(bufnr: number, edits: list<dict<any>>)
  # Sort edits in reverse order to avoid offset issues
  var sorted = sort(copy(edits), (a, b) => {
    if a.line != b.line
      return b.line - a.line
    endif
    return b.character - a.character
  })

  for edit in sorted
    var sl = get(edit, 'line', 0) + 1
    var el = get(edit, 'end_line', get(edit, 'line', 0)) + 1
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

    # LSP columns are UTF-16 code units; Vim buffer APIs and strpart() use bytes.
    var start_offset = get(edit, 'character', 0)
    var end_offset = get(edit, 'end_character', 0)

    var sc = Utf16ToByteOffset(lines[0], start_offset)
    var ec = Utf16ToByteOffset(lines[-1], end_offset)

    # Build the replacement
    var prefix = strpart(lines[0], 0, sc)
    var suffix = strpart(lines[-1], ec)

    new_lines[0] = prefix .. new_lines[0]
    new_lines[-1] = new_lines[-1] .. suffix

    # Keep one line in place so an empty replacement does not accidentally
    # delete the entire logical line.
    setbufline(bufnr, sl, new_lines[0])
    if el > sl
      deletebufline(bufnr, sl + 1, el)
    endif
    if len(new_lines) > 1
      appendbufline(bufnr, sl, new_lines[1 :])
    endif
  endfor
enddef

# ═════════════════════════════════════════════════════════
# Location helpers
# ═════════════════════════════════════════════════════════

def PushNavigationOrigin()
  # m' updates both the previous-context mark and this window's jumplist.
  # cursor() and same-buffer LSP jumps do not do that by themselves.
  normal! m'

  var cur_item = {
    bufnr: bufnr('%'),
    from: getpos('.'),
    tagname: expand('<cword>'),
  }
  try
    settagstack(winnr(), {items: [cur_item]}, 'a')
  catch
  endtry
enddef

def JumpToFilePosition(fpath: string, lnum: number, col: number)
  PushNavigationOrigin()
  if fpath !=# expand('%:p')
    execute 'edit ' .. fnameescape(fpath)
  endif
  cursor(lnum, col > 0 ? col : 1)
  normal! zz
enddef

def JumpToLocation(loc: dict<any>, context: dict<any> = {})
  if !empty(context) && !GoToNavigationWindow(context)
    echohl ErrorMsg
    echom '[SimpleCC] originating window is no longer available'
    echohl None
    return
  endif

  var uri = get(loc, 'uri', '')
  var lnum = get(loc, 'line', 0) + 1
  var fpath = UriToPath(uri)
  PushNavigationOrigin()
  if fpath !=# expand('%:p')
    execute 'edit ' .. fnameescape(fpath)
  endif
  cursor(lnum, Utf16LineColumn(getline(lnum), get(loc, 'character', 0)))
  normal! zz
enddef

def LocationsToQuickfix(locs: list<dict<any>>, title: string,
    context: dict<any> = {})
  var qf_items: list<dict<any>> = []
  for loc in locs
    var uri = get(loc, 'uri', '')
    var fpath = UriToPath(uri)
    add(qf_items, {
      filename: fpath,
      lnum: get(loc, 'line', 0) + 1,
      col: UriUtf16Column(uri, get(loc, 'line', 0) + 1,
          get(loc, 'character', 0)),
      text: title,
    })
  endfor

  if !empty(context) && !GoToNavigationWindow(context)
    echohl ErrorMsg
    echom '[SimpleCC] originating window is no longer available'
    echohl None
    return
  endif

  # LSP result lists belong to the split that issued the request.  A location
  # list gives each source split its own list and, unlike :copen, can be placed
  # directly below that split in a multi-window layout.
  var source_winid = win_getid()
  setloclist(0, qf_items, 'r')
  belowright lopen
  w:simplecc_source_winid = source_winid
  SetupQfMappings()
enddef

def SetupQfMappings()
  # Buffer-local mappings on the quickfix window: <CR> jumps and auto-closes
  # the list; visual <CR> opens every selected entry (first in the window,
  # the rest in splits).
  nnoremap <buffer> <CR> <Cmd>call simplecc#QfEnter()<CR>
  xnoremap <buffer> <CR> :<C-u>call simplecc#QfEnterMulti()<CR>
enddef

export def QfEnter()
  var lnum = line('.')
  var info = get(getwininfo(win_getid()), 0, {})
  var is_loclist = get(info, 'loclist', 0) == 1
  var items = is_loclist ? getloclist(0) : getqflist()
  var source_winid = get(w:, 'simplecc_source_winid', 0)
  if lnum < 1 || lnum > len(items)
    return
  endif
  var item = items[lnum - 1]

  if is_loclist
    lclose
  else
    cclose
  endif
  if source_winid > 0 && !win_gotoid(source_winid)
    echohl ErrorMsg
    echom '[SimpleCC] originating window is no longer available'
    echohl None
    return
  endif

  var fname = bufname(get(item, 'bufnr', 0))
  if fname ==# ''
    return
  endif
  JumpToFilePosition(fnamemodify(fname, ':p'), get(item, 'lnum', 1),
      get(item, 'col', 1))
enddef

export def QfEnterMulti()
  var lstart = line("'<")
  var lend = line("'>")
  var info = get(getwininfo(win_getid()), 0, {})
  var is_loclist = get(info, 'loclist', 0) == 1
  var items = is_loclist ? getloclist(0) : getqflist()
  var source_winid = get(w:, 'simplecc_source_winid', 0)
  if is_loclist
    lclose
  else
    cclose
  endif
  if source_winid > 0 && !win_gotoid(source_winid)
    echohl ErrorMsg
    echom '[SimpleCC] originating window is no longer available'
    echohl None
    return
  endif
  var first = true
  for i in range(lstart - 1, lend - 1)
    if i < 0 || i >= len(items)
      continue
    endif
    var it = items[i]
    if it.bufnr == 0
      continue
    endif
    var fname = bufname(it.bufnr)
    if first
      JumpToFilePosition(fnamemodify(fname, ':p'), it.lnum,
          it.col > 0 ? it.col : 1)
      first = false
    else
      execute 'split ' .. fnameescape(fname)
      cursor(it.lnum, it.col > 0 ? it.col : 1)
      normal! zz
    endif
  endfor
enddef

# ═════════════════════════════════════════════════════════
# Public API
# ═════════════════════════════════════════════════════════

export def Start()
  if s_initialized || s_initializing
    Log('Start: already initialized or initializing')
    return
  endif
  if s_stopping
    Log('Start: daemon is stopping')
    return
  endif
  if !EnsureBackend()
    return
  endif
  # Detect project root
  s_root = FindProjectRoot()
  s_julia_environment = ''
  if filereadable(s_root .. '/JuliaProject.toml') || filereadable(s_root .. '/Project.toml')
    s_julia_environment = s_root
  endif
  Log('project root: ' .. s_root)

  var id = NextId()
  s_initialize_id = id
  s_initializing = true
  var configured = get(g:, 'simplecc_config_path', '')
  var config_path = configured ==# '' ? '' : fnamemodify(expand(configured), ':p')
  Send({
    type: 'initialize',
    id: id,
    root: s_root,
    config_path: config_path,
  })
enddef

export def Stop(restarting: bool = false)
  # A manual stop cancels a queued restart; Restart() keeps the intent until
  # the exact daemon generation has really exited.
  s_restart_pending = restarting
  if !IsRunning() || s_stopping
    return
  endif
  s_stopping = true
  s_initialized = false
  s_initializing = false
  s_initialize_id = 0
  g:simplecc_status = 'stopping'
  var generation = s_job_generation
  var job_to_stop = s_job
  Send({type: 'shutdown', id: NextId()})
  timer_start(500, (_) => {
    if generation != s_job_generation
      return
    endif
    if job_to_stop != null_job && job_status(job_to_stop) ==# 'run'
      job_stop(job_to_stop)
      # Force kill if still running after 3 seconds
      s_kill_timer = timer_start(3000, (timer) => {
        if s_kill_timer == timer
          s_kill_timer = 0
        endif
        if generation != s_job_generation
          return
        endif
        if job_to_stop != null_job && job_status(job_to_stop) ==# 'run'
          job_stop(job_to_stop, 'kill')
          Log('daemon force-killed')
        endif
      })
    endif
  })
enddef

export def Restart()
  if IsRunning() || s_stopping
    Stop(true)
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
  var julia_env = s_julia_environment ==# '' ? '' : ' | Julia env: ' .. s_julia_environment
  echo printf('[SimpleCC] running | root: %s | server: %s%s',
      s_root, g:simplecc_status, julia_env)
enddef

export def JuliaActivateEnvironment(path: string = '')
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif
  if &filetype !=# 'julia'
    echohl ErrorMsg
    echom '[SimpleCC] Julia environment activation requires a Julia buffer'
    echohl None
    return
  endif

  var environment = path ==# '' ? FindNearestJuliaEnvironment() : NormalizeJuliaEnvironment(path)
  if environment ==# ''
    var detail = path ==# ''
        ? 'No Project.toml or JuliaProject.toml found'
        : 'Not a Julia environment: ' .. path
    echohl ErrorMsg
    echom '[SimpleCC] ' .. detail
    echohl None
    return
  endif

  SendWithCb({
    type: 'julia/activateEnvironment',
    id: NextId(),
    languageId: 'julia',
    envPath: environment,
  }, OnJuliaEnvironmentActivation)
  echo '[SimpleCC] Activating Julia environment: ' .. environment
enddef

export def JuliaRefreshLanguageServer()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif
  if &filetype !=# 'julia'
    echohl ErrorMsg
    echom '[SimpleCC] Julia language server refresh requires a Julia buffer'
    echohl None
    return
  endif

  SendWithCb({
    type: 'julia/refreshLanguageServer',
    id: NextId(),
    languageId: 'julia',
  }, OnJuliaLanguageServerRefresh)
enddef

export def ReloadConfiguration()
  if !s_initialized
    echom '[SimpleCC] not initialized'
    return
  endif

  var configured = get(g:, 'simplecc_config_path', '')
  var config_path = configured ==# '' ? '' : fnamemodify(expand(configured), ':p')
  SendWithCb({
    type: 'workspace/reloadConfiguration',
    id: NextId(),
    configPath: config_path,
  }, OnConfigurationReload)
enddef

export def OpenConfig()
  var active = ActiveConfigPath()
  if active !=# ''
    execute 'edit ' .. fnameescape(active)
    return
  endif

  # Create new
  var root = FindProjectRoot()
  var project_config = root .. '/simplecc.json'
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
  RememberNavigationContext(id)
  Send({
    type: 'textDocument/implementation',
    id: id,
    uri: BufUri(),
    languageId: BufFt(),
    line: line('.') - 1,
    character: CursorUtf16(),
  })
enddef

def OnImplementation(ev: dict<any>)
  var context = TakeNavigationContext(ev)
  var locs = get(ev, 'locations', [])
  if empty(locs)
    echo 'No implementation found'
    return
  endif
  if len(locs) == 1
    JumpToLocation(locs[0], context)
  else
    LocationsToQuickfix(locs, 'Implementation', context)
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
  RememberNavigationContext(id)
  Send({
    type: 'textDocument/typeDefinition',
    id: id,
    uri: BufUri(),
    languageId: BufFt(),
    line: line('.') - 1,
    character: CursorUtf16(),
  })
enddef

def OnTypeDefinition(ev: dict<any>)
  var context = TakeNavigationContext(ev)
  var locs = get(ev, 'locations', [])
  if empty(locs)
    echo 'No type definition found'
    return
  endif
  if len(locs) == 1
    JumpToLocation(locs[0], context)
  else
    LocationsToQuickfix(locs, 'TypeDefinition', context)
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
      col: Utf16LineColumn(getline(get(s, 'line', 0) + 1),
          get(s, 'character', 0)),
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
    var fpath = UriToPath(uri)
    add(qf_items, {
      filename: fpath,
      lnum: get(s, 'line', 0) + 1,
      col: UriUtf16Column(uri, get(s, 'line', 0) + 1,
          get(s, 'character', 0)),
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
    character: CursorUtf16(),
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
    var el = get(h, 'end_line', 0) + 1
    var sc = Utf16LineColumn(getline(sl), get(h, 'character', 0))
    var ec = Utf16LineColumn(getline(el), get(h, 'end_character', 0))
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
  var info = getbufinfo(bnr)
  var line_count = empty(info) ? 0 : info[0].linecount
  for h in hints
    var lnum = get(h, 'line', 0) + 1
    var line_text = get(getbufline(bnr, lnum), 0, '')
    var col = Utf16LineColumn(line_text, get(h, 'character', 0))
    var label = get(h, 'label', '')
    var pad_l = get(h, 'padding_left', false)
    var pad_r = get(h, 'padding_right', false)
    var text = (pad_l ? ' ' : '') .. label .. (pad_r ? ' ' : '')
    if lnum > 0 && lnum <= line_count
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
    character: CursorUtf16(),
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
    character: CursorUtf16(),
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
    var fpath = UriToPath(uri)
    add(qf_items, {
      filename: fpath,
      lnum: get(item, 'line', 0) + 1,
      col: UriUtf16Column(uri, get(item, 'line', 0) + 1,
          get(item, 'character', 0)),
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
    var fpath = UriToPath(uri)
    add(qf_items, {
      filename: fpath,
      lnum: get(item, 'line', 0) + 1,
      col: UriUtf16Column(uri, get(item, 'line', 0) + 1,
          get(item, 'character', 0)),
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
      positions: [{line: line('.') - 1, character: CursorUtf16()}],
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
  var sc = Utf16LineColumn(getline(sl), get(r, 'character', 0))
  var el = get(r, 'end_line', 0) + 1
  var end_utf16 = get(r, 'end_character', 0)
  var ends_at_next_line_start = el > sl && end_utf16 == 0
  if ends_at_next_line_start
    el -= 1
  endif
  var effective_end = ends_at_next_line_start
      ? ByteOffsetToUtf16(getline(el), strlen(getline(el))) : end_utf16
  var ec = Utf16EndCursorColumn(getline(el), effective_end)
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
    var line_text = getline(lnum)
    var start_utf16 = get(t, 'start', 0)
    var end_utf16 = start_utf16 + get(t, 'length', 0)
    var start_byte = Utf16ToByteOffset(line_text, start_utf16)
    var end_byte = Utf16ToByteOffset(line_text, end_utf16)
    var col = start_byte + 1
    var length = max([0, end_byte - start_byte])
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
  var prop_types = [
    ['SimpleCCVirtualDiagErrorProp', 'SimpleCCVirtualDiagError'],
    ['SimpleCCVirtualDiagWarnProp', 'SimpleCCVirtualDiagWarn'],
  ]
  for [ptype, highlight] in prop_types
    try
      prop_type_add(ptype, {bufnr: bufnr, highlight: highlight})
    catch
    endtry
    try
      prop_remove({type: ptype, bufnr: bufnr, all: true})
    catch
    endtry
  endfor
  var max_per_line = max([0, get(g:, 'simplecc_diag_max_per_line', 3)])
  if !g:simplecc_virtual_diag || max_per_line == 0
    return
  endif
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
  var info = getbufinfo(bufnr)
  var line_count = empty(info) ? 0 : info[0].linecount
  for [key, diags] in items(line_diags)
    var lnum = str2nr(key)
    # Sort by severity (error first)
    sort(diags, (a, b) => get(a, 'severity', 3) - get(b, 'severity', 3))
    var shown = diags[: max_per_line - 1]
    var msgs: list<string> = []
    for d in shown
      var msg = substitute(get(d, 'message', ''), "\n", ' ', 'g')
      if strchars(msg) > 60
        msg = strcharpart(msg, 0, 57) .. '...'
      endif
      add(msgs, msg)
    endfor
    if len(diags) > max_per_line
      add(msgs, printf('+%d more', len(diags) - max_per_line))
    endif
    var best_sev = get(shown[0], 'severity', 3)
    var ptype = best_sev <= 1
        ? 'SimpleCCVirtualDiagErrorProp' : 'SimpleCCVirtualDiagWarnProp'
    if lnum > 0 && lnum <= line_count
      try
        prop_add(lnum, 0, {type: ptype, text: '  ' .. join(msgs, ' | '),
            text_align: 'after', bufnr: bufnr})
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
    var servers = ['rust-analyzer', 'clangd', 'pyright', 'typescript-language-server',
        'lua-language-server', 'gopls', 'julia-lsp']
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
    if has_key(s_declined_installs, server)
      remove(s_declined_installs, server)
    endif
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
    character: CursorUtf16(),
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
    character: CursorUtf16(),
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
    var fpath = UriToPath(uri)
    add(qf_items, {
      filename: fpath,
      lnum: get(item, 'line', 0) + 1,
      col: UriUtf16Column(uri, get(item, 'line', 0) + 1,
          get(item, 'character', 0)),
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
        var fpath = UriToPath(uri)
        if fpath !=# '' && filereadable(fpath)
          execute 'edit ' .. fnameescape(fpath)
          var lnum = get(item, 'line', 0) + 1
          cursor(lnum, Utf16LineColumn(getline(lnum), get(item, 'character', 0)))
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
    var fpath = UriToPath(detail)
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

def CompletionData(ud: dict<any>): dict<any>
  var data = copy(ud)
  var generation = get(ud, 'generation', 0)
  var item_index = get(ud, 'index', -1)
  if generation <= 0 || item_index < 0
    return data
  endif

  var key = printf('%d:%d', generation, item_index)
  if !has_key(s_comp_resolved_items, key)
    return data
  endif

  var resolved = s_comp_resolved_items[key]
  var is_snippet = get(resolved, 'is_snippet', get(data, 'is_snippet', false))
  data.is_snippet = is_snippet
  data.snippet_text = is_snippet
        ? get(resolved, 'insert_text', get(data, 'snippet_text', ''))
        : ''

  var text_edit = get(resolved, 'text_edit', {})
  if !empty(text_edit)
    data.text_edit = text_edit
  endif
  var additional_edits = get(resolved, 'additional_text_edits', [])
  if !empty(additional_edits)
    data.additional_text_edits = additional_edits
  endif
  var commit_characters = get(resolved, 'commit_characters', [])
  if !empty(commit_characters)
    data.commit_characters = commit_characters
  endif
  return data
enddef

def ApplyCompletionTextEdit(edit: dict<any>): bool
  if empty(edit) || s_comp_bufnr != bufnr('%') || s_comp_line <= 0
        || s_comp_original_line ==# ''
    return false
  endif

  # Completion text edits normally replace a range on the cursor line. Restore
  # the request snapshot first because Vim's complete() has already replaced
  # its guessed prefix. Multiline source ranges remain on the safe fallback path.
  var sl = get(edit, 'line', -1)
  var el = get(edit, 'end_line', sl)
  if sl != s_comp_line - 1 || el != sl
    Log('completion: unsupported multiline textEdit, using Vim insertion')
    return false
  endif

  var start_offset = get(edit, 'character', 0)
  var start_byte = Utf16ToByteOffset(s_comp_original_line, start_offset)
  var prefix = strpart(s_comp_original_line, 0, start_byte)
  var new_text = get(edit, 'new_text', '')
  var new_lines = split(new_text, "\n", true)
  if empty(new_lines)
    new_lines = ['']
  endif

  try
    undojoin
  catch
  endtry
  setline(s_comp_line, s_comp_original_line)
  ApplyTextEdits(bufnr('%'), [edit])

  if len(new_lines) == 1
    cursor(s_comp_line, strlen(prefix .. new_lines[0]) + 1)
  else
    cursor(s_comp_line + len(new_lines) - 1, strlen(new_lines[-1]) + 1)
  endif
  return true
enddef

def CompletionEditLineDelta(edits: list<dict<any>>, anchor_line: number): number
  var delta = 0
  for edit in edits
    var sl = get(edit, 'line', 0)
    var el = get(edit, 'end_line', sl)
    if el < anchor_line
      var new_text = get(edit, 'new_text', '')
      delta += count(new_text, "\n") - (el - sl)
    endif
  endfor
  return delta
enddef

def ApplyCompletionAdditionalEdits(edits: list<dict<any>>)
  if empty(edits) || s_comp_bufnr != bufnr('%') || s_comp_line <= 0
    return
  endif

  # Completion additionalTextEdits must not overlap the main completion edit.
  # Be defensive: apply edits strictly before or after the completion line and
  # skip malformed/overlapping server responses rather than corrupting text.
  var anchor_line = s_comp_line - 1
  var safe_edits: list<dict<any>> = []
  for edit in edits
    var sl = get(edit, 'line', 0)
    var el = get(edit, 'end_line', sl)
    if el < anchor_line || sl > anchor_line
      add(safe_edits, edit)
    else
      Log('completion: skipped overlapping additionalTextEdit: ' .. json_encode(edit))
    endif
  endfor
  if empty(safe_edits)
    return
  endif

  var old_lnum = line('.')
  var old_col = col('.')
  var line_delta = CompletionEditLineDelta(safe_edits, anchor_line)
  try
    undojoin
  catch
  endtry
  ApplyTextEdits(bufnr('%'), safe_edits)

  if line_delta != 0
    cursor(max([1, old_lnum + line_delta]), old_col)
    if !empty(s_snippet_tabstops)
      for i in range(len(s_snippet_tabstops) - 1)
        s_snippet_tabstops[i].lnum += line_delta
      endfor
    endif
  endif
enddef

export def OnCompleteDone()
  if !s_initialized
    return
  endif
  # Clear completion preview state
  s_comp_preview_start_line = 0
  s_comp_preview_start_col = 0
  s_comp_preview_orig_text = ''

  var ci = v:completed_item
  if empty(ci)
    return
  endif
  var ud = get(ci, 'user_data', {})
  if type(ud) != v:t_dict
    return
  endif

  # Resolve may add textEdit/additionalTextEdits/documentation after the menu
  # was built. Merge the resolved item before applying the accepted completion.
  var data = CompletionData(ud)
  var is_snippet = get(data, 'is_snippet', false)
  if !is_snippet
    ApplyCompletionTextEdit(get(data, 'text_edit', {}))
  endif

  if is_snippet
    var snippet_text = get(data, 'snippet_text', '')
    if snippet_text !=# ''
      ExpandSnippet(ci, snippet_text)
    endif
  endif

  ApplyCompletionAdditionalEdits(get(data, 'additional_text_edits', []))
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
                 'Project.toml', 'JuliaProject.toml',
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

def ActiveConfigPath(): string
  var configured = get(g:, 'simplecc_config_path', '')
  if configured !=# ''
    return fnamemodify(expand(configured), ':p')
  endif

  var root = s_root ==# '' ? FindProjectRoot() : s_root
  for path in [root .. '/simplecc.json', root .. '/.simplecc.json']
    if filereadable(path)
      return fnamemodify(path, ':p')
    endif
  endfor

  var global = expand('~/.config/simplecc/simplecc.json')
  return filereadable(global) ? fnamemodify(global, ':p') : ''
enddef

def IsActiveConfigBuffer(): bool
  var current = expand('%:p')
  var active = ActiveConfigPath()
  return current !=# '' && active !=# ''
      && fnamemodify(current, ':p') ==# active
enddef

def NormalizeJuliaEnvironment(path: string): string
  if path ==# ''
    return ''
  endif

  var candidate = fnamemodify(expand(path), ':p')
  if filereadable(candidate)
    var name = fnamemodify(candidate, ':t')
    if name !=# 'Project.toml' && name !=# 'JuliaProject.toml'
      return ''
    endif
    candidate = fnamemodify(candidate, ':h')
  endif
  if !isdirectory(candidate)
    return ''
  endif
  if !filereadable(candidate .. '/Project.toml')
        && !filereadable(candidate .. '/JuliaProject.toml')
    return ''
  endif

  candidate = simplify(candidate)
  return candidate ==# '/' ? candidate : substitute(candidate, '/$', '', '')
enddef

def FindNearestJuliaEnvironment(): string
  var dir = expand('%:p:h')
  if dir ==# ''
    dir = getcwd()
  endif

  var previous = ''
  while dir !=# previous
    var environment = NormalizeJuliaEnvironment(dir)
    if environment !=# ''
      return environment
    endif
    previous = dir
    dir = fnamemodify(dir, ':h')
  endwhile
  return ''
enddef
