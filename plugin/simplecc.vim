vim9script

# simplecc - Rust-powered LSP client for Vim
# Requires Vim 9.0+

if exists('g:loaded_simplecc')
  finish
endif
g:loaded_simplecc = 1

if v:version < 900
  echohl ErrorMsg
  echom '[SimpleCC] requires Vim 9.0+'
  echohl None
  finish
endif

# ─── Options ──────────────────────────────────────────────
g:simplecc_auto_start      = get(g:, 'simplecc_auto_start', 1)
g:simplecc_no_default_maps = get(g:, 'simplecc_no_default_maps', 0)
g:simplecc_config_path     = get(g:, 'simplecc_config_path', '')
g:simplecc_daemon_path     = get(g:, 'simplecc_daemon_path', '')
g:simplecc_auto_complete   = get(g:, 'simplecc_auto_complete', 1)
g:simplecc_change_delay    = get(g:, 'simplecc_change_delay', 120)
g:simplecc_complete_delay  = get(g:, 'simplecc_complete_delay', 80)
g:simplecc_complete_min_chars = get(g:, 'simplecc_complete_min_chars', 1)
g:simplecc_complete_max_items = get(g:, 'simplecc_complete_max_items', 100)
g:simplecc_complete_resolve_delay = get(g:, 'simplecc_complete_resolve_delay', 120)
g:simplecc_sign_error      = get(g:, 'simplecc_sign_error', 'E>')
g:simplecc_sign_warn       = get(g:, 'simplecc_sign_warn', 'W>')
g:simplecc_sign_info       = get(g:, 'simplecc_sign_info', 'I>')
g:simplecc_sign_hint       = get(g:, 'simplecc_sign_hint', 'H>')
g:simplecc_auto_install    = get(g:, 'simplecc_auto_install', 0)
g:simplecc_inlay_hints     = get(g:, 'simplecc_inlay_hints', 1)
g:simplecc_virtual_diag    = get(g:, 'simplecc_virtual_diag', 1)
g:simplecc_diag_max_per_line = get(g:, 'simplecc_diag_max_per_line', 3)
g:simplecc_diag_float      = get(g:, 'simplecc_diag_float', 0)
g:simplecc_diag_min_severity = get(g:, 'simplecc_diag_min_severity', 4)
g:simplecc_semantic_tokens = get(g:, 'simplecc_semantic_tokens', 0)
g:simplecc_semtok_priority = get(g:, 'simplecc_semtok_priority', 100)
g:simplecc_semtok_range_threshold = get(g:, 'simplecc_semtok_range_threshold', 5000)
g:simplecc_pull_diagnostics = get(g:, 'simplecc_pull_diagnostics', 0)
g:simplecc_status          = ''

# ─── Commands ─────────────────────────────────────────────
command! -nargs=0 SimpleCC              simplecc#Status()
command! -nargs=0 SimpleCCStart         simplecc#Start()
command! -nargs=0 SimpleCCStop          simplecc#Stop()
command! -nargs=0 SimpleCCRestart       simplecc#Restart()
command! -nargs=0 SimpleCCConfig        simplecc#OpenConfig()
command! -nargs=0 SimpleCCReloadConfig  simplecc#ReloadConfiguration()
command! -nargs=? -complete=dir SimpleCCJuliaActivate simplecc#JuliaActivateEnvironment(<q-args>)
command! -nargs=0 SimpleCCJuliaRefresh  simplecc#JuliaRefreshLanguageServer()
command! -nargs=0 SimpleCCHover         simplecc#Hover()
command! -nargs=0 SimpleCCDefinition    simplecc#Definition()
command! -nargs=0 SimpleCCReferences    simplecc#References()
command! -nargs=0 SimpleCCRename        simplecc#Rename()
command! -nargs=0 SimpleCCFormat        simplecc#Format()
command! -nargs=0 SimpleCCAction        simplecc#CodeAction()
command! -nargs=0 SimpleCCDiagnostics   simplecc#DiagList()
command! -nargs=0 SimpleCCNextDiag      simplecc#DiagNext()
command! -nargs=0 SimpleCCPrevDiag      simplecc#DiagPrev()
command! -nargs=0 SimpleCCSignatureHelp simplecc#SignatureHelp()
command! -nargs=0 SimpleCCLog           simplecc#ShowLog()
command! -nargs=0 SimpleCCImplementation  simplecc#Implementation()
command! -nargs=0 SimpleCCTypeDef       simplecc#TypeDefinition()
command! -nargs=0 SimpleCCOutline       simplecc#DocumentSymbol()
command! -nargs=? SimpleCCWorkspaceSymbol simplecc#WorkspaceSymbol(<q-args>)
command! -nargs=0 SimpleCCHighlight     simplecc#DocumentHighlight()
command! -nargs=0 SimpleCCHighlightClear simplecc#DocumentHighlightClear()
command! -nargs=0 SimpleCCInlayHints    simplecc#InlayHintsToggle()
command! -nargs=0 SimpleCCIncomingCalls simplecc#IncomingCalls()
command! -nargs=0 SimpleCCOutgoingCalls simplecc#OutgoingCalls()
command! -nargs=0 SimpleCCSelExpand     simplecc#SelectionExpand()
command! -nargs=0 SimpleCCSelShrink     simplecc#SelectionShrink()
command! -nargs=0 SimpleCCSemanticTokens simplecc#SemanticTokens()
command! -nargs=0 SimpleCCCodeLens      simplecc#CodeLens()
command! -nargs=0 SimpleCCFold          simplecc#FoldingRange()
command! -nargs=0 SimpleCCCodeLensRun  simplecc#CodeLensRun()
command! -nargs=0 SimpleCCPullDiag     simplecc#PullDiagnostics()
command! -nargs=0 SimpleCCSupertypes   simplecc#Supertypes()
command! -nargs=0 SimpleCCSubtypes     simplecc#Subtypes()
command! -nargs=0 SimpleCCWorkspaceSymbolLive simplecc#WorkspaceSymbolLive()
command! -nargs=? -complete=custom,SimpleCCInstallComplete SimpleCCInstall simplecc#InstallServer(<q-args>)
command! -nargs=0 SimpleCCServers       simplecc#ListServers()

def SimpleCCInstallComplete(arglead: string, cmdline: string, cursorpos: number): string
  return "rust-analyzer\nclangd\npyright\ntypescript-language-server\nlua-language-server\ngopls\njulia-lsp"
enddef

# ─── Keymaps ──────────────────────────────────────────────
# Stable <Plug> targets let users opt into individual actions without coupling
# their configuration to command names.
nnoremap <silent> <Plug>(simplecc-definition) <Cmd>SimpleCCDefinition<CR>
nnoremap <silent> <Plug>(simplecc-references) <Cmd>SimpleCCReferences<CR>
nnoremap <silent> <Plug>(simplecc-hover) <Cmd>SimpleCCHover<CR>
nnoremap <silent> <Plug>(simplecc-rename) <Cmd>SimpleCCRename<CR>
nnoremap <silent> <Plug>(simplecc-code-action) <Cmd>SimpleCCAction<CR>
nnoremap <silent> <Plug>(simplecc-format) <Cmd>SimpleCCFormat<CR>
nnoremap <silent> <Plug>(simplecc-prev-diagnostic) <Cmd>SimpleCCPrevDiag<CR>
nnoremap <silent> <Plug>(simplecc-next-diagnostic) <Cmd>SimpleCCNextDiag<CR>
nnoremap <silent> <Plug>(simplecc-implementation) <Cmd>SimpleCCImplementation<CR>
nnoremap <silent> <Plug>(simplecc-type-definition) <Cmd>SimpleCCTypeDef<CR>
nnoremap <silent> <Plug>(simplecc-outline) <Cmd>SimpleCCOutline<CR>
nnoremap <silent> <Plug>(simplecc-inlay-hints) <Cmd>SimpleCCInlayHints<CR>
inoremap <silent> <expr> <Plug>(simplecc-select-tab) simplecc#SelectTabKey()
inoremap <silent> <expr> <Plug>(simplecc-select-shift-tab) simplecc#SelectShiftTabKey()
inoremap <silent> <expr> <Plug>(simplecc-select-down) simplecc#SelectDownKey()
inoremap <silent> <expr> <Plug>(simplecc-select-up) simplecc#SelectUpKey()
inoremap <silent> <expr> <Plug>(simplecc-select-enter) simplecc#SelectEnterKey()

if !g:simplecc_no_default_maps
  if maparg('gd', 'n') ==# '' | nmap <silent> gd <Plug>(simplecc-definition) | endif
  if maparg('gr', 'n') ==# '' | nmap <silent> gr <Plug>(simplecc-references) | endif
  if maparg('K', 'n') ==# '' | nmap <silent> K <Plug>(simplecc-hover) | endif
  if maparg('<leader>rn', 'n') ==# '' | nmap <silent> <leader>rn <Plug>(simplecc-rename) | endif
  if maparg('<leader>ca', 'n') ==# '' | nmap <silent> <leader>ca <Plug>(simplecc-code-action) | endif
  if maparg('<leader>fm', 'n') ==# '' | nmap <silent> <leader>f <Plug>(simplecc-format) | endif
  if maparg('[d', 'n') ==# '' | nmap <silent> [d <Plug>(simplecc-prev-diagnostic) | endif
  if maparg(']d', 'n') ==# '' | nmap <silent> ]d <Plug>(simplecc-next-diagnostic) | endif
  if maparg('gi', 'n') ==# '' | nmap <silent> gi <Plug>(simplecc-implementation) | endif
  if maparg('gy', 'n') ==# '' | nmap <silent> gy <Plug>(simplecc-type-definition) | endif
  if maparg('<leader>o', 'n') ==# '' | nmap <silent> <leader>o <Plug>(simplecc-outline) | endif
  if maparg('<leader>ih', 'n') ==# '' | nmap <silent> <leader>ih <Plug>(simplecc-inlay-hints) | endif
  if maparg('<Tab>', 'i') ==# '' | imap <silent> <Tab> <Plug>(simplecc-select-tab) | endif
  if maparg('<S-Tab>', 'i') ==# '' | imap <silent> <S-Tab> <Plug>(simplecc-select-shift-tab) | endif
  if maparg('<Down>', 'i') ==# '' | imap <silent> <Down> <Plug>(simplecc-select-down) | endif
  if maparg('<Up>', 'i') ==# '' | imap <silent> <Up> <Plug>(simplecc-select-up) | endif
  if maparg('<CR>', 'i') ==# '' | imap <silent> <CR> <Plug>(simplecc-select-enter) | endif
endif

# ─── Signs ────────────────────────────────────────────────
def DefineSimpleCCSign(name: string, text: string, texthl: string, fallback: string)
  try
    sign_define(name, {text: text, texthl: texthl})
  catch
    sign_define(name, {text: fallback, texthl: texthl})
  endtry
enddef

DefineSimpleCCSign('SimpleCCError', g:simplecc_sign_error, 'ErrorMsg', 'E>')
DefineSimpleCCSign('SimpleCCWarn', g:simplecc_sign_warn, 'WarningMsg', 'W>')
DefineSimpleCCSign('SimpleCCInfo', g:simplecc_sign_info, 'Normal', 'I>')
DefineSimpleCCSign('SimpleCCHint', g:simplecc_sign_hint, 'Comment', 'H>')

# ─── Highlights ───────────────────────────────────────────
highlight default link SimpleCCErrorHL   SpellBad
highlight default link SimpleCCWarnHL    SpellCap
highlight default link SimpleCCInfoHL    SpellLocal
highlight default link SimpleCCHintHL    SpellRare
highlight default link SimpleCCFloatBorder FloatBorder
highlight default link SimpleCCPmenuSel  PmenuSel
highlight default SimpleCCInlayHint guifg=#7f848e ctermfg=245
highlight default link SimpleCCDocHighlightRead Search
highlight default link SimpleCCDocHighlightWrite IncSearch
highlight default link SimpleCCDocHighlightText CursorLine
highlight default link SimpleCCVirtualDiagError DiagnosticVirtualTextError
highlight default link SimpleCCVirtualDiagWarn  DiagnosticVirtualTextWarn
# Semantic token highlights
highlight default link SimpleCCSemanticNamespace Include
highlight default link SimpleCCSemanticType      Type
highlight default link SimpleCCSemanticClass     Type
highlight default link SimpleCCSemanticEnum      Type
highlight default link SimpleCCSemanticInterface Type
highlight default link SimpleCCSemanticStruct    Type
highlight default link SimpleCCSemanticTypeParameter Type
highlight default link SimpleCCSemanticParameter Identifier
highlight default link SimpleCCSemanticVariable  Identifier
highlight default link SimpleCCSemanticProperty  Identifier
highlight default link SimpleCCSemanticEnumMember Constant
highlight default link SimpleCCSemanticFunction  Function
highlight default link SimpleCCSemanticMethod    Function
highlight default link SimpleCCSemanticMacro     Macro
highlight default link SimpleCCSemanticKeyword   Keyword
highlight default link SimpleCCSemanticComment   Comment
highlight default link SimpleCCSemanticString    String
highlight default link SimpleCCSemanticNumber    Number
highlight default link SimpleCCSemanticOperator  Operator
highlight default link SimpleCCSemanticDecorator PreProc
# Modifier-only semantic token highlights
highlight default SimpleCCSemanticDeprecated gui=strikethrough cterm=strikethrough
highlight default SimpleCCSemanticReadonly gui=italic cterm=italic
highlight default SimpleCCSemanticStatic gui=bold cterm=bold
highlight default SimpleCCSemanticDefaultLibrary gui=italic cterm=italic
highlight default SimpleCCSemanticDeclaration gui=bold cterm=bold

# ─── Autocmds ─────────────────────────────────────────────
augroup simplecc
  autocmd!
  autocmd VimEnter * if g:simplecc_auto_start | simplecc#Start() | endif
  autocmd VimLeavePre * simplecc#Stop()
  autocmd BufNewFile,BufReadPost * simplecc#OnBufOpen()
  autocmd FileType * simplecc#OnBufOpen()
  autocmd BufEnter * simplecc#OnBufEnter()
  autocmd BufWritePost * simplecc#OnBufSave()
  autocmd BufUnload * simplecc#OnBufClose(str2nr(expand('<abuf>')))
  autocmd TextChanged,TextChangedI * simplecc#OnTextChanged()
  autocmd CursorMovedI * simplecc#OnCursorMovedI()
  autocmd InsertLeave * simplecc#OnInsertLeave()
  autocmd CompleteChanged * simplecc#OnCompleteChanged()
  autocmd CompleteDone * simplecc#OnCompleteDone()
  autocmd CursorHold * simplecc#OnCursorHold()
  autocmd WinScrolled * simplecc#OnWinScrolled()
  autocmd InsertCharPre * simplecc#OnInsertCharPre()
augroup END
