# Imports-aware file rename and move

SimpleTree `r` and cut/paste, and the virtual SimpleRemote tree's `r` and
cut/paste, use `workspace/willRenameFiles` before changing a file or directory.
Only running language servers whose advertised static URI scheme, glob and
file/folder filters match the source receive the request. The successful move
is followed by `workspace/didRenameFiles` for interested servers.

When a server returns imports edits, a read-only diff opens first. Press `a`
to apply the rename and imports, or `q` to cancel. Closing the preview also
cancels. Imports are applied to buffers, remain modified, and are saved normally;
the preview never writes source files. Source buffers and affected imports must
be unmodified. The preview checks buffer contents, disk/remote contents, target
absence and connection identity again before moving. Changed contents, dirty
buffers, failed reads, server errors and timeouts cancel the transaction rather
than apply stale edits. A failed move does not change imports.

Remote files use SimpleRemote's asynchronous buffer reads and active connection
generation. Their contents are read from the remote agent, never a similarly
named local file. Source and imports buffers are retargeted by the tree's normal
move bookkeeping. If a transport disconnects or times out during the filesystem
operation, its outcome can be uncertain; the error asks the user to refresh the
tree before retrying and leaves imports unchanged.

With no running language server, the tree retains its ordinary safe rename.
An older SimpleCC daemon is detected immediately; the user can explicitly choose
"Rename only" (imports are not updated) or cancel and update the backend.
Existing overwrite confirmations stay on the tree's original filesystem path;
they are not presented as imports-aware refactors. Integrated cut moves require
an atomic filesystem rename; they refuse a cross-device copy fallback.

Current preview boundaries: static registrations, unversioned text edits,
regular text files, and disjoint edits from multiple servers. Versioned document
edits, additional create/delete/rename operations, overlapping edits, or two
servers editing the same file are rejected with a visible reason. Batch cut
moves preview one item at a time; if a later item needs to edit an imports buffer
already modified by an earlier move, save that buffer before retrying the
remaining clipboard items.

Protocol reference: https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#workspace_willRenameFiles
