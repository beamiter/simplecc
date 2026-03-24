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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_text: Option<String>,
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
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceEdit {
    pub changes: Vec<FileEdit>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileEdit {
    pub uri: String,
    pub edits: Vec<TextEdit>,
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

/// Convert LSP Location to our simplified Location.
pub fn from_lsp_location(loc: &lsp_types::Location) -> Location {
    Location {
        uri: loc.uri.to_string(),
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

/// Convert LSP WorkspaceEdit to our WorkspaceEdit.
pub fn from_lsp_workspace_edit(edit: &lsp_types::WorkspaceEdit) -> WorkspaceEdit {
    let mut changes = Vec::new();
    if let Some(ref ch) = edit.changes {
        for (uri, edits) in ch {
            let file_edits: Vec<TextEdit> = edits
                .iter()
                .map(|e| TextEdit {
                    line: e.range.start.line,
                    character: e.range.start.character,
                    end_line: e.range.end.line,
                    end_character: e.range.end.character,
                    new_text: e.new_text.clone(),
                })
                .collect();
            changes.push(FileEdit {
                uri: uri.to_string(),
                edits: file_edits,
            });
        }
    }
    // Also handle documentChanges if present
    if let Some(ref doc_changes) = edit.document_changes {
        match doc_changes {
            lsp_types::DocumentChanges::Edits(edits) => {
                for edit in edits {
                    let file_edits: Vec<TextEdit> = edit
                        .edits
                        .iter()
                        .filter_map(|e| match e {
                            lsp_types::OneOf::Left(te) => Some(TextEdit {
                                line: te.range.start.line,
                                character: te.range.start.character,
                                end_line: te.range.end.line,
                                end_character: te.range.end.character,
                                new_text: te.new_text.clone(),
                            }),
                            lsp_types::OneOf::Right(ate) => Some(TextEdit {
                                line: ate.text_edit.range.start.line,
                                character: ate.text_edit.range.start.character,
                                end_line: ate.text_edit.range.end.line,
                                end_character: ate.text_edit.range.end.character,
                                new_text: ate.text_edit.new_text.clone(),
                            }),
                        })
                        .collect();
                    changes.push(FileEdit {
                        uri: edit.text_document.uri.to_string(),
                        edits: file_edits,
                    });
                }
            }
            lsp_types::DocumentChanges::Operations(ops) => {
                for op in ops {
                    if let lsp_types::DocumentChangeOperation::Edit(edit) = op {
                        let file_edits: Vec<TextEdit> = edit
                            .edits
                            .iter()
                            .filter_map(|e| match e {
                                lsp_types::OneOf::Left(te) => Some(TextEdit {
                                    line: te.range.start.line,
                                    character: te.range.start.character,
                                    end_line: te.range.end.line,
                                    end_character: te.range.end.character,
                                    new_text: te.new_text.clone(),
                                }),
                                lsp_types::OneOf::Right(ate) => Some(TextEdit {
                                    line: ate.text_edit.range.start.line,
                                    character: ate.text_edit.range.start.character,
                                    end_line: ate.text_edit.range.end.line,
                                    end_character: ate.text_edit.range.end.character,
                                    new_text: ate.text_edit.new_text.clone(),
                                }),
                            })
                            .collect();
                        changes.push(FileEdit {
                            uri: edit.text_document.uri.to_string(),
                            edits: file_edits,
                        });
                    }
                }
            }
        }
    }
    WorkspaceEdit { changes }
}
