from pathlib import Path
import re


def replace_once(path: str, old: str, new: str) -> None:
    file_path = Path(path)
    text = file_path.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{path}: expected one exact match, found {count}: {old[:80]!r}")
    file_path.write_text(text.replace(old, new, 1), encoding="utf-8")


def regex_once(path: str, pattern: str, replacement: str) -> None:
    file_path = Path(path)
    text = file_path.read_text(encoding="utf-8")
    updated, count = re.subn(pattern, replacement, text, count=1, flags=re.S)
    if count != 1:
        raise RuntimeError(f"{path}: expected one regex match, found {count}: {pattern[:80]!r}")
    file_path.write_text(updated, encoding="utf-8")


# ---------------------------------------------------------------------------
# Daemon request plumbing and completion context
# ---------------------------------------------------------------------------
replace_once(
    "src/simplecc/simplecc_daemon.rs",
    '''        #[serde(rename = "maxItems", default = "default_completion_max_items")]
        max_items: usize,
    },''',
    '''        #[serde(rename = "maxItems", default = "default_completion_max_items")]
        max_items: usize,
        #[serde(rename = "triggerKind", default = "default_completion_trigger_kind")]
        trigger_kind: u32,
        #[serde(rename = "triggerCharacter", default)]
        trigger_character: String,
    },''',
)

replace_once(
    "src/simplecc/simplecc_daemon.rs",
    '''fn default_completion_max_items() -> usize {
    100
}
''',
    '''fn default_completion_max_items() -> usize {
    100
}
fn default_completion_trigger_kind() -> u32 {
    1
}
''',
)

replace_once(
    "src/simplecc/simplecc_daemon.rs",
    '''            line,
            character,
            max_items,
        } => {''',
    '''            line,
            character,
            max_items,
            trigger_kind,
            trigger_character,
        } => {''',
)

replace_once(
    "src/simplecc/simplecc_daemon.rs",
    '''                match c.completion(&uri, line, character, max_items).await {''',
    '''                let trigger_character = if trigger_character.is_empty() {
                    None
                } else {
                    Some(trigger_character.as_str())
                };
                match c
                    .completion(
                        &uri,
                        line,
                        character,
                        max_items,
                        trigger_kind,
                        trigger_character,
                    )
                    .await
                {''',
)

# ---------------------------------------------------------------------------
# LSP client: validated CompletionContext
# ---------------------------------------------------------------------------
replace_once(
    "src/simplecc/lsp/client.rs",
    '''        character: u32,
        max_items: usize,
    ) -> Result<(u64, Vec<types::CompletionItem>)> {''',
    '''        character: u32,
        max_items: usize,
        trigger_kind: u32,
        trigger_character: Option<&str>,
    ) -> Result<(u64, Vec<types::CompletionItem>)> {''',
)

replace_once(
    "src/simplecc/lsp/client.rs",
    '''        let generation = self.completion_generation.fetch_add(1, Ordering::SeqCst) + 1;

        let result = self''',
    '''        let generation = self.completion_generation.fetch_add(1, Ordering::SeqCst) + 1;

        // Vim can cheaply infer punctuation-triggered requests, but only the
        // server knows which trigger characters it advertised. Downgrade an
        // unsupported trigger-character request to TriggerForIncompleteCompletions
        // so servers never receive an invalid CompletionContext.
        let mut effective_trigger_kind = trigger_kind.clamp(1, 3);
        let mut effective_trigger_character = trigger_character
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if effective_trigger_kind == 2 {
            let supported = match effective_trigger_character.as_deref() {
                Some(trigger) => {
                    let capabilities = self.capabilities.lock().await;
                    capabilities
                        .as_ref()
                        .and_then(|caps| caps.completion_provider.as_ref())
                        .and_then(|options| options.trigger_characters.as_ref())
                        .map(|characters| characters.iter().any(|value| value == trigger))
                        .unwrap_or(false)
                }
                None => false,
            };
            if !supported {
                effective_trigger_kind = 3;
                effective_trigger_character = None;
            }
        } else {
            effective_trigger_character = None;
        }

        let mut context = json!({
            "triggerKind": effective_trigger_kind,
        });
        if let Some(trigger) = effective_trigger_character {
            context["triggerCharacter"] = json!(trigger);
        }

        let result = self''',
)

replace_once(
    "src/simplecc/lsp/client.rs",
    '''                    "position": { "line": line, "character": character },
                }),''',
    '''                    "position": { "line": line, "character": character },
                    "context": context,
                }),''',
)

# ---------------------------------------------------------------------------
# Vim completion session state
# ---------------------------------------------------------------------------
replace_once(
    "autoload/simplecc.vim",
    '''var s_comp_start_col: number = 0
# Completion item resolve debounce / stale-response protection
var s_comp_resolve_timer: number = 0
var s_comp_resolve_id: number = 0
var s_comp_resolve_key: string = ''
var s_comp_resolved: dict<bool> = {}
''',
    '''var s_comp_start_col: number = 0
var s_comp_original_line: string = ''
# Completion item resolve debounce / stale-response protection
var s_comp_resolve_timer: number = 0
var s_comp_resolve_id: number = 0
var s_comp_resolve_key: string = ''
var s_comp_resolve_request_key: string = ''
var s_comp_resolve_requested: dict<bool> = {}
var s_comp_resolved_items: dict<dict<any>> = {}
''',
)

replace_once(
    "autoload/simplecc.vim",
    '''  s_comp_resolve_id = 0
  s_comp_resolve_key = ''
  CloseSignaturePopup()
''',
    '''  s_comp_resolve_id = 0
  s_comp_resolve_key = ''
  s_comp_resolve_request_key = ''
  s_comp_resolve_requested = {}
  s_comp_resolved_items = {}
  s_comp_original_line = ''
  CloseSignaturePopup()
''',
)

replace_once(
    "autoload/simplecc.vim",
    '''  s_comp_resolve_id = resolve_id
  s_comp_resolved[key] = true
''',
    '''  s_comp_resolve_id = resolve_id
  s_comp_resolve_request_key = key
  s_comp_resolve_requested[key] = true
''',
)

replace_once(
    "autoload/simplecc.vim",
    '''export def OnCompleteChanged()
''',
    '''def ShowCompletionDocumentation(item: dict<any>)
  var detail = get(item, 'detail', '')
  var doc = get(item, 'documentation', '')
  var text = detail
  if doc !=# ''
    text = text !=# '' ? text .. "\\n\\n" .. doc : doc
  endif

  if s_hover_popup > 0
    popup_close(s_hover_popup)
    s_hover_popup = 0
  endif
  if text ==# '' || !pumvisible()
    return
  endif

  s_hover_popup = popup_create(split(text, "\\n"), {
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
''',
)

replace_once(
    "autoload/simplecc.vim",
    '''      if has_key(s_comp_resolved, key)
        return
      endif
''',
    '''      if has_key(s_comp_resolved_items, key)
        ShowCompletionDocumentation(s_comp_resolved_items[key])
        return
      endif
      if get(s_comp_resolve_requested, key, false)
        return
      endif
''',
)

regex_once(
    "autoload/simplecc.vim",
    r'''def OnCompletionResolve\(ev: dict<any>\)\n.*?\nenddef\n\ndef TriggerCompletion\(\)''',
    '''def OnCompletionResolve(ev: dict<any>)
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

def TriggerCompletion()''',
)

replace_once(
    "autoload/simplecc.vim",
    '''    RequestCompletion()
  })
enddef
''',
    '''    RequestCompletion(false)
  })
enddef
''',
)

replace_once(
    "autoload/simplecc.vim",
    '''  RequestCompletion()
enddef

export def SelectTabKey''',
    '''  RequestCompletion(true)
enddef

export def SelectTabKey''',
)

replace_once(
    "autoload/simplecc.vim",
    '''def RequestCompletion()
''',
    '''def RequestCompletion(manual: bool = false)
''',
)

replace_once(
    "autoload/simplecc.vim",
    '''  if strchars(prefix) < g:simplecc_complete_min_chars && !is_trigger
    return
  endif

  # Queue the latest buffer text''',
    '''  if strchars(prefix) < g:simplecc_complete_min_chars && !is_trigger
    return
  endif
  var trigger_character = !manual && is_trigger ? line_text[start - 1] : ''
  var trigger_kind = manual
        ? 1
        : (trigger_character !=# '' ? 2 : 3)

  # Queue the latest buffer text''',
)

replace_once(
    "autoload/simplecc.vim",
    '''  s_comp_start_col = start
  s_comp_resolve_id = 0
  s_comp_resolve_key = ''
  s_comp_resolved = {}
''',
    '''  s_comp_start_col = start
  s_comp_original_line = line_text
  s_comp_resolve_id = 0
  s_comp_resolve_key = ''
  s_comp_resolve_request_key = ''
  s_comp_resolve_requested = {}
  s_comp_resolved_items = {}
''',
)

replace_once(
    "autoload/simplecc.vim",
    '''    character: cchar,
    maxItems: max_items,
  })
''',
    '''    character: cchar,
    maxItems: max_items,
    triggerKind: trigger_kind,
    triggerCharacter: trigger_character,
  })
''',
)

# A failed resolve should be retryable when the item is selected again.
replace_once(
    "autoload/simplecc.vim",
    '''  elseif ev.type ==# 'error'
    Log('error(id=' .. string(id) .. '): ' .. get(ev, 'message', ''))
''',
    '''  elseif ev.type ==# 'error'
    if id == s_comp_resolve_id
      if s_comp_resolve_request_key !=# ''
            && has_key(s_comp_resolve_requested, s_comp_resolve_request_key)
        remove(s_comp_resolve_requested, s_comp_resolve_request_key)
      endif
      s_comp_resolve_id = 0
      s_comp_resolve_request_key = ''
    endif
    Log('error(id=' .. string(id) .. '): ' .. get(ev, 'message', ''))
''',
)

helpers = '''def CompletionData(ud: dict<any>): dict<any>
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
  var start_chars = Utf16ToCharOffset(s_comp_original_line, start_offset)
  var prefix = strcharpart(s_comp_original_line, 0, start_chars)
  var new_text = get(edit, 'new_text', '')
  var new_lines = split(new_text, "\\n", true)
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

'''
replace_once(
    "autoload/simplecc.vim",
    '''def CompletionEditLineDelta(edits: list<dict<any>>, anchor_line: number): number
''',
    helpers + '''def CompletionEditLineDelta(edits: list<dict<any>>, anchor_line: number): number
''',
)

replace_once(
    "autoload/simplecc.vim",
    '''  var is_snippet = get(ud, 'is_snippet', false)
  if is_snippet
    var snippet_text = get(ud, 'snippet_text', '')
    if snippet_text !=# ''
      ExpandSnippet(ci, snippet_text)
    endif
  endif

  ApplyCompletionAdditionalEdits(get(ud, 'additional_text_edits', []))
''',
    '''  # Resolve may add textEdit/additionalTextEdits/documentation after the menu
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
''',
)

print("Applied completion v4 source transforms successfully")
