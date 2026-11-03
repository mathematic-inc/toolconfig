//! The bundled catalog of native configuration producers.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::value::Value;

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
    /// Where blocks and object-valued attributes go, for tools written in HCL.
    #[serde(default)]
    pub(crate) hcl: Option<Hcl>,
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
    /// Projects the property only while the tool's own options turn an item on.
    #[serde(default)]
    pub(crate) when: Option<Enabled>,
}

/// An item the tool's options switch on, such as a linter: an `enable` list names it, or a
/// default set includes it, and a `disable` list does not name it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Enabled {
    pub(crate) item: String,
    /// The list that turns the item on.
    pub(crate) enable: Vec<String>,
    /// The list that turns the item off, which wins over `enable`.
    pub(crate) disable: Vec<String>,
    /// Where the options choose a default set.
    pub(crate) set: Vec<String>,
    /// The default sets that include the item.
    pub(crate) sets: Vec<String>,
}

impl Enabled {
    /// Whether `options`, the tool's options for one file, turn the item on.
    pub(crate) fn holds(&self, options: &Value) -> bool {
        let lists = |path: &[String]| {
            matches!(options.get_path(path), Some(Value::Array(items))
                if items.iter().any(|item| item.as_str() == Some(self.item.as_str())))
        };
        let in_set = options
            .get_path(&self.set)
            .and_then(Value::as_str)
            .is_some_and(|set| self.sets.iter().any(|known| known == set));
        !lists(&self.disable) && (lists(&self.enable) || in_set)
    }
}

/// HCL's JSON syntax tells blocks from object-valued attributes only through the reader's
/// schema, so the catalog names them. Paths start at the file's top; `*` stands for a label.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Hcl {
    /// Block bodies. Each trailing `*` is a label the block type takes.
    pub(crate) blocks: Vec<Vec<String>>,
    /// Attributes whose values are objects, or arrays of objects.
    pub(crate) attributes: Vec<Vec<String>>,
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
                if let Some(when) = &mapping.when {
                    assert!(!when.item.is_empty(), "{}", builtin.id);
                    for path in [&when.enable, &when.disable, &when.set] {
                        assert!(!path.is_empty(), "{}", builtin.id);
                    }
                }
            }
            assert_eq!(
                builtin.formats.contains_key("hcl"),
                builtin.hcl.is_some(),
                "{}: an HCL format needs an HCL schema",
                builtin.id
            );
            if let Some(hcl) = &builtin.hcl {
                for path in hcl.blocks.iter().chain(&hcl.attributes) {
                    assert!(path.first().is_some_and(|key| key != "*"), "{}", builtin.id);
                }
            }
            assert!(reads_editorconfig(&builtin.id).is_none());
        }
    }

    #[test]
    fn enabled_follows_enable_disable_and_default_sets() {
        let lll = get("golangci-lint").unwrap().projection[0]
            .when
            .as_ref()
            .unwrap();
        let options = |json| Value::from_json(&json);
        let on = [
            serde_json::json!({"linters": {"enable": ["lll"]}}),
            serde_json::json!({"linters": {"default": "all"}}),
            serde_json::json!({"linters": {"default": "fast", "disable": ["gocritic"]}}),
        ];
        let off = [
            serde_json::json!({}),
            serde_json::json!({"linters": {"default": "standard"}}),
            serde_json::json!({"linters": {"enable": ["lll"], "disable": ["lll"]}}),
            serde_json::json!({"linters": {"default": "all", "disable": ["lll"]}}),
            serde_json::json!({"formatters": {"enable": ["lll"]}}),
        ];
        for json in on {
            assert!(lll.holds(&options(json.clone())), "{json}");
        }
        for json in off {
            assert!(!lll.holds(&options(json.clone())), "{json}");
        }
    }
}
