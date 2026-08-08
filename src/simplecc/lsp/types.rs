use serde::{Deserialize, Serialize};

/// Simplified completion item sent to Vim.
#[derive(Debug, Clone, Serialize)]
pub struct CompletionItem {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub insert_text: Option<String>,
    /// Server-provided replacement range. Vim still uses `insert_text` for
    /// the popup menu, while retaining this metadata for precise application.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_edit: Option<TextEdit>,
    /// Extra edits associated with accepting the completion, most commonly
    /// import/include insertion.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_text_edits: Vec<TextEdit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commit_characters: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preselect: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_text: Option<String>,
    /// Index for completionItem/resolve
    pub index: usize,
    /// Whether this item uses snippet syntax
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_snippet: Option<bool>,
}

/// Location for definition / references.
#[derive(Debug, Clone, Serialize)]
pub struct Location {
    pub uri: String,
    pub line: u32,
    pub character: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_character: Option<u32>,
}

/// Diagnostic item.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticItem {
    pub line: u32,
    pub character: u32,
    pub end_line: u32,
    pub end_character: u32,
    pub severity: u8, // 1=error, 2=warn, 3=info, 4=hint
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// Text edit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEdit {
    pub line: u32,
    pub character: u32,
    pub end_line: u32,
    pub end_character: u32,
    pub new_text: String,
}

/// Code action.
#[derive(Debug, Clone, Serialize)]
pub struct CodeAction {
    pub title: String,
    pub kind: Option<String>,
    /// Index into the daemon's cached action list for execution.
    pub index: usize,
}

/// Signature help.
#[derive(Debug, Clone, Serialize)]
pub struct SignatureInfo {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_parameter: Option<u32>,
    pub parameters: Vec<ParameterInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ParameterInfo {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
}

/// Workspace edit for multi-file changes.
///
/// `changes` is the flat text-edit view every release so far has sent, kept so
/// that a Vim half older than this daemon still applies the text half of an
/// edit instead of choking on an unknown field.  `operations` is the ordered
/// superset that also carries the create/rename/delete steps.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceEdit {
    pub changes: Vec<FileEdit>,
    pub operations: Vec<WorkspaceOperation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileEdit {
    pub uri: String,
    pub edits: Vec<TextEdit>,
}

/// One entry of an LSP `documentChanges` array, in the order the server sent
/// it.
///
/// The order is load-bearing and cannot be recovered from `changes`: a
/// rename-file refactor edits a file and *then* moves it, so applying the move
/// first would write the edits to a path that no longer exists — and
/// TypeScript's "move to a new file" creates the target before filling it in.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WorkspaceOperation {
    Edit {
        uri: String,
        edits: Vec<TextEdit>,
    },
    Create {
        uri: String,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Rename {
        uri: String,
        new_uri: String,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Delete {
        uri: String,
        recursive: bool,
        ignore_if_not_exists: bool,
    },
}

/// Document symbol for outline view.
#[derive(Debug, Clone, Serialize)]
pub struct DocumentSymbolItem {
    pub name: String,
    pub kind: String,
    pub detail: Option<String>,
    pub line: u32,
    pub character: u32,
    pub end_line: u32,
    pub end_character: u32,
    pub children: Vec<DocumentSymbolItem>,
}

/// Document highlight (same symbol occurrences).
#[derive(Debug, Clone, Serialize)]
pub struct DocumentHighlightItem {
    pub line: u32,
    pub character: u32,
    pub end_line: u32,
    pub end_character: u32,
    pub kind: String, // "text", "read", "write"
}

/// Inlay hint.
#[derive(Debug, Clone, Serialize)]
pub struct InlayHintItem {
    pub line: u32,
    pub character: u32,
    pub label: String,
    pub kind: String, // "type", "parameter"
    pub padding_left: bool,
    pub padding_right: bool,
}

/// Call hierarchy item.
#[derive(Debug, Clone, Serialize)]
pub struct CallHierarchyItem {
    pub name: String,
    pub kind: String,
    pub uri: String,
    pub line: u32,
    pub character: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Call hierarchy call (incoming or outgoing).
#[derive(Debug, Clone, Serialize)]
pub struct CallHierarchyCall {
    pub item: CallHierarchyItem,
    pub from_ranges: Vec<RangeItem>,
}

/// Simple range.
#[derive(Debug, Clone, Serialize)]
pub struct RangeItem {
    pub line: u32,
    pub character: u32,
    pub end_line: u32,
    pub end_character: u32,
}

/// Selection range (nested).
#[derive(Debug, Clone, Serialize)]
pub struct SelectionRangeItem {
    pub line: u32,
    pub character: u32,
    pub end_line: u32,
    pub end_character: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Box<SelectionRangeItem>>,
}

/// Semantic token (decoded).
#[derive(Debug, Clone, Serialize)]
pub struct SemanticTokenItem {
    pub line: u32,
    pub start: u32,
    pub length: u32,
    pub token_type: String,
    pub modifiers: Vec<String>,
}

/// Code lens.
#[derive(Debug, Clone, Serialize)]
pub struct CodeLensItem {
    pub line: u32,
    pub character: u32,
    pub end_line: u32,
    pub end_character: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_title: Option<String>,
    /// Index for codeLens/execute
    pub index: usize,
}

/// Folding range.
#[derive(Debug, Clone, Serialize)]
pub struct FoldingRangeItem {
    pub start_line: u32,
    pub end_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// Linked editing range.
#[derive(Debug, Clone, Serialize)]
pub struct LinkedEditingRangeItem {
    pub ranges: Vec<RangeItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub word_pattern: Option<String>,
}

/// Result of textDocument/prepareRename.
#[derive(Debug, Clone, Serialize)]
pub struct PrepareRenameItem {
    pub line: u32,
    pub character: u32,
    pub end_line: u32,
    pub end_character: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// True when the server asked the client to derive the range itself.
    pub default_behavior: bool,
}

/// Convert LSP CompletionItemKind to string label.
pub fn completion_kind_label(kind: lsp_types::CompletionItemKind) -> &'static str {
    use lsp_types::CompletionItemKind;
    match kind {
        CompletionItemKind::TEXT => "Text",
        CompletionItemKind::METHOD => "Method",
        CompletionItemKind::FUNCTION => "Function",
        CompletionItemKind::CONSTRUCTOR => "Constructor",
        CompletionItemKind::FIELD => "Field",
        CompletionItemKind::VARIABLE => "Variable",
        CompletionItemKind::CLASS => "Class",
        CompletionItemKind::INTERFACE => "Interface",
        CompletionItemKind::MODULE => "Module",
        CompletionItemKind::PROPERTY => "Property",
        CompletionItemKind::UNIT => "Unit",
        CompletionItemKind::VALUE => "Value",
        CompletionItemKind::ENUM => "Enum",
        CompletionItemKind::KEYWORD => "Keyword",
        CompletionItemKind::SNIPPET => "Snippet",
        CompletionItemKind::COLOR => "Color",
        CompletionItemKind::FILE => "File",
        CompletionItemKind::REFERENCE => "Reference",
        CompletionItemKind::FOLDER => "Folder",
        CompletionItemKind::ENUM_MEMBER => "EnumMember",
        CompletionItemKind::CONSTANT => "Constant",
        CompletionItemKind::STRUCT => "Struct",
        CompletionItemKind::EVENT => "Event",
        CompletionItemKind::OPERATOR => "Operator",
        CompletionItemKind::TYPE_PARAMETER => "TypeParam",
        _ => "Unknown",
    }
}

/// Convert LSP SymbolKind to string label.
pub fn symbol_kind_label(kind: lsp_types::SymbolKind) -> &'static str {
    use lsp_types::SymbolKind;
    match kind {
        SymbolKind::FILE => "File",
        SymbolKind::MODULE => "Module",
        SymbolKind::NAMESPACE => "Namespace",
        SymbolKind::PACKAGE => "Package",
        SymbolKind::CLASS => "Class",
        SymbolKind::METHOD => "Method",
        SymbolKind::PROPERTY => "Property",
        SymbolKind::FIELD => "Field",
        SymbolKind::CONSTRUCTOR => "Constructor",
        SymbolKind::ENUM => "Enum",
        SymbolKind::INTERFACE => "Interface",
        SymbolKind::FUNCTION => "Function",
        SymbolKind::VARIABLE => "Variable",
        SymbolKind::CONSTANT => "Constant",
        SymbolKind::STRING => "String",
        SymbolKind::NUMBER => "Number",
        SymbolKind::BOOLEAN => "Boolean",
        SymbolKind::ARRAY => "Array",
        SymbolKind::OBJECT => "Object",
        SymbolKind::KEY => "Key",
        SymbolKind::NULL => "Null",
        SymbolKind::ENUM_MEMBER => "EnumMember",
        SymbolKind::STRUCT => "Struct",
        SymbolKind::EVENT => "Event",
        SymbolKind::OPERATOR => "Operator",
        SymbolKind::TYPE_PARAMETER => "TypeParam",
        _ => "Unknown",
    }
}

/// Convert lsp_types::DocumentHighlightKind to string.
pub fn highlight_kind_label(kind: Option<lsp_types::DocumentHighlightKind>) -> &'static str {
    match kind {
        Some(lsp_types::DocumentHighlightKind::READ) => "read",
        Some(lsp_types::DocumentHighlightKind::WRITE) => "write",
        _ => "text",
    }
}

/// Extract documentation string from LSP MarkupContent or plain string.
pub fn extract_doc(doc: &Option<lsp_types::Documentation>) -> Option<String> {
    match doc {
        Some(lsp_types::Documentation::String(s)) => Some(s.clone()),
        Some(lsp_types::Documentation::MarkupContent(mc)) => Some(mc.value.clone()),
        None => None,
    }
}

/// Convert an LSP text edit to the compact wire representation used by Vim.
pub fn from_lsp_text_edit(edit: &lsp_types::TextEdit) -> TextEdit {
    TextEdit {
        line: edit.range.start.line,
        character: edit.range.start.character,
        end_line: edit.range.end.line,
        end_character: edit.range.end.character,
        new_text: edit.new_text.clone(),
    }
}

/// Order completion items the way the server intended.
///
/// `sortText` is where rust-analyzer, gopls and tsserver put their relevance
/// ranking; the array order on the wire is not it (rust-analyzer emits roughly
/// alphabetically, so `abort()` outranks the field you are actually reaching
/// for). The ordering matters even more than the display suggests, because the
/// caller truncates to `max_items` right after: sorting afterwards would keep
/// the arbitrary first hundred and throw the relevant items away.
///
/// The sort is stable, so items sharing a `sortText` keep the server's own
/// order, and the label is only a tie-break for the servers that omit
/// `sortText` on some items but not others (the spec says to fall back to the
/// label in exactly that case).
pub fn rank_completion_items(items: &mut [lsp_types::CompletionItem]) {
    items.sort_by(|a, b| {
        let a_key = a.sort_text.as_deref().unwrap_or(a.label.as_str());
        let b_key = b.sort_text.as_deref().unwrap_or(b.label.as_str());
        a_key.cmp(b_key).then_with(|| a.label.cmp(&b.label))
    });
}

/// Normalize a full LSP completion item without dropping edit semantics.
pub fn from_lsp_completion_item(item: &lsp_types::CompletionItem, index: usize) -> CompletionItem {
    let text_edit = item.text_edit.as_ref().map(|edit| match edit {
        lsp_types::CompletionTextEdit::Edit(edit) => from_lsp_text_edit(edit),
        lsp_types::CompletionTextEdit::InsertAndReplace(edit) => TextEdit {
            // Vim has one replacement range. Prefer the server's replace range;
            // it is the correct range after the user has already typed a prefix.
            line: edit.replace.start.line,
            character: edit.replace.start.character,
            end_line: edit.replace.end.line,
            end_character: edit.replace.end.character,
            new_text: edit.new_text.clone(),
        },
    });
    let insert_text = item
        .insert_text
        .clone()
        .or_else(|| text_edit.as_ref().map(|edit| edit.new_text.clone()));
    let additional_text_edits = item
        .additional_text_edits
        .as_ref()
        .map(|edits| edits.iter().map(from_lsp_text_edit).collect())
        .unwrap_or_default();
    let is_snippet = item.insert_text_format == Some(lsp_types::InsertTextFormat::SNIPPET);

    CompletionItem {
        label: item.label.clone(),
        kind: item.kind.map(completion_kind_label).map(String::from),
        detail: item.detail.clone(),
        documentation: extract_doc(&item.documentation),
        insert_text,
        text_edit,
        additional_text_edits,
        commit_characters: item.commit_characters.clone().unwrap_or_default(),
        preselect: item.preselect,
        sort_text: item.sort_text.clone(),
        filter_text: item
            .filter_text
            .clone()
            .or_else(|| Some(item.label.clone())),
        index,
        is_snippet: if is_snippet { Some(true) } else { None },
    }
}

/// Convert LSP Location to our simplified Location.
pub fn from_lsp_location(loc: &lsp_types::Location) -> Location {
    Location {
        uri: decode_uri(&loc.uri.to_string()),
        line: loc.range.start.line,
        character: loc.range.start.character,
        end_line: Some(loc.range.end.line),
        end_character: Some(loc.range.end.character),
    }
}

/// Convert LSP DiagnosticSeverity to u8.
pub fn severity_to_u8(sev: Option<lsp_types::DiagnosticSeverity>) -> u8 {
    match sev {
        Some(lsp_types::DiagnosticSeverity::ERROR) => 1,
        Some(lsp_types::DiagnosticSeverity::WARNING) => 2,
        Some(lsp_types::DiagnosticSeverity::INFORMATION) => 3,
        Some(lsp_types::DiagnosticSeverity::HINT) => 4,
        _ => 3,
    }
}

/// Convert u8 severity back to the LSP enum.
pub fn severity_from_u8(sev: u8) -> Option<lsp_types::DiagnosticSeverity> {
    match sev {
        1 => Some(lsp_types::DiagnosticSeverity::ERROR),
        2 => Some(lsp_types::DiagnosticSeverity::WARNING),
        3 => Some(lsp_types::DiagnosticSeverity::INFORMATION),
        4 => Some(lsp_types::DiagnosticSeverity::HINT),
        _ => None,
    }
}

/// The editor's own view of a diagnostic: the flat shape this daemon
/// publishes as `DiagnosticItem`, handed straight back to us as
/// `context.diagnostics` when a code action is requested over a range.
#[derive(Deserialize)]
struct FlatDiagnostic {
    line: u32,
    character: u32,
    end_line: Option<u32>,
    end_character: Option<u32>,
    severity: Option<u8>,
    message: String,
    source: Option<String>,
    /// Servers publish either shape; `DiagnosticItem` stringifies numbers on
    /// the way out, so accept both on the way back in.
    code: Option<serde_json::Value>,
}

impl FlatDiagnostic {
    fn into_lsp(self) -> lsp_types::Diagnostic {
        let end_line = self.end_line.unwrap_or(self.line);
        let end_character = self.end_character.unwrap_or(self.character);
        lsp_types::Diagnostic {
            range: lsp_types::Range {
                start: lsp_types::Position {
                    line: self.line,
                    character: self.character,
                },
                end: lsp_types::Position {
                    line: end_line,
                    character: end_character,
                },
            },
            severity: self.severity.and_then(severity_from_u8),
            // A code that survived the round trip as digits was a number when
            // the server published it, and servers match on the original type.
            code: self.code.and_then(|c| match c {
                serde_json::Value::Number(n) => n
                    .as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .map(lsp_types::NumberOrString::Number),
                serde_json::Value::String(s) => Some(match s.parse::<i32>() {
                    Ok(n) => lsp_types::NumberOrString::Number(n),
                    Err(_) => lsp_types::NumberOrString::String(s),
                }),
                _ => None,
            }),
            source: self.source,
            message: self.message,
            ..Default::default()
        }
    }
}

/// Convert the `context.diagnostics` payload the editor sent into the LSP
/// shape a language server expects.
///
/// The editor holds diagnostics in the flat `DiagnosticItem` shape (`line`,
/// `character`, `end_line`, `end_character`), while `lsp_types::Diagnostic`
/// requires a nested, mandatory `range`. Feeding the former straight into
/// `serde_json::from_value::<Vec<lsp_types::Diagnostic>>` fails with
/// "missing field `range`", which is how every diagnostic-bound quickfix
/// action ("add the missing import", "remove this unused variable",
/// `#[allow]` insertion) used to be dropped before it reached the server.
///
/// Entries already in LSP shape are taken verbatim, so a caller holding a
/// server's own payload still works. Anything unreadable comes back as a
/// message rather than being swallowed, so the next shape mismatch is loud.
pub fn parse_context_diagnostics(
    value: &serde_json::Value,
) -> (Vec<lsp_types::Diagnostic>, Vec<String>) {
    let Some(entries) = value.as_array() else {
        return (Vec::new(), Vec::new());
    };
    let mut diagnostics = Vec::with_capacity(entries.len());
    let mut problems = Vec::new();
    for entry in entries {
        if entry.get("range").is_some() {
            match serde_json::from_value::<lsp_types::Diagnostic>(entry.clone()) {
                Ok(diagnostic) => diagnostics.push(diagnostic),
                Err(error) => problems.push(error.to_string()),
            }
            continue;
        }
        match serde_json::from_value::<FlatDiagnostic>(entry.clone()) {
            Ok(flat) => diagnostics.push(flat.into_lsp()),
            Err(error) => problems.push(error.to_string()),
        }
    }
    (diagnostics, problems)
}

/// Convert LSP WorkspaceEdit to our WorkspaceEdit.
///
/// Both views are produced in one pass: `changes` for the text edits alone and
/// `operations` for the same edits interleaved with the resource operations in
/// wire order.  A server may send `changes` *or* `documentChanges`; only the
/// latter can carry resource operations at all.
pub fn from_lsp_workspace_edit(edit: &lsp_types::WorkspaceEdit) -> WorkspaceEdit {
    let mut changes = Vec::new();
    let mut operations = Vec::new();

    if let Some(ref ch) = edit.changes {
        for (uri, edits) in ch {
            push_edit(
                &mut changes,
                &mut operations,
                uri.to_string(),
                edits.iter().map(text_edit).collect(),
            );
        }
    }
    // Also handle documentChanges if present
    if let Some(ref doc_changes) = edit.document_changes {
        match doc_changes {
            lsp_types::DocumentChanges::Edits(edits) => {
                for edit in edits {
                    push_edit(
                        &mut changes,
                        &mut operations,
                        edit.text_document.uri.to_string(),
                        edit.edits.iter().map(annotated_text_edit).collect(),
                    );
                }
            }
            lsp_types::DocumentChanges::Operations(ops) => {
                for op in ops {
                    match op {
                        lsp_types::DocumentChangeOperation::Edit(edit) => push_edit(
                            &mut changes,
                            &mut operations,
                            edit.text_document.uri.to_string(),
                            edit.edits.iter().map(annotated_text_edit).collect(),
                        ),
                        // Resource operations have no `changes` equivalent, so
                        // they only ever reach the editor through `operations`.
                        lsp_types::DocumentChangeOperation::Op(resource) => {
                            operations.push(resource_operation(resource))
                        }
                    }
                }
            }
        }
    }
    WorkspaceEdit {
        changes,
        operations,
    }
}

/// A text edit belongs in both views: the ordered `operations` list and the
/// flat `changes` list an older Vim half still reads.
fn push_edit(
    changes: &mut Vec<FileEdit>,
    operations: &mut Vec<WorkspaceOperation>,
    uri: String,
    edits: Vec<TextEdit>,
) {
    operations.push(WorkspaceOperation::Edit {
        uri: uri.clone(),
        edits: edits.clone(),
    });
    changes.push(FileEdit { uri, edits });
}

fn text_edit(edit: &lsp_types::TextEdit) -> TextEdit {
    TextEdit {
        line: edit.range.start.line,
        character: edit.range.start.character,
        end_line: edit.range.end.line,
        end_character: edit.range.end.character,
        new_text: edit.new_text.clone(),
    }
}

/// Annotated edits differ from plain ones only by a change-annotation id, which
/// exists to label a group of edits in a confirmation UI simplecc does not have.
fn annotated_text_edit(
    edit: &lsp_types::OneOf<lsp_types::TextEdit, lsp_types::AnnotatedTextEdit>,
) -> TextEdit {
    match edit {
        lsp_types::OneOf::Left(te) => text_edit(te),
        lsp_types::OneOf::Right(ate) => text_edit(&ate.text_edit),
    }
}

/// The LSP options are all tri-state; the absent case is the spec's default —
/// do not overwrite, do not ignore, do not recurse.
fn resource_operation(op: &lsp_types::ResourceOp) -> WorkspaceOperation {
    match op {
        lsp_types::ResourceOp::Create(create) => WorkspaceOperation::Create {
            uri: create.uri.to_string(),
            overwrite: create
                .options
                .as_ref()
                .and_then(|o| o.overwrite)
                .unwrap_or(false),
            ignore_if_exists: create
                .options
                .as_ref()
                .and_then(|o| o.ignore_if_exists)
                .unwrap_or(false),
        },
        lsp_types::ResourceOp::Rename(rename) => WorkspaceOperation::Rename {
            uri: rename.old_uri.to_string(),
            new_uri: rename.new_uri.to_string(),
            overwrite: rename
                .options
                .as_ref()
                .and_then(|o| o.overwrite)
                .unwrap_or(false),
            ignore_if_exists: rename
                .options
                .as_ref()
                .and_then(|o| o.ignore_if_exists)
                .unwrap_or(false),
        },
        lsp_types::ResourceOp::Delete(delete) => WorkspaceOperation::Delete {
            uri: delete.uri.to_string(),
            recursive: delete
                .options
                .as_ref()
                .and_then(|o| o.recursive)
                .unwrap_or(false),
            ignore_if_not_exists: delete
                .options
                .as_ref()
                .and_then(|o| o.ignore_if_not_exists)
                .unwrap_or(false),
        },
    }
}

/// Decode file:// URI to proper path.
pub(crate) fn decode_uri(uri: &str) -> String {
    url::Url::parse(uri)
        .ok()
        .filter(|url| url.scheme() == "file")
        .and_then(|url| url.to_file_path().ok())
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| uri.to_string())
}

#[cfg(test)]
mod tests {
    use super::{decode_uri, parse_context_diagnostics, rank_completion_items};
    use serde_json::json;

    fn item(label: &str, sort_text: Option<&str>) -> lsp_types::CompletionItem {
        lsp_types::CompletionItem {
            label: label.to_string(),
            sort_text: sort_text.map(String::from),
            ..Default::default()
        }
    }

    fn labels(items: &[lsp_types::CompletionItem]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }

    #[test]
    fn ranks_by_sort_text_before_the_caller_truncates() {
        // Wire order as rust-analyzer actually emits it: roughly alphabetical,
        // with relevance hidden in sortText.
        let mut items = vec![
            item("abort", Some("ffffffef")),
            item("zip", Some("ffffffff")),
            item("len", Some("ffffff00")),
        ];
        rank_completion_items(&mut items);
        assert_eq!(labels(&items), ["len", "abort", "zip"]);

        // The point of sorting first: a max_items cut keeps the relevant item.
        items.truncate(1);
        assert_eq!(labels(&items), ["len"]);
    }

    #[test]
    fn falls_back_to_the_label_and_keeps_the_server_order_on_ties() {
        let mut items = vec![
            item("beta", None),
            item("alpha", None),
            item("first", Some("alpha")),
            item("second", Some("alpha")),
        ];
        rank_completion_items(&mut items);
        // "alpha" (the label of one item, the sortText of two) sorts equal, so
        // the label breaks the tie and the two sortText-"alpha" items keep the
        // order the server sent them in.
        assert_eq!(labels(&items), ["alpha", "first", "second", "beta"]);
    }

    /// Verbatim payload of `RangeDiagnostics()` in autoload/simplecc.vim, as
    /// asserted on the wire by test/range_requests.vim.
    fn editor_payload() -> serde_json::Value {
        json!([{
            "line": 2, "character": 8, "end_line": 2, "end_character": 9,
            "severity": 2, "message": "unused variable: `y`",
            "source": "rust-analyzer", "code": "unused_variables"
        }])
    }

    #[test]
    fn converts_the_editors_flat_diagnostics_into_the_lsp_shape() {
        // The bug this guards: the flat shape has no `range`, which
        // lsp_types::Diagnostic requires, so the straight deserialization
        // fails ("missing field `range`") and every diagnostic-bound quickfix
        // action was dropped before the request left the daemon.
        let direct = serde_json::from_value::<Vec<lsp_types::Diagnostic>>(editor_payload());
        let error = direct
            .expect_err("the editor payload is not the LSP shape")
            .to_string();
        assert!(error.contains("missing field `range`"), "got: {error}");

        let (diagnostics, problems) = parse_context_diagnostics(&editor_payload());
        assert!(problems.is_empty(), "unexpected problems: {problems:?}");
        assert_eq!(diagnostics.len(), 1);
        let d = &diagnostics[0];
        assert_eq!(d.range.start, lsp_types::Position::new(2, 8));
        assert_eq!(d.range.end, lsp_types::Position::new(2, 9));
        assert_eq!(d.severity, Some(lsp_types::DiagnosticSeverity::WARNING));
        assert_eq!(d.message, "unused variable: `y`");
        assert_eq!(d.source.as_deref(), Some("rust-analyzer"));
        assert_eq!(
            d.code,
            Some(lsp_types::NumberOrString::String("unused_variables".into()))
        );

        // What reaches the server is the serialization of this, so assert the
        // wire form the way the server reads it.
        let wire = serde_json::to_value(d).unwrap();
        assert_eq!(wire["range"]["start"]["line"], 2);
        assert_eq!(wire["range"]["end"]["character"], 9);
    }

    #[test]
    fn keeps_numeric_codes_numeric_and_lsp_shaped_entries_verbatim() {
        // tsserver publishes numeric codes; DiagnosticItem stringifies them,
        // and a server matching on `code` needs the number back.
        let (diagnostics, problems) = parse_context_diagnostics(&json!([{
            "line": 0, "character": 0, "end_line": 0, "end_character": 4,
            "severity": 1, "message": "Type error", "code": "2345"
        }]));
        assert!(problems.is_empty());
        assert_eq!(
            diagnostics[0].code,
            Some(lsp_types::NumberOrString::Number(2345))
        );

        // An entry already in LSP shape survives untouched.
        let (diagnostics, problems) = parse_context_diagnostics(&json!([{
            "range": {"start": {"line": 1, "character": 2},
                      "end": {"line": 1, "character": 5}},
            "severity": 1, "message": "already lsp"
        }]));
        assert!(problems.is_empty());
        assert_eq!(diagnostics[0].range.end, lsp_types::Position::new(1, 5));
        assert_eq!(diagnostics[0].message, "already lsp");
    }

    #[test]
    fn reports_an_unreadable_entry_instead_of_dropping_it_silently() {
        let (diagnostics, problems) =
            parse_context_diagnostics(&json!([{"line": 1, "character": 0}, "nonsense"]));
        assert!(diagnostics.is_empty());
        assert_eq!(problems.len(), 2, "both entries must be reported");

        // A context that is not an array is simply absent, not an error.
        assert_eq!(parse_context_diagnostics(&json!(null)).1.len(), 0);
    }

    #[test]
    fn decodes_file_uris_without_losing_unicode_or_reserved_characters() {
        assert_eq!(
            decode_uri("file:///tmp/My%20Project/%E4%B8%AD%23%25.rs"),
            "/tmp/My Project/中#%.rs"
        );
        assert_eq!(
            decode_uri("https://example.com/a%20b"),
            "https://example.com/a%20b"
        );
    }
}
