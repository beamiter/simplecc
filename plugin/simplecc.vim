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
g:simplecc_complete_delay  = get(g:, 'simplecc_complete_delay', 100)
g:simplecc_sign_error      = get(g:, 'simplecc_sign_error', 'E>')
g:simplecc_sign_warn       = get(g:, 'simplecc_sign_warn', 'W>')
g:simplecc_sign_info       = get(g:, 'simplecc_sign_info', 'I>')
g:simplecc_sign_hint       = get(g:, 'simplecc_sign_hint', 'H>')
g:simplecc_auto_install    = get(g:, 'simplecc_auto_install', 0)
g:simplecc_status          = ''

# ─── Commands ─────────────────────────────────────────────
command! -nargs=0 SimpleCC              simplecc#Status()
command! -nargs=0 SimpleCCStart         simplecc#Start()
command! -nargs=0 SimpleCCStop          simplecc#Stop()
command! -nargs=0 SimpleCCRestart       simplecc#Restart()
command! -nargs=0 SimpleCCConfig        simplecc#OpenConfig()
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
command! -nargs=? -complete=custom,SimpleCCInstallComplete SimpleCCInstall simplecc#InstallServer(<q-args>)
command! -nargs=0 SimpleCCServers       simplecc#ListServers()

def SimpleCCInstallComplete(arglead: string, cmdline: string, cursorpos: number): string
  return "rust-analyzer\nclangd\npyright\nlua-language-server\ngopls"
enddef

# ─── Keymaps ──────────────────────────────────────────────
if !g:simplecc_no_default_maps
  nnoremap <silent> gd         :SimpleCCDefinition<CR>
  nnoremap <silent> gr         :SimpleCCReferences<CR>
  nnoremap <silent> K          :SimpleCCHover<CR>
  nnoremap <silent> <leader>rn :SimpleCCRename<CR>
  nnoremap <silent> <leader>ca :SimpleCCAction<CR>
  nnoremap <silent> <leader>f  :SimpleCCFormat<CR>
  nnoremap <silent> [d         :SimpleCCPrevDiag<CR>
  nnoremap <silent> ]d         :SimpleCCNextDiag<CR>
endif

# ─── Signs ────────────────────────────────────────────────
sign define SimpleCCError text=E> texthl=ErrorMsg linehl= numhl=
sign define SimpleCCWarn  text=W> texthl=WarningMsg linehl= numhl=
sign define SimpleCCInfo  text=I> texthl=Normal linehl= numhl=
sign define SimpleCCHint  text=H> texthl=Comment linehl= numhl=

execute 'sign define SimpleCCError text=' .. g:simplecc_sign_error .. ' texthl=ErrorMsg'
execute 'sign define SimpleCCWarn  text=' .. g:simplecc_sign_warn  .. ' texthl=WarningMsg'
execute 'sign define SimpleCCInfo  text=' .. g:simplecc_sign_info  .. ' texthl=Normal'
execute 'sign define SimpleCCHint  text=' .. g:simplecc_sign_hint  .. ' texthl=Comment'

# ─── Highlights ───────────────────────────────────────────
highlight default link SimpleCCErrorHL   SpellBad
highlight default link SimpleCCWarnHL    SpellCap
highlight default link SimpleCCInfoHL    SpellLocal
highlight default link SimpleCCHintHL    SpellRare
highlight default link SimpleCCFloatBorder FloatBorder
highlight default link SimpleCCPmenuSel  PmenuSel

# ─── Autocmds ─────────────────────────────────────────────
augroup simplecc
  autocmd!
  autocmd VimEnter * if g:simplecc_auto_start | simplecc#Start() | endif
  autocmd VimLeavePre * simplecc#Stop()
  autocmd BufReadPost * simplecc#OnBufOpen()
  autocmd BufWritePost * simplecc#OnBufSave()
  autocmd BufUnload * simplecc#OnBufClose()
  autocmd TextChanged,TextChangedI * simplecc#OnTextChanged()
  autocmd CursorMovedI * simplecc#OnCursorMovedI()
  autocmd InsertLeave * simplecc#OnInsertLeave()
  autocmd CompleteChanged * simplecc#OnCompleteChanged()
augroup END
