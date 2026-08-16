" Path <-> URI mapping for a SimpleRemote workspace.
"
" A remote language server only ever sees the host's own file:// URIs; Vim
" sees remote:// buffers (virtual mode) or a mounted projection (sshfs,
" docker-bind, local-map).  Both directions used to be untested, and one of
" them was wrong: a definition outside the workspace root -- the stdlib,
" site-packages, ~/.cargo/registry -- came back as a bare absolute path and
" `gd` opened that path on the *local* machine.
"
" SimpleRemote itself is not on the runtimepath: the workspace is simulated
" through g:simpleremote_workspace, exactly as SimpleRemote publishes it.
"
" Run:  vim -Nu NONE -n -i NONE -es -S test/remote_paths.vim

set nocompatible
set encoding=utf-8
set nomore
set noswapfile

let s:root = fnamemodify(expand('<sfile>'), ':p:h:h')
execute 'set runtimepath^=' .. fnameescape(s:root)
call delete(s:root .. '/test/remote-paths-errors.log')

let g:simplecc_auto_start = 0
let g:simplecc_no_default_maps = 1
runtime plugin/simplecc.vim
execute 'source ' .. fnameescape(s:root .. '/autoload/simplecc.vim')

let s:sid = getscriptinfo({'name': 'autoload/simplecc.vim'})[0].sid
function! s:Call(name, ...) abort
  return call(function(printf('<SNR>%d_%s', s:sid, a:name)), a:000)
endfunction

" ------------------------------------------------------------ disconnected ---

" Without a workspace every path is local, and a remote:// buffer name is
" still recognised as a remote path: buffers outlive the connection, and the
" URIs derived from them (didClose, the diagnostics store) must keep matching
" what was sent while connected.
unlet! g:simpleremote_workspace
unlet! g:vimrc_remote_workspace
call assert_equal({}, s:Call('RemoteWorkspace'))
call assert_equal('file:///tmp/local.py', simplecc#PathToUri('/tmp/local.py'))
call assert_equal('/tmp/local.py', simplecc#UriToPath('file:///tmp/local.py'))
call assert_equal('/srv/app/x.py', s:Call('RemotePath', 'remote:///srv/app/x.py'))
call assert_equal('file:///srv/app/x.py', simplecc#PathToUri('remote:///srv/app/x.py'),
      \ 'a remote:// name is a remote path even without a connection')

" ------------------------------------------------------------ virtual mode ---

let g:simpleremote_workspace = {'id': 3, 'kind': 'ssh', 'target': 'h',
      \ 'root': '/srv/app', 'tree_root': '/srv/app', 'local_root': '',
      \ 'mode': 'virtual', 'runtime': '', 'runtime_version': '',
      \ 'protocol': 'json', 'probe': {}, 'uri': 'remote:///srv/app'}
call assert_equal('/srv/app', s:Call('RemoteWorkspace').root)

" Vim -> server: the buffer name is the remote path with its scheme dropped.
call assert_equal('file:///srv/app/x.py', simplecc#PathToUri('remote:///srv/app/x.py'))
call assert_equal('file:///srv/app/dir%20with%20space/y.py',
      \ simplecc#PathToUri('remote:///srv/app/dir with space/y.py'),
      \ 'the remote path is percent-encoded like a local one')
call assert_equal('/srv/app/x.py', s:Call('RemotePath', 'remote:///srv/app/x.py'))
call assert_equal('', s:Call('RemotePath', '/tmp/local.py'),
      \ 'a local path is not part of a virtual workspace')

" Server -> Vim: inside the root, a remote:// buffer.
call assert_equal('remote:///srv/app/x.py', simplecc#UriToPath('file:///srv/app/x.py'))
call assert_equal('remote:///srv/app/dir with space/y.py',
      \ simplecc#UriToPath('file:///srv/app/dir%20with%20space/y.py'))
call assert_equal('remote:///srv/app', s:Call('LocalPath', '/srv/app', g:simpleremote_workspace),
      \ 'the root itself maps to the root')

" Server -> Vim: OUTSIDE the root the file still lives on the host.  It used
" to come back as a bare local path, and `gd` into the stdlib opened
" /usr/lib/python3.12/os.py on this machine.
call assert_equal('remote:///usr/lib/python3.12/os.py',
      \ simplecc#UriToPath('file:///usr/lib/python3.12/os.py'),
      \ 'an out-of-root definition opens as a remote buffer, not a local path')
call assert_equal('remote:///srv/application/z.py',
      \ simplecc#UriToPath('file:///srv/application/z.py'),
      \ 'a sibling whose name merely starts with the root is outside it')

" A local file that is open while connected keeps its local URI: nothing
" maps it, and the servers on the host are simply not told about it.
call assert_equal('file:///tmp/local.py', simplecc#PathToUri('/tmp/local.py'))

" A URI with nothing behind the scheme stays empty: callers tell a resolvable
" URI from an unresolvable one by exactly that, and 'remote://' would send a
" workspace edit at whichever buffer bufnr('') resolves to.
call assert_equal('', simplecc#UriToPath('file://'))
call assert_equal('', simplecc#UriToPath(''))
call assert_equal('already/a/path', simplecc#UriToPath('already/a/path'))

" ---------------------------------------------------------- projected mode ---

let s:mount = tempname()
call mkdir(s:mount .. '/pkg', 'p')
let g:simpleremote_workspace.mode = 'sshfs'
let g:simpleremote_workspace.local_root = s:mount

" Vim -> server: a file under the mount is spelled with the remote root.
call assert_equal('file:///srv/app/pkg/m.py', simplecc#PathToUri(s:mount .. '/pkg/m.py'))
call assert_equal('/srv/app/pkg/m.py', s:Call('RemotePath', s:mount .. '/pkg/m.py'))
call assert_equal('/srv/app', s:Call('RemotePath', s:mount),
      \ 'the mount point is the root')
call assert_equal('', s:Call('RemotePath', s:mount .. '-other/m.py'),
      \ 'a sibling of the mount is not under it')
call assert_equal('file:///srv/app/x.py', simplecc#PathToUri('remote:///srv/app/x.py'),
      \ 'a remote:// buffer maps the same way in every mode')
call assert_equal('file:///tmp/local.py', simplecc#PathToUri('/tmp/local.py'))

" Server -> Vim: inside the root, the projected local path ...
call assert_equal(s:mount .. '/pkg/m.py', simplecc#UriToPath('file:///srv/app/pkg/m.py'))
call assert_equal(s:mount, simplecc#UriToPath('file:///srv/app'))
" ... and outside it, where nothing is mounted, a remote:// buffer.
call assert_equal('remote:///usr/lib/python3.12/os.py',
      \ simplecc#UriToPath('file:///usr/lib/python3.12/os.py'),
      \ 'out-of-root files are not under the mount either')

" A trailing slash on either root does not change the mapping.
let g:simpleremote_workspace.root = '/srv/app/'
let g:simpleremote_workspace.local_root = s:mount .. '/'
call assert_equal('file:///srv/app/pkg/m.py', simplecc#PathToUri(s:mount .. '/pkg/m.py'))
call assert_equal(s:mount .. '/pkg/m.py', simplecc#UriToPath('file:///srv/app/pkg/m.py'))
let g:simpleremote_workspace.root = '/srv/app'
let g:simpleremote_workspace.local_root = s:mount

" ---------------------------------------------------- the workspace itself ---

" Only an ssh/docker workspace with a target and an absolute root counts;
" anything else is ignored rather than mapped half-way.
for s:bad in [{'kind': 'ftp', 'target': 'h', 'root': '/srv'},
      \ {'kind': 'ssh', 'target': '', 'root': '/srv'},
      \ {'kind': 'ssh', 'target': 'h', 'root': 'srv'},
      \ 'not a dict']
  let g:simpleremote_workspace = s:bad
  call assert_equal({}, s:Call('RemoteWorkspace'), 'rejected: ' .. string(s:bad))
endfor
unlet g:simpleremote_workspace

" The legacy g:vimrc_remote_workspace (published during SimpleRemote's
" handshake, before Connected) is honoured when the snapshot is absent.
let g:vimrc_remote_workspace = {'kind': 'docker', 'target': 'c1', 'root': '/work'}
call assert_equal('/work', s:Call('RemoteWorkspace').root)
call assert_equal('remote:///work/a.py', simplecc#UriToPath('file:///work/a.py'))
unlet g:vimrc_remote_workspace

" The remote configuration file and the buffers that hold it.
let g:simpleremote_workspace = {'id': 4, 'kind': 'ssh', 'target': 'h',
      \ 'root': '/srv/app', 'local_root': '', 'mode': 'virtual'}
call assert_equal('/srv/app/simplecc.json', s:Call('RemoteConfigPath', g:simpleremote_workspace))
call assert_equal('/simplecc.json', s:Call('RemoteConfigPath', {'root': '/'}))
enew!
call assert_equal(v:false, s:Call('IsRemoteConfigBuffer'))
let b:vimrc_remote = {'path': '/srv/app/simplecc.json', 'uri': 'remote:///srv/app/simplecc.json', 'generation': 4}
call assert_equal(v:true, s:Call('IsRemoteConfigBuffer'), 'the remote:// buffer for it')
unlet b:vimrc_remote
let b:simpleremote_path = '/srv/app/simplecc.json'
call assert_equal(v:true, s:Call('IsRemoteConfigBuffer'), 'the projected local file for it')
let b:simpleremote_path = '/srv/app/other.json'
call assert_equal(v:false, s:Call('IsRemoteConfigBuffer'))
unlet b:simpleremote_path

" BufUri() prefers the path SimpleRemote recorded over the buffer name: the
" name lags behind a rename made in the remote tree.
enew!
silent file remote:///srv/app/old-name.py
call assert_equal('file:///srv/app/old-name.py', s:Call('BufUri', bufnr('%')))
let b:vimrc_remote = {'path': '/srv/app/new-name.py', 'uri': 'remote:///srv/app/new-name.py', 'generation': 4}
call assert_equal('file:///srv/app/new-name.py', s:Call('BufUri', bufnr('%')),
      \ 'b:vimrc_remote.path is the truth about which remote file this is')

" ------------------------------------------------------- what may be sent ---

" A remote:// buffer is served only while connected, and only when it was
" filled by *this* connection.
setlocal buftype=acwrite
call assert_equal(v:true, s:Call('BufferServable', bufnr('%')))
let b:vimrc_remote.generation = 3
call assert_equal(v:false, s:Call('BufferServable', bufnr('%')),
      \ 'a buffer from a previous connection is not replayed to the new host')
let b:vimrc_remote.generation = 4
unlet g:simpleremote_workspace
call assert_equal(v:false, s:Call('BufferServable', bufnr('%')),
      \ 'a remote buffer is not sent to the local servers after a disconnect')
let g:simpleremote_workspace = {'id': 4, 'kind': 'ssh', 'target': 'h',
      \ 'root': '/srv/app', 'local_root': '', 'mode': 'virtual'}
call assert_equal(v:true, s:Call('BufferServable', bufnr('%')))
" A remote:// buffer whose read has not completed has no b:vimrc_remote yet.
enew!
silent file remote:///srv/app/unread.py
setlocal buftype=acwrite
call assert_equal(v:false, s:Call('BufferServable', bufnr('%')))
call assert_equal(v:true, s:Call('RemoteBufferUnread', bufnr('%')))
call assert_equal(v:false, s:Call('RemoteReadPending', bufnr('%')))
let b:vimrc_remote_read = {'request_id': 9, 'tick': 1}
call assert_equal(v:true, s:Call('RemoteReadPending', bufnr('%')))
call assert_equal(v:true, s:Call('RemoteBufferUnread', bufnr('%')))
let b:vimrc_remote_read = {}
let b:vimrc_remote = {'path': '/srv/app/unread.py', 'uri': 'remote:///srv/app/unread.py', 'generation': 4}
call assert_equal(v:false, s:Call('RemoteBufferUnread', bufnr('%')))
call assert_equal(v:true, s:Call('BufferServable', bufnr('%')))

" Special buffers never had a file; acwrite is what remote buffers are.
unlet g:simpleremote_workspace
enew!
silent file /tmp/simplecc-local.py
call assert_equal(v:true, s:Call('BufferServable', bufnr('%')))
setlocal buftype=nofile
call assert_equal(v:false, s:Call('BufferServable', bufnr('%')))
setlocal buftype=acwrite
call assert_equal(v:true, s:Call('BufferServable', bufnr('%')))
setlocal buftype=

" A purely local buffer is not offered to the servers of a connected
" workspace: they all run on the host, so its text would go to a server that
" cannot resolve it -- and every file:// URI that server answered with would
" be read as a path on the host, which turned `gd` inside ~/scratch.py into a
" jump to remote:///home/.../scratch.py.
let g:simpleremote_workspace = {'id': 4, 'kind': 'ssh', 'target': 'h',
      \ 'root': '/srv/app', 'local_root': '', 'mode': 'virtual'}
call assert_equal(v:false, s:Call('BufferServable', bufnr('%')),
      \ 'a local file is never sent to the servers running on the host')
unlet g:simpleremote_workspace
call assert_equal(v:true, s:Call('BufferServable', bufnr('%')),
      \ 'and is served again as soon as the servers are local ones')

" In a projected mode the mount is the workspace: a buffer under local_root
" is a file on the host too and is sent, its neighbours are not.
let g:simpleremote_workspace = {'id': 5, 'kind': 'ssh', 'target': 'h',
      \ 'root': '/srv/app', 'local_root': s:mount, 'mode': 'sshfs'}
execute 'silent file ' .. fnameescape(s:mount .. '/pkg/m.py')
call assert_equal(v:true, s:Call('BufferServable', bufnr('%')),
      \ 'the projection makes it the same file the servers see')
execute 'silent file ' .. fnameescape(s:mount .. '-other/m.py')
call assert_equal(v:false, s:Call('BufferServable', bufnr('%')),
      \ 'a sibling of the mount is not part of the workspace')
let g:simpleremote_workspace = {'id': 4, 'kind': 'ssh', 'target': 'h',
      \ 'root': '/srv/app', 'local_root': '', 'mode': 'virtual'}

" -------------------------------------------------------- python fallback ---

" With no :SimpleCCPython selection, a remote workspace takes the interpreter
" and language server the runtime probe found; locally nothing is guessed.
let g:simplecc_python_state_file = tempname()
let g:simplecc_python_path = ''
let g:simplecc_python_lsp_path = ''
call assert_equal({'python': '', 'lsp': ''}, s:Call('EffectivePythonSelection', {}))
call assert_equal({'python': '', 'lsp': ''},
      \ s:Call('EffectivePythonSelection', g:simpleremote_workspace),
      \ 'a probe that has not run yet leaves the choice to the daemon')
let g:simpleremote_workspace.probe = {'python': '/srv/app/.venv/bin/python3',
      \ 'python_lsp': '/srv/app/.venv/bin/pyright-langserver', 'runtime_ms': '12'}
call assert_equal({'python': '/srv/app/.venv/bin/python3',
      \ 'lsp': '/srv/app/.venv/bin/pyright-langserver'},
      \ s:Call('EffectivePythonSelection', g:simpleremote_workspace))
let g:simplecc_python_path = '/opt/py/bin/python'
call assert_equal({'python': '/opt/py/bin/python', 'lsp': ''},
      \ s:Call('EffectivePythonSelection', g:simpleremote_workspace),
      \ 'an explicit selection always wins over the probe')
let g:simplecc_python_path = ''

" A probe entry that is not a string is no answer at all: it used to be
" assigned into a string variable before the guard could look at it, which
" threw E1012 out of EffectivePythonSelection() -- and, since SendInitialize()
" calls it, would have left the daemon initialized for nothing.
let g:simpleremote_workspace.probe = {'python': 42, 'python_lsp': v:null,
      \ 'status': 0}
call assert_equal({'python': '', 'lsp': ''},
      \ s:Call('EffectivePythonSelection', g:simpleremote_workspace))

" A probe that is still in flight is seeded {status: -1} by SimpleRemote; it
" is not "not run" for the fallback either, it simply has no python in it.
let g:simpleremote_workspace.probe = {'status': -1}
call assert_equal({'python': '', 'lsp': ''},
      \ s:Call('EffectivePythonSelection', g:simpleremote_workspace))
call assert_equal(v:true, s:Call('ProbePending', {'status': -1}))
call assert_equal(v:true, s:Call('ProbePending', {}))
call assert_equal(v:false, s:Call('ProbePending', {'status': 0}),
      \ 'a probe that ran and found nothing is an answer')
call assert_equal(v:false, s:Call('ProbePending', {'status': -1, 'error': 'timed out'}),
      \ 'and so is one that failed')

call delete(s:mount, 'rf')

if len(v:errors)
  call writefile(v:errors, s:root .. '/test/remote-paths-errors.log')
  for s:e in v:errors
    echomsg s:e
  endfor
  cquit
endif
qall!
