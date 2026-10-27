//! The bundled catalog of native configuration producers.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Builtin {
    pub(crate) id: String,
    /// The format written when a source does not choose one.
    pub(crate) default: String,
    /// Native filename for each supported format.
    pub(crate) formats: BTreeMap<String, String>,
    /// Every filename the tool discovers; any of them competes with the generated file.
    pub(crate) alternatives: Vec<String>,
    /// A `package.json` field the tool also reads.
    #[serde(default)]
    pub(crate) manifest_key: Option<String>,
    /// Defaults written before projected and source values.
    #[serde(default)]
    pub(crate) config: Option<serde_json::Value>,
    /// File extensions the tool handles; empty means every file.
    pub(crate) extensions: Vec<String>,
    /// Whether the tool accepts `overrides: [{ files, options }]`.
    #[serde(default)]
    pub(crate) overrides: bool,
    /// Whether the tool uses the nearest config file for each file it handles.
    #[serde(default = "nearest")]
    pub(crate) nearest: bool,
    #[serde(default)]
    pub(crate) projection: Vec<Mapping>,
}

/// How a normalized EditorConfig property becomes a native key.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Mapping {
    pub(crate) property: String,
    pub(crate) path: Vec<String>,
    /// Native values for each property value. A missing value cannot be expressed;
    /// `null` means the value needs no native setting.
    #[serde(default)]
    pub(crate) values: Option<BTreeMap<String, serde_json::Value>>,
    #[serde(default)]
    pub(crate) max: Option<i64>,
    /// The tool's value when the key is absent.
    #[serde(default)]
    pub(crate) default: Option<serde_json::Value>,
}

fn nearest() -> bool {
    true
}

pub(crate) fn all() -> &'static [Builtin] {
    static CATALOG: OnceLock<Vec<Builtin>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("catalog.json")).expect("the bundled catalog is valid")
    })
}

pub(crate) fn get(id: &str) -> Option<&'static Builtin> {
    all().iter().find(|builtin| builtin.id == id)
}

/// Tools configured only through `.editorconfig`, which the generated file already serves.
pub(crate) fn reads_editorconfig(id: &str) -> Option<&'static str> {
    match id {
        "shfmt" => Some("[\"*.sh\"] shell_variant = \"bash\""),
        "ktlint" => Some("[\"*.{kt,kts}\"] ktlint_code_style = \"ktlint_official\""),
        _ => None,
    }
}

impl Builtin {
    pub(crate) fn handles(&self, extension: Option<&str>) -> bool {
        self.extensions.is_empty()
            || extension.is_some_and(|extension| {
                self.extensions
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(extension))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        let properties = [
            "indent_style",
            "indent_width",
            "max_line_length",
            "end_of_line",
            "insert_final_newline",
            "trim_trailing_whitespace",
            "spelling_language",
        ];
        let mut ids = std::collections::BTreeSet::new();
        for builtin in all() {
            assert!(ids.insert(&builtin.id), "duplicate {}", builtin.id);
            assert!(
                builtin.formats.contains_key(&builtin.default),
                "{}",
                builtin.id
            );
            for filename in builtin.formats.values() {
                assert!(builtin.alternatives.contains(filename), "{filename}");
            }
            for mapping in &builtin.projection {
                assert!(
                    properties.contains(&mapping.property.as_str()),
                    "{}: {}",
                    builtin.id,
                    mapping.property
                );
                assert!(!mapping.path.is_empty());
            }
            assert!(reads_editorconfig(&builtin.id).is_none());
        }
    }
}
