//! Requests for inlay hints, signature help, symbols and semantic tokens.

use std::path::{Path, PathBuf};

use lsp_types::{
    DocumentSymbol, DocumentSymbolResponse, InlayHint as LspInlayHint, InlayHintLabel, Location,
    OneOf, ParameterLabel, Position, Range, SemanticToken as Packed, SemanticTokensResult,
    SignatureHelp, SymbolKind, WorkspaceSymbolResponse,
};
use serde_json::{Value, json};

use crate::{
    client::{Client, LspError},
    convert,
    features::documentation_text,
};

/// A short note a server wants shown next to the code, like a type or a parameter name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlayHint {
    /// Where the note belongs.
    pub position: Position,
    /// The note text.
    pub label: String,
}

/// The signature of the call around the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    /// The whole signature, like `fn add(a: i32, b: i32) -> i32`.
    pub label: String,
    /// The chars of `label` that name the parameter being typed, as `(from, to)`.
    pub active: Option<(usize, usize)>,
    /// What the function does, if the server says.
    pub documentation: Option<String>,
}

/// A named thing in the code, like a function or a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// The symbol name.
    pub name: String,
    /// Extra detail like a signature or the containing type.
    pub detail: String,
    /// What kind of thing it is.
    pub kind: SymbolKind,
    /// How deep it is nested inside other symbols, 0 at the top.
    pub depth: usize,
    /// The file it is in.
    pub path: PathBuf,
    /// Where its name starts.
    pub position: Position,
}

/// A run of text the server classified, like a parameter or a macro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticToken {
    /// The line it is on.
    pub line: u32,
    /// The UTF-16 column it starts at.
    pub start: u32,
    /// Its length in UTF-16 units.
    pub length: u32,
    /// The token type name from the server legend, like `function`.
    pub kind: String,
    /// The token modifier names from the server legend, like `readonly`.
    pub modifiers: Vec<String>,
}

/// Returns a short name for `kind`, like `fn` or `struct`.
pub fn kind_name(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::FILE => "file",
        SymbolKind::MODULE | SymbolKind::NAMESPACE | SymbolKind::PACKAGE => "mod",
        SymbolKind::CLASS => "class",
        SymbolKind::METHOD => "method",
        SymbolKind::PROPERTY | SymbolKind::FIELD => "field",
        SymbolKind::CONSTRUCTOR => "new",
        SymbolKind::ENUM => "enum",
        SymbolKind::INTERFACE => "trait",
        SymbolKind::FUNCTION => "fn",
        SymbolKind::VARIABLE => "let",
        SymbolKind::CONSTANT => "const",
        SymbolKind::STRING => "str",
        SymbolKind::NUMBER => "num",
        SymbolKind::BOOLEAN => "bool",
        SymbolKind::ARRAY => "array",
        SymbolKind::OBJECT => "object",
        SymbolKind::KEY => "key",
        SymbolKind::NULL => "null",
        SymbolKind::ENUM_MEMBER => "variant",
        SymbolKind::STRUCT => "struct",
        SymbolKind::EVENT => "event",
        SymbolKind::OPERATOR => "op",
        SymbolKind::TYPE_PARAMETER => "type",
        _ => "symbol",
    }
}

/// Returns the text of an inlay hint label.
fn label_text(label: InlayHintLabel) -> String {
    match label {
        InlayHintLabel::String(text) => text,
        InlayHintLabel::LabelParts(parts) => parts.into_iter().map(|part| part.value).collect(),
    }
}

/// Converts a UTF-16 offset into `text` to a char offset.
fn utf16_to_char(text: &str, units: usize) -> usize {
    let mut seen = 0;
    for (index, ch) in text.chars().enumerate() {
        if seen >= units {
            return index;
        }
        seen += ch.len_utf16();
    }
    text.chars().count()
}

/// Picks the active signature out of a signature help answer.
fn pick_signature(help: SignatureHelp) -> Option<Signature> {
    let index = help.active_signature.unwrap_or(0) as usize;
    let signature = help
        .signatures
        .get(index)
        .or_else(|| help.signatures.first())?
        .clone();
    let active_parameter = signature.active_parameter.or(help.active_parameter);
    let parameter = active_parameter
        .and_then(|active| signature.parameters.as_ref()?.get(active as usize).cloned());
    let active = parameter.and_then(|parameter| match parameter.label {
        ParameterLabel::Simple(name) => {
            let byte = signature.label.find(&name)?;
            let from = signature.label[..byte].chars().count();
            Some((from, from + name.chars().count()))
        }
        ParameterLabel::LabelOffsets([from, to]) => Some((
            utf16_to_char(&signature.label, from as usize),
            utf16_to_char(&signature.label, to as usize),
        )),
    });
    Some(Signature {
        documentation: signature
            .documentation
            .as_ref()
            .map(|doc| documentation_text(doc).to_owned())
            .filter(|doc| !doc.trim().is_empty()),
        label: signature.label,
        active,
    })
}

/// Flattens nested document symbols into a list in reading order, tracking depth.
fn flatten(symbols: Vec<DocumentSymbol>, path: &Path, depth: usize, out: &mut Vec<Symbol>) {
    for symbol in symbols {
        out.push(Symbol {
            name: symbol.name,
            detail: symbol.detail.unwrap_or_default(),
            kind: symbol.kind,
            depth,
            path: path.to_owned(),
            position: symbol.selection_range.start,
        });
        if let Some(children) = symbol.children {
            flatten(children, path, depth + 1, out);
        }
    }
}

/// Builds a symbol from a flat symbol `location`.
fn located(
    name: String,
    kind: SymbolKind,
    container: Option<String>,
    location: &Location,
) -> Option<Symbol> {
    Some(Symbol {
        name,
        detail: container.unwrap_or_default(),
        kind,
        depth: 0,
        path: convert::uri_to_path(&location.uri)?,
        position: location.range.start,
    })
}

/// Decodes the packed token `data` with the token type and modifier names from `legend`.
fn decode_tokens(data: &[Packed], legend: &Value) -> Vec<SemanticToken> {
    let names = |key: &str| -> Vec<String> {
        legend[key]
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .map(|name| name.as_str().unwrap_or_default().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    let (types, modifiers) = (names("tokenTypes"), names("tokenModifiers"));
    let (mut line, mut start) = (0, 0);
    let mut tokens = Vec::with_capacity(data.len());
    for token in data {
        if token.delta_line > 0 {
            line += token.delta_line;
            start = token.delta_start;
        } else {
            start += token.delta_start;
        }
        let Some(kind) = types.get(token.token_type as usize) else {
            continue;
        };
        let modifiers = modifiers
            .iter()
            .enumerate()
            .filter(|(bit, _)| *bit < 32 && token.token_modifiers_bitset & (1 << bit) != 0)
            .map(|(_, name)| name.clone())
            .collect();
        tokens.push(SemanticToken {
            line,
            start,
            length: token.length,
            kind: kind.clone(),
            modifiers,
        });
    }
    tokens
}

impl Client {
    /// Returns whether the server said it can answer `provider`, like `inlayHintProvider`.
    pub fn supports(&self, provider: &str) -> bool {
        self.capabilities()
            .is_some_and(|caps| !caps[provider].is_null() && caps[provider] != json!(false))
    }

    /// Asks for inlay hints in `range` of `path`.
    ///
    /// # Errors
    ///
    /// Returns an error if the server is gone or fails the request.
    pub async fn inlay_hints(&self, path: &Path, range: Range) -> Result<Vec<InlayHint>, LspError> {
        let Some(uri) = convert::path_to_uri(path) else {
            return Ok(Vec::new());
        };
        let params = json!({ "textDocument": { "uri": uri }, "range": range });
        let value = self.request("textDocument/inlayHint", params).await?;
        let hints = serde_json::from_value::<Option<Vec<LspInlayHint>>>(value)
            .ok()
            .flatten()
            .unwrap_or_default();
        Ok(hints
            .into_iter()
            .map(|hint| {
                let mut label = label_text(hint.label);
                if hint.padding_left == Some(true) {
                    label.insert(0, ' ');
                }
                if hint.padding_right == Some(true) {
                    label.push(' ');
                }
                InlayHint {
                    position: hint.position,
                    label,
                }
            })
            .collect())
    }

    /// Asks for the signature of the call around `position` in `path`.
    ///
    /// # Errors
    ///
    /// Returns an error if the server is gone or fails the request.
    pub async fn signature_help(
        &self,
        path: &Path,
        position: Position,
    ) -> Result<Option<Signature>, LspError> {
        let Some(uri) = convert::path_to_uri(path) else {
            return Ok(None);
        };
        let params = json!({ "textDocument": { "uri": uri }, "position": position });
        let value = self.request("textDocument/signatureHelp", params).await?;
        Ok(serde_json::from_value::<Option<SignatureHelp>>(value)
            .ok()
            .flatten()
            .and_then(pick_signature))
    }

    /// Asks for every symbol in `path`, nested ones after their parent.
    ///
    /// # Errors
    ///
    /// Returns an error if the server is gone or fails the request.
    pub async fn document_symbols(&self, path: &Path) -> Result<Vec<Symbol>, LspError> {
        let Some(uri) = convert::path_to_uri(path) else {
            return Ok(Vec::new());
        };
        let params = json!({ "textDocument": { "uri": uri } });
        let value = self.request("textDocument/documentSymbol", params).await?;
        let mut symbols = Vec::new();
        match serde_json::from_value::<Option<DocumentSymbolResponse>>(value) {
            Ok(Some(DocumentSymbolResponse::Nested(nested))) => {
                flatten(nested, path, 0, &mut symbols);
            }
            Ok(Some(DocumentSymbolResponse::Flat(flat))) => {
                symbols.extend(flat.into_iter().filter_map(|symbol| {
                    located(
                        symbol.name,
                        symbol.kind,
                        symbol.container_name,
                        &symbol.location,
                    )
                }));
                symbols.sort_by_key(|symbol| (symbol.position.line, symbol.position.character));
            }
            _ => {}
        }
        Ok(symbols)
    }

    /// Searches the whole project for symbols matching `query`.
    ///
    /// # Errors
    ///
    /// Returns an error if the server is gone or fails the request.
    pub async fn workspace_symbols(&self, query: &str) -> Result<Vec<Symbol>, LspError> {
        let value = self
            .request("workspace/symbol", json!({ "query": query }))
            .await?;
        Ok(
            match serde_json::from_value::<Option<WorkspaceSymbolResponse>>(value) {
                Ok(Some(WorkspaceSymbolResponse::Flat(flat))) => flat
                    .into_iter()
                    .filter_map(|symbol| {
                        located(
                            symbol.name,
                            symbol.kind,
                            symbol.container_name,
                            &symbol.location,
                        )
                    })
                    .collect(),
                Ok(Some(WorkspaceSymbolResponse::Nested(nested))) => nested
                    .into_iter()
                    .filter_map(|symbol| {
                        let location = match symbol.location {
                            OneOf::Left(location) => location,
                            // without a range the symbol is shown at the top of its file
                            OneOf::Right(file) => Location::new(file.uri, Range::default()),
                        };
                        located(symbol.name, symbol.kind, symbol.container_name, &location)
                    })
                    .collect(),
                _ => Vec::new(),
            },
        )
    }

    /// Asks for the semantic tokens of all of `path`, decoded with the server legend.
    ///
    /// # Errors
    ///
    /// Returns an error if the server is gone or fails the request.
    pub async fn semantic_tokens(&self, path: &Path) -> Result<Vec<SemanticToken>, LspError> {
        let Some(uri) = convert::path_to_uri(path) else {
            return Ok(Vec::new());
        };
        let Some(legend) = self
            .capabilities()
            .map(|caps| caps["semanticTokensProvider"]["legend"].clone())
            .filter(|legend| !legend.is_null())
        else {
            return Ok(Vec::new());
        };
        let params = json!({ "textDocument": { "uri": uri } });
        let value = self
            .request("textDocument/semanticTokens/full", params)
            .await?;
        Ok(
            match serde_json::from_value::<Option<SemanticTokensResult>>(value) {
                Ok(Some(SemanticTokensResult::Tokens(tokens))) => {
                    decode_tokens(&tokens.data, &legend)
                }
                _ => Vec::new(),
            },
        )
    }
}

#[cfg(test)]
/// Tests for the symbol and token helpers.
mod tests {
    use lsp_types::{
        ParameterInformation, ParameterLabel, SemanticToken as Packed, SignatureHelp,
        SignatureInformation,
    };
    use serde_json::json;

    use super::{decode_tokens, pick_signature, utf16_to_char};

    /// The active parameter is found by name or by offsets.
    #[test]
    fn picks_active_parameter() {
        let signature = |label: ParameterLabel| SignatureHelp {
            signatures: vec![SignatureInformation {
                label: "fn add(a: i32, b: i32)".into(),
                documentation: None,
                parameters: Some(vec![
                    ParameterInformation {
                        label: ParameterLabel::Simple("a: i32".into()),
                        documentation: None,
                    },
                    ParameterInformation {
                        label,
                        documentation: None,
                    },
                ]),
                active_parameter: None,
            }],
            active_signature: None,
            active_parameter: Some(1),
        };
        let by_name = pick_signature(signature(ParameterLabel::Simple("b: i32".into())));
        assert_eq!(by_name.expect("signature").active, Some((15, 21)));
        let by_offset = pick_signature(signature(ParameterLabel::LabelOffsets([15, 21])));
        assert_eq!(by_offset.expect("signature").active, Some((15, 21)));
    }

    /// Tokens are relative to the one before and named from the legend.
    #[test]
    fn decodes_tokens() {
        let legend =
            json!({ "tokenTypes": ["function", "variable"], "tokenModifiers": ["readonly"] });
        let data = [
            Packed {
                delta_line: 1,
                delta_start: 4,
                length: 3,
                token_type: 0,
                token_modifiers_bitset: 0,
            },
            Packed {
                delta_line: 0,
                delta_start: 5,
                length: 2,
                token_type: 1,
                token_modifiers_bitset: 1,
            },
        ];
        let tokens = decode_tokens(&data, &legend);
        assert_eq!(
            (tokens[0].line, tokens[0].start, &tokens[0].kind[..]),
            (1, 4, "function")
        );
        assert_eq!(
            (tokens[1].line, tokens[1].start, &tokens[1].kind[..]),
            (1, 9, "variable")
        );
        assert_eq!(tokens[1].modifiers, ["readonly"]);
    }

    /// UTF-16 offsets count characters outside the basic plane twice.
    #[test]
    fn converts_utf16_offsets() {
        assert_eq!(utf16_to_char("a\u{1F600}b", 3), 2);
        assert_eq!(utf16_to_char("abc", 9), 3);
    }
}
