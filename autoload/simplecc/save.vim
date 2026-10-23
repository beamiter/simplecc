vim9script

var active: dict<any> = {}
var serial = 0
var outcome: dict<any> = {}

def PayloadEol(buffer: number): bool
  return !!(getbufvar(buffer, '&endofline')
    || (getbufvar(buffer, '&fixendofline') && !getbufvar(buffer, '&binary')))
enddef

def Guard(state: dict<any>): bool
  return get(active, 'serial', -1) == state.serial
    && bufnr() == state.buffer && win_getid() == state.window
    && bufname() ==# state.name && b:changedtick == state.tick
    && &modifiable && !&readonly && mode(1) !~# '^[iR]' && mode(1) !=# 'c'
    && !get(b:, 'vimrc_large_file', 0)
    && (&buftype ==# '' || &buftype ==# 'acwrite')
enddef

# One controller owns exactly one group and at most two timers (deadline and
# next-tick resume). Superseding a save must retire these before replacing active.
def ClearDeferred(state: dict<any>)
  for key in ['defer_timer', 'resume_timer']
    var timer = get(state, key, 0)
    if timer > 0
      timer_stop(timer)
      state[key] = 0
    endif
  endfor
  augroup SimpleCCDeferredSave
    autocmd!
  augroup END
enddef

def CancelDeferred(token: number)
  if get(active, 'serial', -1) == token
    Fail(active, 'document or focus changed while save was deferred')
  endif
enddef

def Fail(state: dict<any>, message: string)
  if get(active, 'serial', -1) == state.serial
    outcome = {buffer: state.buffer, serial: state.serial, status: 'error', message: message}
    ClearDeferred(active)
    active = {}
    echohl WarningMsg
    echom '[SimpleCC Save] ' .. message .. '; buffer remains unsaved'
    echohl None
  endif
enddef

def Step(state: dict<any>)
  if !Guard(state)
    Fail(state, 'document, focus or mode changed')
    return
  endif
  if empty(state.steps)
    active = {}
    var prior = get(b:, 'vimrc_remote_write_state', {})
    outcome = {buffer: state.buffer, serial: state.serial, status: 'writing', message: '',
      write_tick: b:changedtick, acwrite: &buftype ==# 'acwrite',
      saw_modified: &modified, remote: {}}
    try
      # Normal :write deliberately retains BufWriteCmd for remote acwrite
      # buffers, write errors and the normal BufWritePre/Post lifecycle.
      write
      outcome.write_tick = b:changedtick
      outcome.saw_modified = outcome.saw_modified || &modified
      var remote = get(b:, 'vimrc_remote_write_state', {})
      if type(remote) == v:t_dict && has_key(remote, 'serial')
        if get(remote, 'serial', 0) <= get(prior, 'serial', 0)
          outcome.status = 'error'
          outcome.message = 'remote write did not start a new acknowledged request'
        else
          outcome.remote = deepcopy(remote)
        endif
      elseif !&modified && (!outcome.acwrite || outcome.saw_modified)
        outcome.status = 'success'
      endif
    catch
      outcome.status = 'error'
      outcome.message = v:exception
      echohl ErrorMsg
      echom '[SimpleCC Save] write failed: ' .. v:exception
      echohl None
    endtry
    return
  endif
  var kind = remove(state.steps, 0)
  simplecc#SaveRequest(kind, (ev: dict<any>) => Reply(state, ev), state.timeout)
enddef

def Reply(state: dict<any>, ev: dict<any>)
  if !Guard(state)
    Fail(state, 'document, focus or mode changed')
    return
  endif
  if get(ev, 'type', '') ==# 'error'
    Fail(state, get(ev, 'message', 'request failed'))
    return
  endif
  var edits = get(ev, 'edits', [])
  var view = winsaveview()
  try
    if !empty(edits)
      if state.edited
        undojoin
      else
        # Start a new undo block rather than joining the user's last typing.
        &undolevels = &undolevels
      endif
      simplecc#ApplyTextEdits(state.buffer, edits)
      state.edited = true
      state.tick = b:changedtick
    endif
  catch
    Fail(state, 'could not apply edits: ' .. v:exception)
    return
  finally
    winrestview(view)
  endtry
  Step(state)
enddef

# Never rewrite text or dismiss completion while Insert/Replace owns it. The
# explicit save remains pending until InsertLeave, against the original tick.
def Resume(token: number)
  if get(active, 'serial', -1) != token || !get(active, 'deferred', false)
    return
  endif
  active.deferred = false
  ClearDeferred(active)
  active.resume_timer = timer_start(0, (_) => ResumeAfterLeave(token))
enddef

def ResumeAfterLeave(token: number)
  if get(active, 'serial', -1) == token
    active.resume_timer = 0
    Step(active)
  endif
enddef

def ExpireDeferred(token: number)
  if get(active, 'serial', -1) == token && get(active, 'deferred', false)
    Fail(active, 'save deferred until InsertLeave timed out')
  endif
enddef

# Explicit save command, never a BufWritePre hook. A newer invocation invalidates
# older callbacks; typing, switching windows or changing buffers cancels saving.
export def Start(options: dict<any> = {}): number
  ClearDeferred(active)
  serial += 1
  active = {}
  if !&modifiable || &readonly || get(b:, 'vimrc_large_file', 0)
      || index(['', 'acwrite'], &buftype) < 0 || bufname() ==# ''
    echohl WarningMsg
    echom '[SimpleCC Save] buffer is protected or has no file name'
    echohl None
    outcome = {buffer: bufnr(), serial: serial, status: 'error', message: 'protected buffer'}
    return serial
  endif
  var steps: list<string> = []
  if get(options, 'format', true)
    add(steps, 'formatting')
  endif
  if get(options, 'imports', false)
    add(steps, 'organizeImports')
  endif
  active = {serial: serial, buffer: bufnr(), window: win_getid(),
    name: bufname(), tick: b:changedtick, edited: false, steps: steps,
    deferred: false, defer_timer: 0, resume_timer: 0,
    timeout: max([100, min([30000, get(options, 'timeout_ms', 3000)])])}
  outcome = {buffer: bufnr(), serial: serial, status: 'pending', message: ''}
  if mode(1) =~# '^[iR]' || mode(1) =~# '^ni'
    active.deferred = true
    var token = serial
    active.defer_timer = timer_start(active.timeout, (_) => ExpireDeferred(token))
    # Embed the immutable serial in the autocmd; a later request cannot be
    # resumed accidentally by an older buffer's InsertLeave.
    augroup SimpleCCDeferredSave
      execute 'autocmd InsertLeave <buffer> ++once Resume(' .. serial .. ')'
      execute 'autocmd BufLeave,BufWipeout,WinLeave <buffer> ++once CancelDeferred(' .. serial .. ')'
    augroup END
    outcome.message = 'save deferred until InsertLeave'
  else
    Step(active)
  endif
  return serial
enddef

# The caller must retain both buffer and serial; a later save cannot satisfy an
# earlier build request. acwrite remains pending until its transport clears modified.
export def Status(buffer: number, token: number): dict<any>
  if get(outcome, 'serial', -1) != token || get(outcome, 'buffer', -1) != buffer
    return {status: 'error', message: 'save request superseded'}
  endif
  if outcome.status ==# 'writing'
    if !bufexists(buffer)
      outcome.status = 'error'
      outcome.message = 'save buffer no longer exists'
    elseif getbufvar(buffer, 'changedtick') != outcome.write_tick
      outcome.status = 'error'
      outcome.message = 'document changed while awaiting write acknowledgement'
    elseif !empty(outcome.remote)
      var remote = getbufvar(buffer, 'vimrc_remote_write_state', {})
      if get(remote, 'serial', -1) != outcome.remote.serial
          || get(remote, 'generation', -1) != outcome.remote.generation
          || get(remote, 'tick', -1) != outcome.remote.tick
          || (has_key(outcome.remote, 'final_eol') &&
            (!!get(remote, 'final_eol', false) != !!outcome.remote.final_eol
              || PayloadEol(buffer) != !!outcome.remote.final_eol))
          || (get(outcome.remote, 'path', '') != '' &&
            (get(remote, 'path', '') != outcome.remote.path
              || get(getbufvar(buffer, 'vimrc_remote', {}), 'path', '') != outcome.remote.path))
        outcome.status = 'error'
        outcome.message = 'remote write request superseded or connection changed'
      elseif index(['error', 'stale'], get(remote, 'status', '')) >= 0
        outcome.status = 'error'
        outcome.message = get(remote, 'message', 'remote write failed')
      elseif get(remote, 'status', '') ==# 'success'
        if !getbufvar(buffer, '&modified') && get(remote, 'tick', -1) == outcome.write_tick
          outcome.status = 'success'
        else
          outcome.status = 'error'
          outcome.message = 'remote acknowledgement does not match saved document'
        endif
      endif
    elseif !getbufvar(buffer, '&modified') && (!outcome.acwrite || outcome.saw_modified)
      outcome.status = 'success'
    endif
  endif
  return copy(outcome)
enddef
