//! The source model every syntax evaluates to, and the rules both syntaxes share.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::catalog;
use crate::repository::Repository;
use crate::vale;
use crate::value::{self, Value};

/// Source filenames, in the order they are reported when several share a directory.
pub(crate) const SOURCE_NAMES: [&str; 4] = [
    ".toolconfig",
    ".toolconfig.toml",
    ".editorconfig.toml",
    ".toolconfig.pkl",
];

/// Standard EditorConfig properties. They are typed, written to `.editorconfig` and projected.
pub(crate) const STANDARD: [&str; 9] = [
    "indent_style",
    "indent_size",
    "tab_width",
    "end_of_line",
    "charset",
    "trim_trailing_whitespace",
    "insert_final_newline",
    "max_line_length",
    "spelling_language",
];

pub(crate) const ADOPTING_GUIDE: &str =
    "https://github.com/mathematic-inc/toolconfig/blob/main/docs/adopting.md";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Origin {
    pub(crate) file: PathBuf,
    pub(crate) line: Option<usize>,
    repository: Option<String>,
}

impl Origin {
    pub(crate) fn new(file: &Path, line: Option<usize>) -> Self {
        Self {
            file: file.to_path_buf(),
            line,
            repository: None,
        }
    }

    /// The origin relative to the project root, as `path:line`.
    pub(crate) fn display(&self, root: &Path) -> String {
        let file = self.repository.clone().unwrap_or_else(|| {
            self.file
                .strip_prefix(root)
                .unwrap_or(&self.file)
                .to_string_lossy()
                .replace('\\', "/")
        });
        match self.line {
            Some(line) => format!("{file}:{line}"),
            None => file,
        }
    }

    pub(crate) fn identify_repository(&mut self, tree: &Path, name: &str) {
        if let Ok(path) = self.file.strip_prefix(tree) {
            self.repository = Some(format!(
                "{name}:{}",
                path.to_string_lossy().replace('\\', "/")
            ));
        }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display(Path::new("")))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Syntax {
    Toml,
    Pkl,
}

pub(crate) fn syntax_of(path: &Path) -> Option<Syntax> {
    match path.file_name()?.to_str()? {
        ".toolconfig" | ".toolconfig.toml" | ".editorconfig.toml" => Some(Syntax::Toml),
        ".toolconfig.pkl" => Some(Syntax::Pkl),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Source {
    pub(crate) path: PathBuf,
    pub(crate) dir: PathBuf,
    pub(crate) root: bool,
    pub(crate) settings: Settings,
    pub(crate) sections: Vec<Section>,
}

/// A declaration before imports and omitted settings have been resolved.
#[derive(Clone, Debug)]
pub(crate) struct ParsedSource {
    pub(crate) path: PathBuf,
    pub(crate) dir: PathBuf,
    pub(crate) root: bool,
    pub(crate) settings: ParsedSettings,
    pub(crate) sections: Vec<Section>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ParsedSettings {
    pub(crate) extends: Vec<Repository>,
    pub(crate) tools: Option<Vec<(String, Origin)>>,
    pub(crate) tool: BTreeMap<String, ParsedToolSettings>,
    pub(crate) files: Vec<FileSpec>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ParsedToolSettings {
    pub(crate) format: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) skip: Option<Vec<String>>,
    pub(crate) vale: vale::Settings,
}

impl ParsedSource {
    pub(crate) fn validate(&self) -> Result<()> {
        let source = Source {
            path: self.path.clone(),
            dir: self.dir.clone(),
            root: self.root,
            settings: Settings {
                tools: self.settings.tools.clone().unwrap_or_default(),
                tool: self
                    .settings
                    .tool
                    .iter()
                    .map(|(id, settings)| {
                        (
                            id.clone(),
                            ToolSettings {
                                format: settings.format.clone(),
                                path: settings.path.clone(),
                                skip: settings.skip.clone().unwrap_or_default(),
                                vale: settings.vale.clone(),
                            },
                        )
                    })
                    .collect(),
                files: self.settings.files.clone(),
            },
            sections: self.sections.clone(),
        };
        source.check_settings(Validation::Declaration)?;
        source.validate_sections()
    }

    pub(crate) fn identify_repository(&mut self, tree: &Path, name: &str) {
        if let Some(tools) = &mut self.settings.tools {
            for (_, origin) in tools {
                origin.identify_repository(tree, name);
            }
        }
        for file in &mut self.settings.files {
            file.origin.identify_repository(tree, name);
        }
        for section in &mut self.sections {
            section.origin.identify_repository(tree, name);
            for property in &mut section.properties {
                property.origin.identify_repository(tree, name);
            }
            for tool in &mut section.tools {
                for (_, origin) in &mut tool.origins {
                    origin.identify_repository(tree, name);
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Settings {
    pub(crate) tools: Vec<(String, Origin)>,
    pub(crate) tool: BTreeMap<String, ToolSettings>,
    pub(crate) files: Vec<FileSpec>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ToolSettings {
    pub(crate) format: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) skip: Vec<String>,
    pub(crate) vale: vale::Settings,
}

#[derive(Clone, Debug)]
pub(crate) struct FileSpec {
    pub(crate) label: String,
    pub(crate) path: String,
    pub(crate) format: String,
    pub(crate) config: Value,
    pub(crate) origin: Origin,
}

#[derive(Clone)]
pub(crate) struct Section {
    pub(crate) glob: String,
    pub(crate) matcher: ec4rs::Section,
    pub(crate) origin: Origin,
    /// Standard and other properties, in source order, with dotted keys flattened.
    pub(crate) properties: Vec<Property>,
    pub(crate) tools: Vec<ToolTable>,
}

impl fmt::Debug for Section {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Section")
            .field("glob", &self.glob)
            .field("origin", &self.origin)
            .field("properties", &self.properties)
            .field("tools", &self.tools)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Property {
    pub(crate) key: String,
    pub(crate) value: Value,
    pub(crate) origin: Origin,
}

#[derive(Clone, Debug)]
pub(crate) struct ToolTable {
    pub(crate) id: String,
    pub(crate) value: Value,
    /// The origin of each leaf, for diagnostics and `explain`.
    pub(crate) origins: Vec<(value::Path, Origin)>,
}

impl Section {
    pub(crate) fn new(glob: String, origin: Origin) -> Result<Self> {
        if glob.is_empty() {
            bail!("{origin}: a section glob cannot be empty");
        }
        if glob.contains(['\n', '\r', '\0']) {
            bail!("{origin}: a section glob cannot contain line breaks");
        }
        Ok(Self {
            matcher: ec4rs::Section::new(&glob),
            glob,
            origin,
            properties: Vec::new(),
            tools: Vec::new(),
        })
    }

    /// Whether this section applies to `path`, given relative to the source's directory.
    pub(crate) fn applies_to(&self, relative: &Path) -> bool {
        self.matcher.applies_to(relative)
    }
}

/// Checks a standard property's value. `key` must already be lowercase.
pub(crate) fn check_standard(key: &str, value: &Value) -> Result<()> {
    let text = value.as_str().map(str::to_ascii_lowercase);
    let text = text.as_deref();
    if text == Some("unset") {
        return Ok(());
    }
    let positive = matches!(value, Value::Int(number) if *number > 0);
    let valid = match key {
        "indent_style" => matches!(text, Some("space" | "tab")),
        "indent_size" => positive || text == Some("tab"),
        "tab_width" => positive,
        "end_of_line" => matches!(text, Some("lf" | "crlf" | "cr")),
        "charset" => matches!(
            text,
            Some("latin1" | "utf-8" | "utf-8-bom" | "utf-16be" | "utf-16le")
        ),
        "trim_trailing_whitespace" | "insert_final_newline" => matches!(value, Value::Bool(_)),
        "max_line_length" => positive || text == Some("off"),
        "spelling_language" => text.is_some_and(|text| !text.is_empty()),
        _ => unreachable!("not a standard property: {key}"),
    };
    if !valid {
        let expected = match key {
            "indent_style" => r#""space" or "tab""#,
            "indent_size" => r#"a positive integer or "tab""#,
            "tab_width" => "a positive integer",
            "end_of_line" => r#""lf", "crlf" or "cr""#,
            "charset" => r#""latin1", "utf-8", "utf-8-bom", "utf-16be" or "utf-16le""#,
            "trim_trailing_whitespace" | "insert_final_newline" => "true or false",
            "max_line_length" => r#"a positive integer or "off""#,
            _ => "a language tag such as \"en-US\"",
        };
        bail!(
            "{key} must be {expected} (or \"unset\"), not {}",
            value.describe()
        );
    }
    Ok(())
}

/// The bare spelling EditorConfig uses for a property value.
pub(crate) fn render_scalar(value: &Value) -> Result<String> {
    let text = match value {
        Value::Bool(value) => value.to_string(),
        Value::Int(value) => value.to_string(),
        Value::Float(value) => value.to_string(),
        Value::String(value) | Value::Datetime(value) => value.clone(),
        _ => bail!(
            "EditorConfig values are single-line scalars, not {}",
            value.describe()
        ),
    };
    if text.contains(['\n', '\r', '\0']) || text.trim() != text {
        bail!(
            "EditorConfig values cannot contain line breaks or surrounding whitespace: {:?}",
            text
        );
    }
    Ok(text)
}

enum Validation {
    Declaration,
    Resolved,
}

impl Source {
    /// Checks effective generation settings after inheritance.
    pub(crate) fn validate_settings(&self) -> Result<()> {
        self.check_settings(Validation::Resolved)
    }

    fn check_settings(&self, validation: Validation) -> Result<()> {
        let mut seen = BTreeSet::new();
        for (id, origin) in &self.settings.tools {
            if let Some(example) = catalog::reads_editorconfig(id) {
                bail!(
                    "{origin}: {id} reads .editorconfig; put its options in a section instead, such as {example}"
                );
            }
            catalog::get(id).with_context(|| {
                format!(
                    "{origin}: unknown tool {id:?}{}",
                    suggestion(id, catalog::all().iter().map(|builtin| builtin.id.as_str()))
                )
            })?;
            if !seen.insert(id) {
                bail!("{origin}: {id} is listed twice in tools");
            }
        }
        for (id, settings) in &self.settings.tool {
            let builtin = catalog::get(id)
                .with_context(|| format!("{}: unknown tool {id:?}", self.path.display()))?;
            if let Some(format) = &settings.format
                && !builtin.formats.contains_key(format)
            {
                bail!(
                    "{}: {id} cannot be written as {format}; supported formats: {}",
                    self.path.display(),
                    builtin
                        .formats
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            if let Some(path) = &settings.path
                && !builtin.alternatives.contains(path)
            {
                bail!(
                    "{}: {id} does not read {path:?}; choose one of {}",
                    self.path.display(),
                    builtin.alternatives.join(", ")
                );
            }
            for property in &settings.skip {
                if property != "*" && !STANDARD.contains(&property.as_str()) {
                    bail!(
                        "{}: {id} skip lists {property:?}, which is not a standard property",
                        self.path.display()
                    );
                }
            }
            settings.vale.validate(&self.path)?;
            if matches!(validation, Validation::Resolved) && !seen.contains(id) {
                bail!(
                    "{}: [toolconfig.{id}] configures a tool missing from tools",
                    self.path.display()
                );
            }
        }
        let mut labels = BTreeSet::new();
        for file in &self.settings.files {
            if !labels.insert(&file.label) {
                bail!("{}: duplicate file {:?}", file.origin, file.label);
            }
            if !matches!(
                file.format.as_str(),
                "json" | "yaml" | "toml" | "ini" | "text" | "lines"
            ) {
                bail!(
                    "{}: file {:?} has unsupported format {:?}; use json, yaml, toml, ini, text or lines",
                    file.origin,
                    file.label,
                    file.format
                );
            }
        }
        Ok(())
    }

    fn validate_sections(&self) -> Result<()> {
        let mut globs = BTreeSet::new();
        for section in &self.sections {
            if !globs.insert(&section.glob) {
                bail!(
                    "{}: section {:?} appears twice",
                    section.origin,
                    section.glob
                );
            }
            let mut keys = BTreeSet::new();
            for property in &section.properties {
                let lower = property.key.to_ascii_lowercase();
                if !keys.insert(lower.clone()) {
                    bail!(
                        "{}: {} is set twice in {:?}",
                        property.origin,
                        property.key,
                        section.glob
                    );
                }
                if lower == "root" {
                    bail!(
                        "{}: root belongs at the top of the file, outside any section",
                        property.origin
                    );
                }
                if STANDARD.contains(&lower.as_str()) {
                    if lower != property.key {
                        bail!(
                            "{}: write {lower} in lowercase; EditorConfig lowercases keys",
                            property.origin
                        );
                    }
                    check_standard(&lower, &property.value)
                        .with_context(|| property.origin.to_string())?;
                }
                render_scalar(&property.value).with_context(|| {
                    format!(
                        "{}: {} in {:?}",
                        property.origin, property.key, section.glob
                    )
                })?;
            }
        }
        Ok(())
    }
}

/// A "did you mean" suffix for an unknown name.
pub(crate) fn suggestion<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> String {
    fn distance(a: &str, b: &str) -> usize {
        let b = b.chars().collect::<Vec<_>>();
        let mut previous = (0..=b.len()).collect::<Vec<_>>();
        for (i, left) in a.chars().enumerate() {
            let mut current = vec![i + 1];
            for (j, right) in b.iter().enumerate() {
                let cost = usize::from(left != *right);
                current.push(
                    (previous[j] + cost)
                        .min(previous[j + 1] + 1)
                        .min(current[j] + 1),
                );
            }
            previous = current;
        }
        previous[b.len()]
    }
    candidates
        .map(|candidate| (distance(name, candidate), candidate))
        .filter(|(distance, _)| *distance <= 2)
        .min()
        .map(|(_, candidate)| format!("; did you mean {candidate:?}?"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_values_are_checked() {
        check_standard("indent_style", &Value::String("Space".into())).unwrap();
        check_standard("indent_size", &Value::String("tab".into())).unwrap();
        check_standard("max_line_length", &Value::String("off".into())).unwrap();
        check_standard("insert_final_newline", &Value::String("unset".into())).unwrap();
        assert!(check_standard("indent_style", &Value::String("spaces".into())).is_err());
        assert!(check_standard("indent_size", &Value::Int(0)).is_err());
        assert!(check_standard("trim_trailing_whitespace", &Value::String("true".into())).is_err());
    }

    #[test]
    fn suggestions_find_close_names() {
        let names = ["rustfmt", "ruff"];
        assert_eq!(
            suggestion("rustmft", names.into_iter()),
            "; did you mean \"rustfmt\"?"
        );
        assert_eq!(suggestion("prettier", names.into_iter()), "");
    }
}
