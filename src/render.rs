//! Native file formats, and the generated `.editorconfig`.

mod hcl;

use anyhow::{Context, Result, bail};

use crate::catalog::Builtin;
use crate::model::{self, STANDARD, Source};
use crate::project::GENERATED_HEADER;
use crate::value::Value;

/// Layout for generated files, taken from the EditorConfig properties at the file's own path,
/// so formatters that read the same properties leave the file unchanged.
#[derive(Clone, Debug)]
pub(crate) struct Style {
    /// Columns per indentation level.
    pub(crate) indent: usize,
    pub(crate) tabs: bool,
    /// The line width within which short arrays stay on one line.
    pub(crate) width: usize,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            indent: 2,
            tabs: false,
            width: 80,
        }
    }
}

impl Style {
    fn pad(&self, depth: usize) -> String {
        if self.tabs {
            "\t".repeat(depth)
        } else {
            " ".repeat(depth * self.indent)
        }
    }
}

/// A tool's native file, in its own syntax or one of the shared formats.
pub(crate) fn native(
    builtin: &Builtin,
    format: &str,
    value: &Value,
    style: &Style,
) -> Result<String> {
    match format {
        "hcl" => {
            let schema = builtin
                .hcl
                .as_ref()
                .context("the catalog has no HCL schema")?;
            hcl::render(value, schema, style).map_err(|error| {
                if error.is::<hcl::Unplaced>() && builtin.formats.contains_key("json") {
                    anyhow::anyhow!(
                        "{error}\nhelp: write JSON with [toolconfig.{id}] format = \"json\", which {id} and its plugins read by their own schemas",
                        id = builtin.id
                    )
                } else {
                    error
                }
            })
        }
        "shellcheckrc" => shellcheckrc(value),
        _ => render(format, value, style),
    }
}

pub(crate) fn render(format: &str, value: &Value, style: &Style) -> Result<String> {
    match format {
        "json" => {
            let mut text = String::new();
            json(&value.to_json()?, 0, 0, style, &mut text)?;
            text.push('\n');
            Ok(text)
        }
        "yaml" => {
            let value = value.to_json()?;
            match &value {
                serde_json::Value::Object(fields) if !fields.is_empty() => {
                    let mut text = String::new();
                    // YAML forbids tab indentation.
                    let spaces = Style {
                        tabs: false,
                        ..style.clone()
                    };
                    yaml_mapping(fields, 0, &spaces, &mut text)?;
                    Ok(text)
                }
                other => Ok(format!("{}\n", yaml_scalar(other)?)),
            }
        }
        "toml" => {
            let value = value
                .to_toml()
                .context("value is not representable in TOML")?;
            if !value.is_table() {
                bail!("a TOML file needs a table at its top level");
            }
            Ok(toml::to_string(&value)?)
        }
        "ini" => render_ini(&value.to_json()?),
        "text" => value
            .as_str()
            .map(str::to_owned)
            .context("format \"text\" needs a string"),
        "lines" => {
            let Value::Array(values) = value else {
                bail!("format \"lines\" needs an array of strings");
            };
            let mut text = String::new();
            for value in values {
                let line = value
                    .as_str()
                    .context("format \"lines\" needs an array of strings")?;
                if line.contains(['\n', '\r']) {
                    bail!("format \"lines\" entries cannot contain line breaks");
                }
                text.push_str(line);
                text.push('\n');
            }
            Ok(text)
        }
        _ => bail!("unsupported format {format:?}"),
    }
}

/// JSON in the layout formatters such as Prettier produce: tables expanded, short arrays of
/// scalars on one line. `column` is where the value starts on its line.
fn json(
    value: &serde_json::Value,
    depth: usize,
    column: usize,
    style: &Style,
    text: &mut String,
) -> Result<()> {
    let inner = (depth + 1) * style.indent;
    match value {
        serde_json::Value::Object(fields) if fields.is_empty() => text.push_str("{}"),
        serde_json::Value::Object(fields) => {
            text.push_str("{\n");
            for (index, (key, value)) in fields.iter().enumerate() {
                let key = serde_json::to_string(key)?;
                text.push_str(&style.pad(depth + 1));
                text.push_str(&key);
                text.push_str(": ");
                json(value, depth + 1, inner + key.len() + 2, style, text)?;
                if index + 1 < fields.len() {
                    text.push(',');
                }
                text.push('\n');
            }
            text.push_str(&style.pad(depth));
            text.push('}');
        }
        serde_json::Value::Array(values) if values.is_empty() => text.push_str("[]"),
        serde_json::Value::Array(values) => {
            let scalars = values
                .iter()
                .all(|value| !value.is_object() && !value.is_array());
            let inline = values
                .iter()
                .map(serde_json::to_string)
                .collect::<Result<Vec<_>, _>>()?
                .join(", ");
            // The trailing comma after the array counts toward the width.
            if scalars && column + inline.len() + 3 <= style.width {
                text.push('[');
                text.push_str(&inline);
                text.push(']');
            } else {
                text.push_str("[\n");
                for (index, value) in values.iter().enumerate() {
                    text.push_str(&style.pad(depth + 1));
                    json(value, depth + 1, inner, style, text)?;
                    if index + 1 < values.len() {
                        text.push(',');
                    }
                    text.push('\n');
                }
                text.push_str(&style.pad(depth));
                text.push(']');
            }
        }
        scalar => text.push_str(&serde_json::to_string(scalar)?),
    }
    Ok(())
}

/// Block-style YAML with indented sequences, as Prettier and yamllint's defaults expect.
fn yaml_mapping(
    fields: &serde_json::Map<String, serde_json::Value>,
    indent: usize,
    style: &Style,
    text: &mut String,
) -> Result<()> {
    let pad = " ".repeat(indent);
    let step = style.indent.max(1);
    for (key, value) in fields {
        let key = yaml_scalar(&serde_json::Value::String(key.clone()))?;
        match value {
            serde_json::Value::Object(fields) if !fields.is_empty() => {
                text.push_str(&format!("{pad}{key}:\n"));
                yaml_mapping(fields, indent + step, style, text)?;
            }
            serde_json::Value::Array(values) if !values.is_empty() => {
                text.push_str(&format!("{pad}{key}:\n"));
                yaml_sequence(values, indent + step, style, text)?;
            }
            value => text.push_str(&format!("{pad}{key}: {}\n", yaml_scalar(value)?)),
        }
    }
    Ok(())
}

fn yaml_sequence(
    values: &[serde_json::Value],
    indent: usize,
    style: &Style,
    text: &mut String,
) -> Result<()> {
    let pad = " ".repeat(indent);
    let step = style.indent.max(1);
    for value in values {
        match value {
            serde_json::Value::Object(fields) if !fields.is_empty() => {
                // The first entry shares the dash's line; the rest align under it.
                // The dash takes two columns, so entries align two columns in.
                let mut item = String::new();
                yaml_mapping(fields, indent + 2, style, &mut item)?;
                text.push_str(&pad);
                text.push_str("- ");
                text.push_str(&item[indent + 2..]);
            }
            serde_json::Value::Array(values) if !values.is_empty() => {
                text.push_str(&format!("{pad}-\n"));
                yaml_sequence(values, indent + step, style, text)?;
            }
            value => text.push_str(&format!("{pad}- {}\n", yaml_scalar(value)?)),
        }
    }
    Ok(())
}

/// A YAML scalar: plain when that is unambiguous, otherwise double-quoted.
fn yaml_scalar(value: &serde_json::Value) -> Result<String> {
    Ok(match value {
        serde_json::Value::Object(_) => "{}".to_owned(),
        serde_json::Value::Array(_) => "[]".to_owned(),
        serde_json::Value::String(text) if plain_yaml(text) => text.clone(),
        // JSON's double-quoted strings are valid YAML double-quoted scalars.
        scalar => serde_json::to_string(scalar)?,
    })
}

/// Whether a string reads back as the same string when written unquoted, in YAML 1.1 and 1.2.
/// Anything doubtful is quoted.
fn plain_yaml(text: &str) -> bool {
    let Some(first) = text.chars().next() else {
        return false;
    };
    // YAML 1.1 readers such as yamllint and GitHub Actions treat these as booleans or null.
    let reserved = matches!(
        text.to_ascii_lowercase().as_str(),
        "y" | "n" | "yes" | "no" | "on" | "off" | "true" | "false" | "null" | "~" | "<<"
    );
    // Numbers, dates and special floats such as .inf.
    let numeric = first.is_ascii_digit()
        || (matches!(first, '-' | '+' | '.')
            && text[1..]
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_digit() || ch.is_ascii_alphabetic() && first == '.'));
    !reserved
        && !numeric
        && text.trim() == text
        && !"-?:,[]{}#&*!|>'\"%@`".contains(first)
        && !text.ends_with(':')
        && !text.contains(": ")
        && !text.contains(" #")
        && !text.chars().any(|ch| ch.is_control())
}

/// INI with sections, as sqlfluff and Vale read it. A list is one entry whose items are separated
/// by commas, with a comma inside an item written `\,`, as Vale reads its lists.
fn render_ini(value: &serde_json::Value) -> Result<String> {
    fn scalar(value: &serde_json::Value) -> Result<String> {
        match value {
            serde_json::Value::String(value) => {
                if value.trim() != value || value.contains(['\r', '\n', '\0']) {
                    bail!("INI strings cannot contain line breaks, NUL, or surrounding whitespace");
                }
                Ok(value.clone())
            }
            serde_json::Value::Number(_) | serde_json::Value::Bool(_) => Ok(value.to_string()),
            serde_json::Value::Array(items) => {
                if items.is_empty() {
                    bail!("INI lists cannot be empty; leave the key out");
                }
                let items = items
                    .iter()
                    .map(|item| match item {
                        serde_json::Value::String(_)
                        | serde_json::Value::Number(_)
                        | serde_json::Value::Bool(_) => {
                            scalar(item).map(|item| item.replace(',', "\\,"))
                        }
                        _ => bail!("INI lists hold strings, numbers, or booleans"),
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(items.join(", "))
            }
            _ => bail!("INI entries must be strings, numbers, booleans, or lists of them"),
        }
    }
    fn entry(result: &mut String, key: &str, value: &serde_json::Value) -> Result<()> {
        if key.is_empty()
            || key.trim() != key
            || key.contains(['=', ':', '\r', '\n', '\0'])
            || key.starts_with(['#', ';', '['])
        {
            bail!("invalid INI key {key:?}");
        }
        result.push_str(&format!("{key} = {}\n", scalar(value)?));
        Ok(())
    }
    let object = value
        .as_object()
        .context("INI requires an object of entries and sections")?;
    let mut result = String::new();
    for (key, value) in object.iter().filter(|(_, value)| !value.is_object()) {
        entry(&mut result, key, value)?;
    }
    for (section, value) in object.iter().filter(|(_, value)| value.is_object()) {
        if section.is_empty() || section.contains(['[', ']', '\r', '\n', '\0']) {
            bail!("invalid INI section {section:?}");
        }
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&format!("[{section}]\n"));
        for (key, value) in value.as_object().context("INI section must be an object")? {
            entry(&mut result, key, value)?;
        }
    }
    Ok(result)
}

/// ShellCheck's rc file: a `key=value` directive per line. An array repeats its key, as
/// directives such as `enable` and `source-path` accumulate.
fn shellcheckrc(value: &Value) -> Result<String> {
    let Value::Object(fields) = value else {
        bail!("a .shellcheckrc needs a table of directives");
    };
    let mut text = String::new();
    for (key, value) in fields {
        if key.is_empty() || !key.chars().all(|ch| ch.is_ascii_alphabetic() || ch == '-') {
            bail!("invalid ShellCheck directive {key:?}; directives are letters and dashes");
        }
        let values = match value {
            Value::Array(values) => values.as_slice(),
            value => std::slice::from_ref(value),
        };
        for value in values {
            text.push_str(&format!("{key}={}\n", directive(key, value)?));
        }
    }
    Ok(text)
}

fn directive(key: &str, value: &Value) -> Result<String> {
    let text = match value {
        Value::String(text) | Value::Datetime(text) => text.clone(),
        Value::Bool(_) | Value::Int(_) | Value::Float(_) => value.to_json()?.to_string(),
        _ => bail!(
            "ShellCheck's {key} takes strings, numbers or booleans, not {}",
            value.describe()
        ),
    };
    if text.contains(['\n', '\r', '\0']) {
        bail!("ShellCheck's {key} cannot contain line breaks: {text:?}");
    }
    if !text.is_empty() && !text.contains(|ch: char| ch.is_whitespace() || "#\"'".contains(ch)) {
        return Ok(text);
    }
    // ShellCheck 0.9 and later read quoted values, which may hold spaces.
    match (text.contains('"'), text.contains('\'')) {
        (false, _) => Ok(format!("\"{text}\"")),
        (true, false) => Ok(format!("'{text}'")),
        (true, true) => bail!("ShellCheck's {key} cannot hold both kinds of quote: {text:?}"),
    }
}

/// The `.editorconfig` for a source, or `None` when it would say nothing.
pub(crate) fn editorconfig(source: &Source) -> Result<Option<String>> {
    let name = source
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut text = format!("{GENERATED_HEADER}{name}. Edit that file instead.\n");
    if source.root {
        text.push_str("root = true\n");
    }
    let mut sections = 0;
    for section in &source.sections {
        if section.properties.is_empty() {
            continue;
        }
        sections += 1;
        text.push_str(&format!("\n[{}]\n", section.glob));
        for property in &section.properties {
            let lower = property.key.to_ascii_lowercase();
            let key = if STANDARD.contains(&lower.as_str()) {
                lower
            } else {
                property.key.clone()
            };
            text.push_str(&format!(
                "{key} = {}\n",
                model::render_scalar(&property.value)?
            ));
        }
    }
    Ok((source.root || sections > 0).then_some(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_ini_sections() {
        let value = Value::from_json(&serde_json::json!({
            "sqlfluff": {"dialect": "postgres", "max_line_length": 100},
            "sqlfluff:indentation": {"tab_space_size": 4}
        }));
        assert_eq!(
            render("ini", &value, &Style::default()).unwrap(),
            "[sqlfluff]\ndialect = postgres\nmax_line_length = 100\n\n[sqlfluff:indentation]\ntab_space_size = 4\n"
        );
    }

    #[test]
    fn renders_ini_lists_and_top_level_keys() {
        let value = Value::from_json(&serde_json::json!({
            "StylesPath": ".vale/styles",
            "Vocab": ["A", "B"],
            "*.md": {
                "BasedOnStyles": ["Vale", "Local"],
                "TokenIgnores": ["(x{2,3})", "(y)"],
                "Local.Letters": "NO"
            }
        }));
        assert_eq!(
            render("ini", &value, &Style::default()).unwrap(),
            "StylesPath = .vale/styles\nVocab = A, B\n\n[*.md]\nBasedOnStyles = Vale, Local\nTokenIgnores = (x{2\\,3}), (y)\nLocal.Letters = NO\n"
        );
        for invalid in [
            serde_json::json!({"a": []}),
            serde_json::json!({"a": [["b"]]}),
            serde_json::json!({"a": [{"b": 1}]}),
            serde_json::json!({"a": [" b"]}),
        ] {
            assert!(
                render("ini", &Value::from_json(&invalid), &Style::default()).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn renders_toml_in_source_order() {
        let value = Value::from_json(&serde_json::json!({"z": 1, "a": {"b": true}, "m": "x"}));
        assert_eq!(
            render("toml", &value, &Style::default()).unwrap(),
            "z = 1\nm = \"x\"\n\n[a]\nb = true\n"
        );
    }

    #[test]
    fn renders_json_like_prettier() {
        let value = Value::from_json(&serde_json::json!({
            "files": ["**/*.razor.js"],
            "empty": {},
            "overrides": [{"files": ["a", "b"], "rules": {"x": "off"}}],
            "long": ["aaaaaaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbbbbbb", "cccccccccccccccccccc", "dddd"]
        }));
        assert_eq!(
            render("json", &value, &Style::default()).unwrap(),
            r#"{
  "files": ["**/*.razor.js"],
  "empty": {},
  "overrides": [
    {
      "files": ["a", "b"],
      "rules": {
        "x": "off"
      }
    }
  ],
  "long": [
    "aaaaaaaaaaaaaaaaaaaa",
    "bbbbbbbbbbbbbbbbbbbb",
    "cccccccccccccccccccc",
    "dddd"
  ]
}
"#
        );
    }

    #[test]
    fn renders_yaml_with_indented_sequences() {
        let value = Value::from_json(&serde_json::json!({
            "version": "v2",
            "modules": [{"path": "src/Protos", "name": "x"}],
            "lint": {"use": ["STANDARD"]},
            "ignore": ".playwright-cli/\n",
            "on": "yes",
            "count": 2,
            "empty": []
        }));
        assert_eq!(
            render("yaml", &value, &Style::default()).unwrap(),
            "version: v2\nmodules:\n  - path: src/Protos\n    name: x\nlint:\n  use:\n    - STANDARD\nignore: \".playwright-cli/\\n\"\n\"on\": \"yes\"\ncount: 2\nempty: []\n"
        );
    }

    #[test]
    fn quotes_ambiguous_yaml_strings() {
        for plain in ["v2", "relaxed", "src/Protos", "lf", "a:b", "x#y"] {
            assert!(plain_yaml(plain), "{plain}");
        }
        for quoted in [
            "",
            "yes",
            "On",
            "null",
            "~",
            "12",
            "1.2.3",
            "-1",
            ".5",
            ".inf",
            "2026-10-27",
            "*.js",
            "a: b",
            "a #b",
            "key:",
            " lead",
            "trail ",
            "line\nbreak",
            "!tag",
            "@at",
            "'q'",
        ] {
            assert!(!plain_yaml(quoted), "{quoted:?}");
        }
    }

    #[test]
    fn renders_shellcheck_directives() {
        let value = Value::from_json(&serde_json::json!({
            "shell": "bash",
            "external-sources": true,
            "enable": ["quote-safe-variables", "require-variable-braces"],
            "disable": "SC2236",
            "source-path": ["SCRIPTDIR", "My Documents/scripts", "say \"hi\""]
        }));
        assert_eq!(
            shellcheckrc(&value).unwrap(),
            "shell=bash\nexternal-sources=true\nenable=quote-safe-variables\nenable=require-variable-braces\ndisable=SC2236\nsource-path=SCRIPTDIR\nsource-path=\"My Documents/scripts\"\nsource-path='say \"hi\"'\n"
        );
        for invalid in [
            serde_json::json!({"source path": "x"}),
            serde_json::json!({"shell": {"name": "bash"}}),
            serde_json::json!({"source-path": "a\nb"}),
            serde_json::json!({"source-path": "it's \"x\""}),
        ] {
            assert!(
                shellcheckrc(&Value::from_json(&invalid)).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn renders_lines() {
        let value = Value::Array(vec![
            Value::String("dist/".into()),
            Value::String("*.map".into()),
        ]);
        assert_eq!(
            render("lines", &value, &Style::default()).unwrap(),
            "dist/\n*.map\n"
        );
    }
}
