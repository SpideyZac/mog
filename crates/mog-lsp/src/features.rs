//! Typed requests for editor features like completion, hover and go to definition.

use std::path::{Path, PathBuf};

use lsp_types::{
    CompletionItem, CompletionResponse, DocumentChanges, Documentation, GotoDefinitionResponse,
    Hover, HoverContents, Location, MarkedString, OneOf, Position, TextEdit, WorkspaceEdit,
};
use serde_json::{Value, json};

use crate::{
    client::{Client, LspError},
    convert,
};

/// Builds the `textDocument` and `position` arguments most requests take.
fn at(path: &Path, position: Position) -> Option<Value> {
    let uri = convert::path_to_uri(path)?;
    Some(json!({ "textDocument": { "uri": uri }, "position": position }))
}

/// Turns hover contents into plain text.
fn hover_text(contents: HoverContents) -> String {
    let marked = |marked: MarkedString| match marked {
        MarkedString::String(text) => text,
        MarkedString::LanguageString(code) => code.value,
    };
    match contents {
        HoverContents::Scalar(one) => marked(one),
        HoverContents::Array(many) => many
            .into_iter()
            .map(marked)
            .collect::<Vec<_>>()
            .join("\n\n"),
        HoverContents::Markup(markup) => markup.value,
    }
}

/// Returns the plain text of completion documentation.
pub fn documentation_text(documentation: &Documentation) -> &str {
    match documentation {
        Documentation::String(text) => text,
        Documentation::MarkupContent(markup) => &markup.value,
    }
}

impl Client {
    /// Asks for completions at `position` in `path`.
    ///
    /// # Errors
    ///
    /// Returns an error if the server is gone or fails the request.
    pub async fn completion(
        &self,
        path: &Path,
        position: Position,
    ) -> Result<Vec<CompletionItem>, LspError> {
        let Some(params) = at(path, position) else {
            return Ok(Vec::new());
        };
        let value = self.request("textDocument/completion", params).await?;
        Ok(
            match serde_json::from_value::<Option<CompletionResponse>>(value) {
                Ok(Some(CompletionResponse::Array(items))) => items,
                Ok(Some(CompletionResponse::List(list))) => list.items,
                _ => Vec::new(),
            },
        )
    }

    /// Asks what is at `position` in `path`, as plain text.
    ///
    /// # Errors
    ///
    /// Returns an error if the server is gone or fails the request.
    pub async fn hover(&self, path: &Path, position: Position) -> Result<Option<String>, LspError> {
        let Some(params) = at(path, position) else {
            return Ok(None);
        };
        let value = self.request("textDocument/hover", params).await?;
        Ok(serde_json::from_value::<Option<Hover>>(value)
            .ok()
            .flatten()
            .map(|hover| hover_text(hover.contents))
            .filter(|text| !text.trim().is_empty()))
    }

    /// Asks where the symbol at `position` in `path` is defined.
    ///
    /// # Errors
    ///
    /// Returns an error if the server is gone or fails the request.
    pub async fn definition(
        &self,
        path: &Path,
        position: Position,
    ) -> Result<Option<(PathBuf, Position)>, LspError> {
        let Some(params) = at(path, position) else {
            return Ok(None);
        };
        let value = self.request("textDocument/definition", params).await?;
        let first = |location: Location| {
            convert::uri_to_path(&location.uri).map(|path| (path, location.range.start))
        };
        Ok(
            match serde_json::from_value::<Option<GotoDefinitionResponse>>(value) {
                Ok(Some(GotoDefinitionResponse::Scalar(location))) => first(location),
                Ok(Some(GotoDefinitionResponse::Array(locations))) => {
                    locations.into_iter().next().and_then(first)
                }
                Ok(Some(GotoDefinitionResponse::Link(links))) => {
                    links.into_iter().next().and_then(|link| {
                        convert::uri_to_path(&link.target_uri)
                            .map(|path| (path, link.target_selection_range.start))
                    })
                }
                _ => None,
            },
        )
    }

    /// Asks how to rename the symbol at `position` in `path` to `new_name`.
    ///
    /// Returns the edits for every file that changes.
    ///
    /// # Errors
    ///
    /// Returns an error if the server is gone or fails the request.
    pub async fn rename(
        &self,
        path: &Path,
        position: Position,
        new_name: &str,
    ) -> Result<Vec<(PathBuf, Vec<TextEdit>)>, LspError> {
        let Some(mut params) = at(path, position) else {
            return Ok(Vec::new());
        };
        params["newName"] = json!(new_name);
        let value = self.request("textDocument/rename", params).await?;
        let Ok(Some(edit)) = serde_json::from_value::<Option<WorkspaceEdit>>(value) else {
            return Ok(Vec::new());
        };
        let mut files = Vec::new();
        for (uri, edits) in edit.changes.unwrap_or_default() {
            files.extend(convert::uri_to_path(&uri).map(|path| (path, edits)));
        }
        if let Some(DocumentChanges::Edits(documents)) = edit.document_changes {
            for document in documents {
                let edits = document
                    .edits
                    .into_iter()
                    .map(|edit| match edit {
                        OneOf::Left(edit) => edit,
                        OneOf::Right(annotated) => annotated.text_edit,
                    })
                    .collect();
                files.extend(
                    convert::uri_to_path(&document.text_document.uri).map(|path| (path, edits)),
                );
            }
        }
        Ok(files)
    }

    /// Asks how to format the whole of `path`.
    ///
    /// # Errors
    ///
    /// Returns an error if the server is gone or fails the request.
    pub async fn formatting(
        &self,
        path: &Path,
        tab_size: u32,
        insert_spaces: bool,
    ) -> Result<Vec<TextEdit>, LspError> {
        let Some(uri) = convert::path_to_uri(path) else {
            return Ok(Vec::new());
        };
        let params = json!({
            "textDocument": { "uri": uri },
            "options": { "tabSize": tab_size, "insertSpaces": insert_spaces },
        });
        let value = self.request("textDocument/formatting", params).await?;
        Ok(serde_json::from_value::<Option<Vec<TextEdit>>>(value)
            .ok()
            .flatten()
            .unwrap_or_default())
    }
}

#[cfg(test)]
/// Tests for feature helpers.
mod tests {
    use lsp_types::{HoverContents, LanguageString, MarkedString, MarkupContent, MarkupKind};

    use super::hover_text;

    /// Every hover shape flattens to text.
    #[test]
    fn flattens_hover() {
        let markup = HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: "**fn** main".into(),
        });
        assert_eq!(hover_text(markup), "**fn** main");
        let many = HoverContents::Array(vec![
            MarkedString::String("a".into()),
            MarkedString::LanguageString(LanguageString {
                language: "rust".into(),
                value: "fn b()".into(),
            }),
        ]);
        assert_eq!(hover_text(many), "a\n\nfn b()");
    }
}
