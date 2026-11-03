//! HCL's native syntax, for tools such as tflint that read it.
//!
//! A value maps onto HCL as HCL's JSON syntax reads it: objects are block bodies, the keys under
//! a labeled block type are its labels, and everything else is an attribute. JSON leaves it to
//! the reader's schema whether an object is a block or an attribute, so the catalog says where
//! each goes, and an object it does not place is an error rather than a guess. The layout is
//! `hclfmt`'s: runs of single-line attributes align on `=`, and blocks stand apart.

use anyhow::{Result, bail};

use super::Style;
use crate::catalog::Hcl;
use crate::value::{Value, display_path};

/// An object the schema places neither as a block nor as an attribute.
#[derive(Debug)]
pub(super) struct Unplaced(String);

impl std::fmt::Display for Unplaced {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cannot tell whether {} is a block or an attribute; ToolConfig's schema does not place it",
            self.0
        )
    }
}

impl std::error::Error for Unplaced {}

pub(super) fn render(value: &Value, schema: &Hcl, style: &Style) -> Result<String> {
    let Value::Object(fields) = value else {
        bail!("an HCL file needs a table at its top level");
    };
    let mut text = String::new();
    Writer { schema, style }.body(fields, &mut Vec::new(), 0, &mut text)?;
    Ok(text)
}

enum Item {
    /// A name and its expression, which may span lines.
    Attribute(String, String),
    /// A whole block, without its final line break.
    Block(String),
}

struct Writer<'a> {
    schema: &'a Hcl,
    style: &'a Style,
}

impl Writer<'_> {
    /// Writes a body whose keys start at `path`, indented `depth` levels.
    fn body(
        &self,
        fields: &[(String, Value)],
        path: &mut Vec<String>,
        depth: usize,
        text: &mut String,
    ) -> Result<()> {
        let mut items = Vec::new();
        for (key, value) in fields {
            path.push(key.clone());
            if !identifier(key) {
                bail!(
                    "{} is not an HCL identifier, so it cannot name an attribute or block",
                    display_path(path)
                );
            }
            let objects = match value {
                Value::Object(_) => true,
                Value::Array(values) => {
                    values.iter().any(|value| matches!(value, Value::Object(_)))
                }
                _ => false,
            };
            if !objects
                || self
                    .schema
                    .attributes
                    .iter()
                    .any(|pattern| matches(pattern, path))
            {
                let column = depth * self.style.indent + key.len() + 3;
                items.push(Item::Attribute(
                    key.clone(),
                    expression(value, depth, column, self.style)?,
                ));
            } else if let Some(labels) = self.labels(path) {
                self.blocks(key, value, labels, path, depth, &mut items)?;
            } else {
                return Err(Unplaced(display_path(path)).into());
            }
            path.pop();
        }
        layout(&items, depth, self.style, text);
        Ok(())
    }

    /// The blocks of one type. `path` ends at the type, and each label is pushed as it is read.
    fn blocks(
        &self,
        header: &str,
        value: &Value,
        labels: usize,
        path: &mut Vec<String>,
        depth: usize,
        items: &mut Vec<Item>,
    ) -> Result<()> {
        if labels > 0 {
            let Value::Object(fields) = value else {
                bail!(
                    "{} needs a table keyed by its blocks' labels",
                    display_path(path)
                );
            };
            for (label, value) in fields {
                path.push(label.clone());
                let header = format!("{header} {}", quote(label));
                self.blocks(&header, value, labels - 1, path, depth, items)?;
                path.pop();
            }
            return Ok(());
        }
        // As in HCL's JSON syntax, an array holds one body per block.
        let bodies = match value {
            Value::Array(values) => values.as_slice(),
            value => std::slice::from_ref(value),
        };
        let pad = self.style.pad(depth);
        for value in bodies {
            let Value::Object(fields) = value else {
                bail!("{} needs a table as its block body", display_path(path));
            };
            if fields.is_empty() {
                items.push(Item::Block(format!("{pad}{header} {{}}")));
                continue;
            }
            let mut text = format!("{pad}{header} {{\n");
            self.body(fields, path, depth + 1, &mut text)?;
            text.push_str(&pad);
            text.push('}');
            items.push(Item::Block(text));
        }
        Ok(())
    }

    /// The labels a block type at `path` takes, when the schema makes it a block.
    fn labels(&self, path: &[String]) -> Option<usize> {
        self.schema.blocks.iter().find_map(|pattern| {
            let labels = pattern.get(path.len()..)?;
            (matches(&pattern[..path.len()], path) && labels.iter().all(|key| key == "*"))
                .then_some(labels.len())
        })
    }
}

/// Writes items one per line. Blocks stand apart from their neighbors, and each run of
/// single-line attributes aligns its `=`.
fn layout(items: &[Item], depth: usize, style: &Style, text: &mut String) {
    let pad = style.pad(depth);
    let single = |item: &Item| matches!(item, Item::Attribute(_, value) if !value.contains('\n'));
    let mut index = 0;
    while index < items.len() {
        if index > 0
            && (matches!(items[index], Item::Block(_))
                || matches!(items[index - 1], Item::Block(_)))
        {
            text.push('\n');
        }
        let run = items[index..]
            .iter()
            .take_while(|item| single(item))
            .count()
            .max(1);
        let width = items[index..index + run]
            .iter()
            .map(|item| match item {
                Item::Attribute(name, _) if run > 1 => name.len(),
                _ => 0,
            })
            .max()
            .unwrap_or(0);
        for item in &items[index..index + run] {
            match item {
                Item::Attribute(name, value) => {
                    text.push_str(&format!("{pad}{name:width$} = {value}\n"));
                }
                Item::Block(block) => {
                    text.push_str(block);
                    text.push('\n');
                }
            }
        }
        index += run;
    }
}

/// An expression for `value`, starting at `column` of a line indented `depth` levels.
fn expression(value: &Value, depth: usize, column: usize, style: &Style) -> Result<String> {
    Ok(match value {
        Value::Null => "null".to_owned(),
        Value::Bool(_) | Value::Int(_) | Value::Float(_) => value.to_json()?.to_string(),
        Value::String(text) | Value::Datetime(text) => quote(text),
        Value::Array(values) if values.is_empty() => "[]".to_owned(),
        Value::Array(values) => {
            // Short arrays of scalars stay on one line, as in the JSON layout.
            if values
                .iter()
                .all(|value| !matches!(value, Value::Array(_) | Value::Object(_)))
            {
                let inline = values
                    .iter()
                    .map(|value| expression(value, depth, 0, style))
                    .collect::<Result<Vec<_>>>()?
                    .join(", ");
                if column + inline.len() + 2 <= style.width {
                    return Ok(format!("[{inline}]"));
                }
            }
            let inner = style.pad(depth + 1);
            let mut text = "[\n".to_owned();
            for value in values {
                let column = (depth + 1) * style.indent;
                text.push_str(&format!(
                    "{inner}{},\n",
                    expression(value, depth + 1, column, style)?
                ));
            }
            text.push_str(&style.pad(depth));
            text.push(']');
            text
        }
        Value::Object(fields) if fields.is_empty() => "{}".to_owned(),
        Value::Object(fields) => {
            let mut items = Vec::new();
            for (key, value) in fields {
                let name = if identifier(key) && !KEYWORDS.contains(&key.as_str()) {
                    key.clone()
                } else {
                    quote(key)
                };
                let column = (depth + 1) * style.indent + name.len() + 3;
                items.push(Item::Attribute(
                    name,
                    expression(value, depth + 1, column, style)?,
                ));
            }
            let mut text = "{\n".to_owned();
            layout(&items, depth + 1, style, &mut text);
            text.push_str(&style.pad(depth));
            text.push('}');
            text
        }
    })
}

/// Words that an object key would read as something other than a name.
const KEYWORDS: [&str; 6] = ["true", "false", "null", "for", "in", "if"];

/// Whether `path` matches `pattern`, where `*` matches any key.
fn matches(pattern: &[String], path: &[String]) -> bool {
    pattern.len() == path.len()
        && pattern
            .iter()
            .zip(path)
            .all(|(pattern, key)| pattern == "*" || pattern == key)
}

fn identifier(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

/// A quoted string. Template sequences are escaped, so the text is taken literally.
fn quote(text: &str) -> String {
    let mut quoted = String::from("\"");
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            '$' | '%' if chars.peek() == Some(&'{') => {
                quoted.push(ch);
                quoted.push(ch);
            }
            ch if ch.is_control() => quoted.push_str(&format!("\\u{:04x}", u32::from(ch))),
            ch => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Hcl {
        let paths = |paths: &[&[&str]]| {
            paths
                .iter()
                .map(|path| path.iter().map(|key| (*key).to_owned()).collect())
                .collect()
        };
        Hcl {
            blocks: paths(&[&["config"], &["rule", "*"], &["rule", "naming", "variable"]]),
            attributes: paths(&[&["config", "ignore_module"]]),
        }
    }

    fn hcl(json: serde_json::Value) -> Result<String> {
        render(&Value::from_json(&json), &schema(), &Style::default())
    }

    #[test]
    fn renders_blocks_labels_and_attributes_like_hclfmt() {
        let text = hcl(serde_json::json!({
            "config": {
                "format": "compact",
                "call_module_type": "all",
                "ignore_module": {"terraform-aws-modules/vpc/aws": true, "local": false, "for": 1},
                "varfile": ["a.tfvars", "b.tfvars"],
                "force": false
            },
            "rule": {
                "naming": {"enabled": true, "variable": {"format": "snake_case"}},
                "empty": {}
            }
        }))
        .unwrap();
        assert_eq!(
            text,
            r#"config {
  format           = "compact"
  call_module_type = "all"
  ignore_module = {
    "terraform-aws-modules/vpc/aws" = true
    local                           = false
    "for"                           = 1
  }
  varfile = ["a.tfvars", "b.tfvars"]
  force   = false
}

rule "naming" {
  enabled = true

  variable {
    format = "snake_case"
  }
}

rule "empty" {}
"#
        );
    }

    #[test]
    fn arrays_of_bodies_repeat_blocks_and_long_arrays_wrap() {
        let long = [
            "aaaaaaaaaaaaaaaaaaaa",
            "bbbbbbbbbbbbbbbbbbbb",
            "cccccccccccccccccccc",
        ];
        let text = hcl(serde_json::json!({
            "config": [{"varfile": long}, {"force": true}]
        }))
        .unwrap();
        assert_eq!(
            text,
            "config {\n  varfile = [\n    \"aaaaaaaaaaaaaaaaaaaa\",\n    \"bbbbbbbbbbbbbbbbbbbb\",\n    \"cccccccccccccccccccc\",\n  ]\n}\n\nconfig {\n  force = true\n}\n"
        );
    }

    #[test]
    fn quotes_strings_literally() {
        assert_eq!(quote("a\"b\\c\nd\u{1}"), r#""a\"b\\c\nd\u0001""#);
        assert_eq!(quote("${var} %{if} $x %y"), r#""$${var} %%{if} $x %y""#);
    }

    #[test]
    fn refuses_what_it_cannot_place_or_name() {
        let error = hcl(serde_json::json!({"rule": {"x": {"nested": {"a": 1}}}})).unwrap_err();
        assert!(error.is::<Unplaced>(), "{error}");
        assert!(error.to_string().contains("rule.x.nested"), "{error}");
        assert!(hcl(serde_json::json!({"plugin": {"aws": {"enabled": true}}})).is_err());
        assert!(hcl(serde_json::json!({"bad key": 1})).is_err());
        assert!(hcl(serde_json::json!({"rule": {"x": 1}})).is_err());
        assert!(hcl(serde_json::json!(["not", "a", "body"])).is_err());
    }
}
