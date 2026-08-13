# SimpleCC

SimpleCC is a Vim 9 Language Server Protocol client with a small Rust daemon
and a native Vim9 UI. The daemon owns language-server processes and JSON-RPC
traffic; Vim handles completion, diagnostics, navigation, edits, snippets,
inlay hints, semantic tokens, code lenses, and hierarchy views.

## Requirements

- Vim 9.0 or newer with Vim9 script, jobs, channels, popups, timers, and text
  properties. A recent Vim 9.1 build is recommended.
- A stable Rust toolchain with Cargo to build the daemon.
- Bash and standard Unix tools for <code>install.sh</code>.
- At least one language server, installed on <code>PATH</code> or through
  <code>:SimpleCCInstall</code>.

The managed language-server installer targets Linux and macOS on x86_64 and
aarch64. Other systems may still use a manually built daemon and servers
already available on <code>PATH</code>, but are not currently covered by CI.

## Installation

With vim-plug:

~~~vim
Plug 'beamiter/simplecc', { 'do': './install.sh' }
~~~

Run <code>:PlugInstall</code>, or rebuild an existing checkout:

~~~sh
cd ~/.vim/plugged/simplecc
./install.sh
~~~

For a manual package installation:

~~~sh
git clone https://github.com/beamiter/simplecc.git \
  ~/.vim/pack/plugins/start/simplecc
~/.vim/pack/plugins/start/simplecc/install.sh
~~~

The installer performs a reproducible <code>cargo build --release --locked</code>,
stages the daemon, verifies it, and atomically replaces
<code>lib/simplecc-daemon</code>. To keep the daemon elsewhere:

~~~vim
let g:simplecc_daemon_path = '/absolute/path/to/simplecc-daemon'
~~~

## Quick start

1. Install SimpleCC and one language server.
2. Open a supported source file. SimpleCC starts automatically by default.
3. Check the state with <code>:SimpleCC</code>.
4. Use <code>gd</code> for definition, <code>K</code> for hover, and
   <code>&lt;leader&gt;rn</code> for rename.

For example:

~~~vim
:SimpleCCInstall rust-analyzer
:SimpleCCRestart
~~~

If a server is already installed system-wide, no managed installation is
needed. SimpleCC resolves managed installations first and then searches
<code>PATH</code>.

## Configuration

Set an explicit configuration before the plugin loads:

~~~vim
let g:simplecc_config_path = expand('~/.config/simplecc/simplecc.json')
~~~

Without an explicit path, SimpleCC searches in this order:

1. <code>simplecc.json</code> in the detected project root.
2. <code>.simplecc.json</code> in the detected project root.
3. <code>~/.config/simplecc/simplecc.json</code>.
4. Built-in defaults when no file exists.

Open or create the active project configuration with
<code>:SimpleCCConfig</code>. <code>:SimpleCCReloadConfig</code> validates the
replacement file and hot-pushes <code>settings</code> to servers that are
already running. Changes to <code>command</code>, <code>args</code>,
<code>filetypes</code>, <code>rootPatterns</code>, <code>priority</code>, or
<code>initializationOptions</code> require <code>:SimpleCCRestart</code>.
Invalid JSON is reported and does not silently replace the running
configuration.

Minimal configuration:

~~~json
{
  "languageServers": {
    "rust-analyzer": {
      "command": "rust-analyzer",
      "args": [],
      "filetypes": ["rust"],
      "rootPatterns": ["Cargo.toml"],
      "priority": 100,
      "initializationOptions": {},
      "settings": {}
    }
  }
}
~~~

Each language-server entry supports:

- <code>command</code>: executable name or absolute path.
- <code>args</code>: command-line arguments.
- <code>filetypes</code>: Vim filetypes handled by the server.
- <code>rootPatterns</code>: project marker names.
- <code>priority</code>: optional integer used when multiple servers handle the
  same filetype. Higher values win; ties are ordered by server name so
  selection is stable.
- <code>initializationOptions</code>: value sent during LSP initialization.
- <code>settings</code>: values used for configuration notifications and
  server-initiated <code>workspace/configuration</code> requests.

See [simplecc.json.example](simplecc.json.example) for all built-in server
examples, including Julia settings.

## Supported languages

| Language | Default server | Managed install | External prerequisite |
| --- | --- | --- | --- |
| Rust | rust-analyzer | yes | none |
| C and C++ | clangd | yes | none |
| Python | pyright-langserver | yes | Node.js and npm |
| Go | gopls | yes | Go |
| Lua | lua-language-server | yes | none |
| Julia | LanguageServer.jl | yes | Julia |
| TypeScript and JavaScript | typescript-language-server | yes | Node.js and npm |

Install TypeScript support through SimpleCC:

~~~vim
:SimpleCCInstall typescript-language-server
~~~

List managed servers with <code>:SimpleCCServers</code>. Install one with
<code>:SimpleCCInstall {name}</code>. Managed installs may contact GitHub,
npm, the Go module proxy, or Julia package registries.

## SimpleRemote Python workspaces

SimpleCC automatically follows the active
[SimpleRemote](https://github.com/beamiter/simpleremote) workspace. Connecting
or disconnecting rebuilds the LSP workspace, starts the configured language
server through SSH or Docker, and maps both virtual buffers and SSHFS paths to
their real remote `file://` URIs. Completion, diagnostics, definitions,
references, rename, and imports therefore use the server's Python environment
instead of the local machine.

For the built-in Python configuration, `pyright-langserver` must be available
in the remote project `.venv/bin` or on the remote `PATH`. Put `simplecc.json`
in the remote project root when a different command or pyright settings are
needed. Set
`g:simplecc_remote_auto_restart = 0` to disable lifecycle synchronization.

Run `:SimpleCCPython` to discover project virtual environments, the active
venv or conda environment, every environment reported by conda, and system
Python. The picker shows the matching `pyright-langserver` or
`basedpyright-langserver` and marks the current selection. Choices are stored
per local project or per remote transport/target/root and immediately restart
SimpleCC. Use `:SimpleCCPython auto` to restore automatic project/PATH
behavior, or pass explicit interpreter and LSP paths for a custom setup.

## Commands

### Lifecycle and configuration

| Command | Action |
| --- | --- |
| <code>:SimpleCC</code> | Show daemon, project, and server status |
| <code>:SimpleCCStart</code> | Start and initialize SimpleCC |
| <code>:SimpleCCStop</code> | Shut down SimpleCC and its servers |
| <code>:SimpleCCRestart</code> | Restart the daemon |
| <code>:SimpleCCConfig</code> | Open or create the active configuration |
| <code>:SimpleCCReloadConfig</code> | Validate configuration and hot-reload server settings |
| <code>:SimpleCCPython [python] [lsp]</code> | Select and persist the project Python interpreter and LSP executable |
| <code>:SimpleCCLog</code> | Open the in-memory SimpleCC log |
| <code>:SimpleCCHealth</code> | Full report in a scratch buffer: environment, daemon age vs. plugin sources, config and server-command resolution, runtime, and why this buffer is or is not served |
| <code>:SimpleCCInstall [server]</code> | Install a managed language server |
| <code>:SimpleCCServers</code> | List managed server installation state |

### Navigation and inspection

| Command | Action |
| --- | --- |
| <code>:SimpleCCHover</code> | Show hover documentation |
| <code>:SimpleCCDefinition</code> | Go to definition |
| <code>:SimpleCCReferences</code> | List references |
| <code>:SimpleCCImplementation</code> | Go to implementation |
| <code>:SimpleCCTypeDef</code> | Go to type definition |
| <code>:SimpleCCOutline</code> | Show document symbols |
| <code>:SimpleCCWorkspaceSymbol [query]</code> | Search workspace symbols |
| <code>:SimpleCCWorkspaceSymbolLive</code> | Open live workspace-symbol search |
| <code>:SimpleCCHighlight</code> | Highlight references under the cursor |
| <code>:SimpleCCHighlightClear</code> | Clear document highlights |
| <code>:SimpleCCIncomingCalls</code> | Show incoming calls |
| <code>:SimpleCCOutgoingCalls</code> | Show outgoing calls |
| <code>:SimpleCCSupertypes</code> | Show supertypes |
| <code>:SimpleCCSubtypes</code> | Show subtypes |

### Editing and language features

| Command | Action |
| --- | --- |
| <code>:SimpleCCRename</code> | Rename the symbol under the cursor |
| <code>:SimpleCCFormat</code> | Format the current buffer, or a <code>:'&lt;,'&gt;</code> range |
| <code>:SimpleCCAction</code> | Select a code action at the cursor, or over a <code>:'&lt;,'&gt;</code> range |
| <code>:SimpleCCSignatureHelp</code> | Show signature help |
| <code>:SimpleCCInlayHints</code> | Toggle inlay hints |
| <code>:SimpleCCSelExpand</code> | Expand the current selection |
| <code>:SimpleCCSelShrink</code> | Shrink the current selection |
| <code>:SimpleCCSemanticTokens</code> | Refresh semantic tokens |
| <code>:SimpleCCCodeLens</code> | Display code lenses |
| <code>:SimpleCCCodeLensRun</code> | Execute a code lens |
| <code>:SimpleCCFold</code> | Apply server-provided folding ranges |

### Diagnostics and Julia

| Command | Action |
| --- | --- |
| <code>:SimpleCCDiagnostics[!] [severity]</code> | List current-buffer diagnostics; use `!` for the workspace and optionally filter one severity |
| <code>:SimpleCCDiag</code> | Show all visible diagnostics on the current line, including source and code |
| <code>:SimpleCCNextDiag [severity]</code> | Jump to the next diagnostic, optionally filtering one severity |
| <code>:SimpleCCPrevDiag [severity]</code> | Jump to the previous diagnostic, optionally filtering one severity |
| <code>:SimpleCCPullDiag</code> | Request pull diagnostics |
| <code>:SimpleCCJuliaActivate [dir]</code> | Activate a Julia environment |
| <code>:SimpleCCJuliaRefresh</code> | Refresh LanguageServer.jl caches |

`:SimpleCCDiagnostics` opens a location list owned by the current split.
`:SimpleCCDiagnostics!` gathers every diagnostic snapshot currently known to
SimpleCC into the global quickfix list. The optional severity is one of
`all`, `error`, `warning`, `info`, or `hint` and is an exact filter, for
example `:SimpleCCDiagnostics! error`. Both lists are sorted deterministically
by path and position. `[d` and `]d` follow
`g:simplecc_diag_min_severity`, including same-line diagnostics, and wrap in
position order.

The optional navigation severity is `all`, `error`, `warning`, `info`, or
`hint`. With no argument, navigation keeps obeying
`g:simplecc_diag_min_severity`; an explicit severity (or `all`) is a temporary
one-command filter and does not change signs, virtual text, or configuration.

`:SimpleCCDiag` is the explicit counterpart to `g:simplecc_diag_float`: it
works even when automatic diagnostic popups are disabled. Multiple diagnostics
are ordered by severity and position, multiline messages are preserved, and
the popup uses the same `g:simplecc_diag_min_severity` boundary as signs,
virtual text, and navigation. Map `<Plug>(simplecc-show-diagnostic)` if you
want a dedicated key.

## Default mappings

Set <code>let g:simplecc_no_default_maps = 1</code> before loading the plugin to
disable all default mappings.

| Mapping | Action |
| --- | --- |
| <code>gd</code> | Definition |
| <code>gr</code> | References |
| <code>K</code> | Hover |
| <code>gi</code> | Implementation |
| <code>gy</code> | Type definition |
| <code>&lt;leader&gt;rn</code> | Rename |
| <code>&lt;leader&gt;ca</code> | Code action (Normal and Visual; a selection unlocks the extract refactorings) |
| <code>&lt;leader&gt;fm</code> | Format (Normal and Visual; a selection uses range formatting) |
| <code>&lt;leader&gt;o</code> | Document outline |
| <code>&lt;leader&gt;ih</code> | Toggle inlay hints |
| <code>[d</code> / <code>]d</code> | Previous / next diagnostic |
| Insert-mode Tab, Shift-Tab, arrows, Enter | Navigate and accept completion |

Every command worth a key also has a <code>&lt;Plug&gt;</code> target, so
<code>g:simplecc_no_default_maps = 1</code> does not mean typing command names
in full -- see <code>:help simplecc-mappings</code> for the list.

### Vim's own extension points

In every buffer the daemon serves, SimpleCC also points three Vim options at the
language server, so the keys that work everywhere else in Vim keep working here:

| Option | Key | What it does |
| --- | --- | --- |
| <code>omnifunc</code> | <code>&lt;C-x&gt;&lt;C-o&gt;</code> | Completion from the server |
| <code>tagfunc</code> | <code>&lt;C-]&gt;</code>, <code>:tag</code>, <code>&lt;C-w&gt;}</code> | Jump to the definition and push the tag stack, so <code>&lt;C-t&gt;</code> comes back |
| <code>formatexpr</code> | <code>gq</code>, <code>gw</code> | Format the range through the server |

Each one falls back to what Vim would have done on its own -- keyword
completion, the tags file, internal formatting -- whenever it cannot do better:
no daemon, no server for the buffer, no such capability advertised, or a tag
request that is not the identifier under the cursor. Auto-formatting while
typing is always Vim's. Set <code>g:simplecc_native_options</code> to 0 to
leave the three options alone, or 2 to override a filetype plugin's choice.

## Options

Set options before <code>plugin/simplecc.vim</code> is loaded.

| Option | Default | Purpose |
| --- | ---: | --- |
| <code>g:simplecc_auto_start</code> | 1 | Start on VimEnter |
| <code>g:simplecc_remote_auto_restart</code> | 1 | Follow SimpleRemote connect/disconnect lifecycle |
| <code>g:simplecc_python_path</code> | empty | Default Python interpreter without a saved project selection |
| <code>g:simplecc_python_lsp_path</code> | empty | Default Python LSP executable without a saved project selection |
| <code>g:simplecc_python_state_file</code> | automatic | Per-project Python environment selection store |
| <code>g:simplecc_no_default_maps</code> | 0 | Disable built-in mappings |
| <code>g:simplecc_config_path</code> | empty | Explicit configuration path |
| <code>g:simplecc_daemon_path</code> | empty | Explicit daemon executable |
| <code>g:simplecc_auto_restart</code> | 1 | Restart a daemon that died unexpectedly, with backoff |
| <code>g:simplecc_max_restarts</code> | 5 | Crashes per minute before the crash-loop breaker trips |
| <code>g:simplecc_request_timeout</code> | 30000 | Reply timeout in ms for requests with a callback |
| <code>g:simplecc_resource_operations</code> | 1 | Apply the create/rename/delete steps of a workspace edit, not only its text edits |
| <code>g:simplecc_native_options</code> | 1 | Point <code>omnifunc</code>/<code>tagfunc</code>/<code>formatexpr</code> at the server: 0 never, 1 where unset, 2 always |
| <code>g:simplecc_tagfunc_timeout</code> | 1000 | Milliseconds <code>tagfunc</code> waits before falling back to the tags file |
| <code>g:simplecc_auto_complete</code> | 1 | Enable automatic completion |
| <code>g:simplecc_change_delay</code> | 120 | Document-change debounce in ms |
| <code>g:simplecc_complete_delay</code> | 80 | Completion debounce in ms |
| <code>g:simplecc_complete_min_chars</code> | 1 | Minimum typed characters |
| <code>g:simplecc_complete_max_items</code> | 100 | Maximum completion items |
| <code>g:simplecc_complete_resolve_delay</code> | 120 | Resolve debounce in ms |
| <code>g:simplecc_complete_sort</code> | 1 | Honour the server's <code>sortText</code>/<code>filterText</code>/<code>preselect</code> ranking hints |
| <code>g:simplecc_complete_buffer_words</code> | 1 | Supplement LSP with keyword matches from open buffers |
| <code>g:simplecc_complete_buffer_max_items</code> | 20 | Max buffer-word candidates per menu |
| <code>g:simplecc_sign_error</code> | E&gt; | Error sign text |
| <code>g:simplecc_sign_warn</code> | W&gt; | Warning sign text |
| <code>g:simplecc_sign_info</code> | I&gt; | Information sign text |
| <code>g:simplecc_sign_hint</code> | H&gt; | Hint sign text |
| <code>g:simplecc_auto_install</code> | 0 | Install missing managed servers without prompting |
| <code>g:simplecc_inlay_hints</code> | 1 | Enable inlay hints |
| <code>g:simplecc_inlay_margin</code> | 100 | Lines around the viewport that inlay hints are requested for |
| <code>g:simplecc_virtual_diag</code> | 1 | Enable virtual diagnostic text |
| <code>g:simplecc_diag_max_per_line</code> | 3 | Virtual diagnostics per line |
| <code>g:simplecc_diag_float</code> | 0 | Show diagnostics near the cursor |
| <code>g:simplecc_diag_min_severity</code> | 4 | Include severities up to this value in signs, virtual text, and diagnostic navigation: 1 error, 4 hint |
| <code>g:simplecc_diag_sources</code> | `[]` | Language servers whose diagnostics are shown; empty means all |
| <code>g:simplecc_semantic_tokens</code> | 0 | Enable automatic semantic tokens |
| <code>g:simplecc_semtok_priority</code> | 100 | Semantic-token property priority |
| <code>g:simplecc_semtok_range_threshold</code> | 5000 | Use range requests above this line count |
| <code>g:simplecc_pull_diagnostics</code> | 0 | Enable pull diagnostics |
| <code>g:simplecc_signature_help</code> | 1 | Auto-show signature help while typing call arguments |
| <code>g:simplecc_status</code> | empty | Current statusline-friendly state |

For statuslines, <code>simplecc#DiagCounts()</code> returns the current
buffer's diagnostic counts as <code>{error, warning, info, hint}</code>, and
the same dictionary is cached in <code>b:simplecc_diag_counts</code> whenever
diagnostics are displayed.

Set the <code>SIMPLECC_DEBUG</code> environment variable to make the daemon
log per-request details to stderr.

## Troubleshooting

### Daemon not found

Run <code>./install.sh</code>, confirm
<code>lib/simplecc-daemon</code> is executable, or set
<code>g:simplecc_daemon_path</code> to an absolute executable path.

### Language server not found

Check the executable directly, for example
<code>rust-analyzer --version</code>. Use <code>:SimpleCCInstall</code> for a
managed server, then <code>:SimpleCCRestart</code>. Inspect
<code>:SimpleCCLog</code> for startup errors.

### Language server crashed

A crashed server restarts automatically with bounded backoff (up to three
attempts per minute) while a matching buffer is loaded. If it keeps
stopping, inspect <code>:SimpleCCLog</code> and use
<code>:SimpleCCRestart</code> after fixing the cause.

### Configuration does not apply

Validate the JSON and save it. Use <code>:SimpleCCReloadConfig</code> for
<code>settings</code>-only changes; use <code>:SimpleCCRestart</code> for
server command, arguments, initialization, routing, root-pattern, or priority
changes. Check <code>:SimpleCCLog</code> if loading fails. An explicit
<code>g:simplecc_config_path</code> takes precedence over discovered files.

### No completion or navigation result

Confirm <code>:SimpleCC</code> reports a ready daemon, the current
<code>&filetype</code> appears in the server configuration, and the server
starts successfully. Use <code>:SimpleCCRestart</code> after changing server
executables.

### Julia server does not start

Julia itself must be on <code>PATH</code>. Run
<code>:SimpleCCInstall julia-lsp</code>, open a Julia buffer, and use
<code>:SimpleCCJuliaActivate</code> in the desired environment.

## Development

<code>Cargo.lock</code> is tracked because SimpleCC ships a binary. Use locked
commands so local and CI dependency resolution agree:

~~~sh
cargo fmt --all -- --check
cargo check --locked --all-targets
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
~~~

Compile the Vim9 script without starting the daemon:

~~~sh
vim -Nu NONE -n -i NONE -es \
  -c 'let g:simplecc_auto_start=0' \
  -c 'set rtp^=.' \
  -c 'runtime plugin/simplecc.vim' \
  -c 'source autoload/simplecc.vim' \
  -c 'defcompile' \
  -c 'helptags doc' \
  -c 'qa!'
~~~

Run the Vim-side regression suite (UTF-16 positions, file URIs, edits,
mappings, and daemon restart lifecycle):

~~~sh
vim -Nu NONE -n -i NONE -es -S test/vim9_smoke.vim
~~~

Run <code>:help simplecc</code> inside Vim for the concise reference.
