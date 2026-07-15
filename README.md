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
| <code>:SimpleCCLog</code> | Open the in-memory SimpleCC log |
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
| <code>:SimpleCCFormat</code> | Format the current buffer |
| <code>:SimpleCCAction</code> | Select a code action |
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
| <code>:SimpleCCDiagnostics</code> | Put diagnostics in the quickfix list |
| <code>:SimpleCCNextDiag</code> | Jump to the next diagnostic |
| <code>:SimpleCCPrevDiag</code> | Jump to the previous diagnostic |
| <code>:SimpleCCPullDiag</code> | Request pull diagnostics |
| <code>:SimpleCCJuliaActivate [dir]</code> | Activate a Julia environment |
| <code>:SimpleCCJuliaRefresh</code> | Refresh LanguageServer.jl caches |

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
| <code>&lt;leader&gt;ca</code> | Code action |
| <code>&lt;leader&gt;f</code> | Format |
| <code>&lt;leader&gt;o</code> | Document outline |
| <code>&lt;leader&gt;ih</code> | Toggle inlay hints |
| <code>[d</code> / <code>]d</code> | Previous / next diagnostic |
| Insert-mode Tab, Shift-Tab, arrows, Enter | Navigate and accept completion |

## Options

Set options before <code>plugin/simplecc.vim</code> is loaded.

| Option | Default | Purpose |
| --- | ---: | --- |
| <code>g:simplecc_auto_start</code> | 1 | Start on VimEnter |
| <code>g:simplecc_no_default_maps</code> | 0 | Disable built-in mappings |
| <code>g:simplecc_config_path</code> | empty | Explicit configuration path |
| <code>g:simplecc_daemon_path</code> | empty | Explicit daemon executable |
| <code>g:simplecc_auto_complete</code> | 1 | Enable automatic completion |
| <code>g:simplecc_change_delay</code> | 120 | Document-change debounce in ms |
| <code>g:simplecc_complete_delay</code> | 80 | Completion debounce in ms |
| <code>g:simplecc_complete_min_chars</code> | 1 | Minimum typed characters |
| <code>g:simplecc_complete_max_items</code> | 100 | Maximum completion items |
| <code>g:simplecc_complete_resolve_delay</code> | 120 | Resolve debounce in ms |
| <code>g:simplecc_sign_error</code> | E&gt; | Error sign text |
| <code>g:simplecc_sign_warn</code> | W&gt; | Warning sign text |
| <code>g:simplecc_sign_info</code> | I&gt; | Information sign text |
| <code>g:simplecc_sign_hint</code> | H&gt; | Hint sign text |
| <code>g:simplecc_auto_install</code> | 0 | Install missing managed servers without prompting |
| <code>g:simplecc_inlay_hints</code> | 1 | Enable inlay hints |
| <code>g:simplecc_virtual_diag</code> | 1 | Enable virtual diagnostic text |
| <code>g:simplecc_diag_max_per_line</code> | 3 | Virtual diagnostics per line |
| <code>g:simplecc_diag_float</code> | 0 | Show diagnostics near the cursor |
| <code>g:simplecc_diag_min_severity</code> | 4 | Include severities up to this value: 1 error, 4 hint |
| <code>g:simplecc_semantic_tokens</code> | 0 | Enable automatic semantic tokens |
| <code>g:simplecc_semtok_priority</code> | 100 | Semantic-token property priority |
| <code>g:simplecc_semtok_range_threshold</code> | 5000 | Use range requests above this line count |
| <code>g:simplecc_pull_diagnostics</code> | 0 | Enable pull diagnostics |
| <code>g:simplecc_status</code> | empty | Current statusline-friendly state |

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
