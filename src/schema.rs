//! The JSON Schema for TOML sources, generated from the catalog.

use serde_json::{Value, json};

use crate::catalog;

pub(crate) fn url() -> String {
    format!(
        "https://github.com/mathematic-inc/toolconfig/releases/download/v{}/toolconfig.schema.json",
        env!("CARGO_PKG_VERSION")
    )
}

pub(crate) fn generate() -> Value {
    let ids = catalog::all()
        .iter()
        .map(|builtin| builtin.id.clone())
        .collect::<Vec<_>>();
    let mut settings = serde_json::Map::new();
    settings.insert(
        "tools".into(),
        json!({
            "description": "Tools whose native configuration file is generated in this directory.",
            "type": "array",
            "items": { "enum": ids },
            "uniqueItems": true
        }),
    );
    settings.insert(
        "files".into(),
        json!({
            "description": "Additional generated files, keyed by label.",
            "type": "object",
            "additionalProperties": { "$ref": "#/definitions/file" }
        }),
    );
    let mut section = serde_json::Map::new();
    for (key, schema) in standard() {
        section.insert(key.into(), schema);
    }
    for builtin in catalog::all() {
        settings.insert(
            builtin.id.clone(),
            json!({
                "description": format!("How {} is generated.", builtin.id),
                "type": "object",
                "properties": {
                    "format": {
                        "description": "The native format to write.",
                        "enum": builtin.formats.keys().collect::<Vec<_>>()
                    },
                    "path": {
                        "description": "The native filename to write, among those the tool reads.",
                        "enum": builtin.alternatives
                    },
                    "skip": {
                        "description": "Standard properties not projected into this tool; \"*\" skips all.",
                        "type": "array",
                        "items": { "enum": std::iter::once("*").chain(crate::model::STANDARD).collect::<Vec<_>>() },
                        "uniqueItems": true
                    }
                },
                "additionalProperties": false
            }),
        );
        section.insert(
            builtin.id.clone(),
            json!({
                "description": format!("Native {} options for the files this section matches.", builtin.id),
                "type": "object"
            }),
        );
    }
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": url(),
        "title": "ToolConfig source",
        "description": "EditorConfig sections in TOML, with native options for each tool. ToolConfig generates .editorconfig and each tool's configuration file from it.",
        "type": "object",
        "properties": {
            "root": {
                "description": "Stops the EditorConfig cascade at this directory. The project's top source must set it.",
                "type": "boolean"
            },
            "toolconfig": {
                "description": "Settings for the files generated in this directory.",
                "type": "object",
                "properties": settings,
                "additionalProperties": false
            }
        },
        "additionalProperties": { "$ref": "#/definitions/section" },
        "definitions": {
            "section": {
                "description": "An EditorConfig section. The key is a glob; later sections take precedence.",
                "type": "object",
                "properties": section,
                "additionalProperties": { "$ref": "#/definitions/property" }
            },
            "property": {
                "description": "An EditorConfig property that other tools read, written to .editorconfig as is.",
                "anyOf": [
                    { "type": ["string", "integer", "number", "boolean"] },
                    { "type": "object", "additionalProperties": { "$ref": "#/definitions/property" } }
                ]
            },
            "file": {
                "type": "object",
                "properties": {
                    "path": { "description": "Path relative to this directory.", "type": "string" },
                    "format": { "enum": ["json", "yaml", "toml", "ini", "text", "lines"] },
                    "config": { "description": "The file's content: a table, a string for text, or an array of strings for lines." }
                },
                "required": ["path"],
                "additionalProperties": false
            }
        }
    })
}

fn standard() -> Vec<(&'static str, Value)> {
    let unset = json!({ "const": "unset" });
    let positive = json!({ "type": "integer", "minimum": 1 });
    vec![
        (
            "indent_style",
            json!({ "description": "Indent with spaces or tabs.", "enum": ["space", "tab", "unset"] }),
        ),
        (
            "indent_size",
            json!({ "description": "Columns per indentation level, or \"tab\" to use tab_width.", "anyOf": [positive, { "enum": ["tab", "unset"] }] }),
        ),
        (
            "tab_width",
            json!({ "description": "Columns a tab character occupies. Defaults to indent_size.", "anyOf": [positive, unset] }),
        ),
        (
            "end_of_line",
            json!({ "description": "Line ending.", "enum": ["lf", "crlf", "cr", "unset"] }),
        ),
        (
            "charset",
            json!({ "description": "Character set.", "enum": ["latin1", "utf-8", "utf-8-bom", "utf-16be", "utf-16le", "unset"] }),
        ),
        (
            "trim_trailing_whitespace",
            json!({ "description": "Remove whitespace before line breaks.", "anyOf": [{ "type": "boolean" }, unset] }),
        ),
        (
            "insert_final_newline",
            json!({ "description": "End files with a line break.", "anyOf": [{ "type": "boolean" }, unset] }),
        ),
        (
            "max_line_length",
            json!({ "description": "Maximum line length, or \"off\".", "anyOf": [positive, { "enum": ["off", "unset"] }] }),
        ),
        (
            "spelling_language",
            json!({ "description": "Natural language for spell checking, such as \"en-US\".", "type": "string" }),
        ),
    ]
}
