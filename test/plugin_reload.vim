vim9script

# Sourcing plugin/simplecc.vim a second time must leave it working.
#
# A plugin manager sources plugin/ again when the vimrc is reloaded.  The script
# is guarded, so nothing is redefined -- but plain `vim9script` deletes every
# script-local function and variable before the guard reaches `finish`, and the
# commands, autocommands and g: functions defined the first time round go on
# referring to them.  Here that was the completion
# of :SimpleCCInstall, whose -complete=custom function is script-local.
#
# Run:  vim -Nu NONE -n -i NONE -es -S test/plugin_reload.vim

set nocompatible nomore
const ROOT = fnamemodify(resolve(expand('<sfile>:p')), ':h:h')
const SCRIPT = ROOT .. '/plugin/simplecc.vim'
const ERRORS = ROOT .. '/test/plugin-reload-errors.log'
execute 'set runtimepath^=' .. fnameescape(ROOT)
delete(ERRORS)

# What the script owns, as Vim sees it.
def ScriptItems(): dict<list<string>>
  for info in getscriptinfo()
    if resolve(fnamemodify(info.name, ':p')) ==# SCRIPT
      var detail = getscriptinfo({sid: info.sid})[0]
      return {
        functions: sort(copy(detail.functions)),
        variables: sort(keys(detail.variables)),
      }
    endif
  endfor
  return {functions: [], variables: []}
enddef

g:simplecc_auto_start = 0
g:simplecc_no_default_maps = 1
execute 'source ' .. fnameescape(SCRIPT)
var before = ScriptItems()
assert_true(!empty(before.functions),
  'the script defines no script-local function: this test checks nothing')

execute 'source ' .. fnameescape(SCRIPT)
assert_equal(before, ScriptItems(),
  'sourcing the script again deleted script-local items')

# :SimpleCCInstall completes through a script-local function.
var servers: list<string> = []
try
  servers = getcompletion('SimpleCCInstall ', 'cmdline')
catch
  assert_report(':SimpleCCInstall completion threw after a reload: ' .. v:exception)
endtry
assert_true(index(servers, 'pyright') >= 0,
  ':SimpleCCInstall completion lost its candidates after a reload: ' .. string(servers))

if !empty(v:errors)
  writefile(v:errors, ERRORS)
  cquit
endif
qa!
