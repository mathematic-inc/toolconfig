//! TOML sources: `.toolconfig`, `.toolconfig.toml` and `.editorconfig.toml`.

use std::path::Path;

use anyhow::{Context, Result, bail};
use toml_edit::{Item, Table};

use crate::catalog;
use crate::model::{self, FileSpec, Origin, Property, Section, Settings, Source, ToolTable};
use crate::value::{self, Value};

pub(crate) fn parse(path: &Path, text: &str) -> Result<Source> {
    let document = toml_edit::Document::parse(text).map_err(|error| {
        let location = error
            .span()
            .map(|span| format!(":{}", line_of(text, span.start)))
            .unwrap_or_default();
        anyhow::anyhow!(
            "{}{location}: {}",
            path.display(),
            error.message().trim_end()
        )
    })?;
    let lines = Lines { path, text };
    let dir = path.parent().context("a source needs a directory")?;
    let mut source = Source {
        path: path.to_path_buf(),
        dir: dir.to_path_buf(),
        root: false,
        settings: Settings::default(),
        sections: Vec::new(),
    };
    let root = document.as_table();
    let mut sections = Vec::new();
    for (key, item) in root.iter() {
        let key_repr = root.key(key);
        let origin = lines.origin(key_repr.and_then(toml_edit::Key::span));
        match key {
            "root" if !quoted(text, key_repr) => {
                source.root = item
                    .as_bool()
                    .with_context(|| format!("{origin}: root must be true or false"))?;
            }
            "toolconfig" if !quoted(text, key_repr) => {
                let table = table_like(item)
                    .with_context(|| format!("{origin}: [toolconfig] must be a table"))?;
                source.settings = settings(&lines, table)?;
            }
            _ if !quoted(text, key_repr) => bail!(
                "{origin}: section headers must be quoted, as in [\"{key}\"]; an unquoted {key:?} is not a glob"
            ),
            glob => {
                let table = table_like(item).with_context(|| {
                    format!("{origin}: section {glob:?} must be a table, as in [\"{glob}\"]")
                })?;
                let mut headers = Vec::new();
                collect_headers(item, &mut headers);
                let start = headers
                    .iter()
                    .copied()
                    .chain(
                        key_repr
                            .and_then(toml_edit::Key::span)
                            .map(|span| span.start),
                    )
                    .min();
                let section = section(&lines, glob, table, origin)?;
                sections.push((start.unwrap_or(usize::MAX), headers, section));
            }
        }
    }
    check_contiguous(&lines, &sections)?;
    sections.sort_by_key(|(start, _, _)| *start);
    source.sections = sections
        .into_iter()
        .map(|(_, _, section)| section)
        .collect();
    source.validate()?;
    Ok(source)
}

struct Lines<'a> {
    path: &'a Path,
    text: &'a str,
}

impl Lines<'_> {
    fn origin(&self, span: Option<std::ops::Range<usize>>) -> Origin {
        Origin::new(self.path, span.map(|span| line_of(self.text, span.start)))
    }
}

fn line_of(text: &str, offset: usize) -> usize {
    text.as_bytes()[..offset.min(text.len())]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
}

/// Whether a key was written quoted. Parsed keys carry spans into the source text.
fn quoted(text: &str, key: Option<&toml_edit::Key>) -> bool {
    key.and_then(toml_edit::Key::span)
        .and_then(|span| text.get(span))
        .is_some_and(|raw| raw.starts_with(['"', '\'']))
}

/// A view over either a `[table]` or an `{ inline = "table" }`.
enum TableLike<'a> {
    Table(&'a Table),
    Inline(&'a toml_edit::InlineTable),
}

impl<'a> TableLike<'a> {
    fn entries(&self) -> Vec<(&'a str, Option<&'a toml_edit::Key>, Entry<'a>)> {
        match self {
            Self::Table(table) => table
                .iter()
                .map(|(key, item)| (key, table.key(key), Entry::Item(item)))
                .collect(),
            Self::Inline(table) => table
                .iter()
                .map(|(key, value)| (key, table.key(key), Entry::Value(value)))
                .collect(),
        }
    }
}

#[derive(Clone, Copy)]
enum Entry<'a> {
    Item(&'a Item),
    Value(&'a toml_edit::Value),
}

impl<'a> Entry<'a> {
    fn table_like(self) -> Option<TableLike<'a>> {
        match self {
            Self::Item(item) => table_like(item),
            Self::Value(toml_edit::Value::InlineTable(table)) => Some(TableLike::Inline(table)),
            Self::Value(_) => None,
        }
    }
    fn span(self) -> Option<std::ops::Range<usize>> {
        match self {
            Self::Item(item) => item.span(),
            Self::Value(value) => value.span(),
        }
    }
}

fn table_like(item: &Item) -> Option<TableLike<'_>> {
    match item {
        Item::Table(table) => Some(TableLike::Table(table)),
        Item::Value(toml_edit::Value::InlineTable(table)) => Some(TableLike::Inline(table)),
        _ => None,
    }
}

/// Offsets of every `[header]` and `[[header]]` inside a section, for ordering and contiguity.
fn collect_headers(item: &Item, headers: &mut Vec<usize>) {
    match item {
        Item::Table(table) => {
            if !table.is_implicit()
                && !table.is_dotted()
                && let Some(span) = table.span()
            {
                headers.push(span.start);
            }
            for (_, item) in table.iter() {
                collect_headers(item, headers);
            }
        }
        Item::ArrayOfTables(array) => {
            for table in array.iter() {
                if let Some(span) = table.span() {
                    headers.push(span.start);
                }
                for (_, item) in table.iter() {
                    collect_headers(item, headers);
                }
            }
        }
        _ => {}
    }
}

fn check_contiguous(lines: &Lines<'_>, sections: &[(usize, Vec<usize>, Section)]) -> Result<()> {
    let mut headers = sections
        .iter()
        .enumerate()
        .flat_map(|(index, (_, headers, _))| headers.iter().map(move |offset| (*offset, index)))
        .collect::<Vec<_>>();
    headers.sort_unstable();
    let mut finished = vec![false; sections.len()];
    let mut current = None;
    for (offset, index) in headers {
        if current != Some(index) {
            if finished[index] {
                let section = &sections[index].2;
                bail!(
                    "{}:{}: section {:?} continues after another section; keep each section's tables together, because section order decides precedence",
                    lines.path.display(),
                    line_of(lines.text, offset),
                    section.glob
                );
            }
            if let Some(previous) = current {
                finished[previous] = true;
            }
            current = Some(index);
        }
    }
    Ok(())
}

fn settings(lines: &Lines<'_>, table: TableLike<'_>) -> Result<Settings> {
    let mut settings = Settings::default();
    for (key, key_repr, entry) in table.entries() {
        let origin = lines.origin(key_repr.and_then(toml_edit::Key::span));
        match key {
            "tools" => {
                let Entry::Item(Item::Value(toml_edit::Value::Array(array))) = entry else {
                    bail!("{origin}: tools must be an array of tool names");
                };
                for value in array.iter() {
                    let id = value
                        .as_str()
                        .with_context(|| format!("{origin}: tools must contain only tool names"))?;
                    settings
                        .tools
                        .push((id.to_owned(), lines.origin(value.span())));
                }
            }
            "files" => {
                let files = entry
                    .table_like()
                    .with_context(|| format!("{origin}: files must be a table of outputs"))?;
                for (label, label_key, entry) in files.entries() {
                    let origin = lines.origin(label_key.and_then(toml_edit::Key::span));
                    let file = entry
                        .table_like()
                        .with_context(|| format!("{origin}: file {label:?} must be a table"))?;
                    settings.files.push(file_spec(lines, label, file, origin)?);
                }
            }
            id => {
                if catalog::get(id).is_none() {
                    bail!(
                        "{origin}: unknown [toolconfig] key {id:?}; expected tools, files, or a tool name{}",
                        model::suggestion(
                            id,
                            catalog::all().iter().map(|builtin| builtin.id.as_str())
                        )
                    );
                }
                let table = entry
                    .table_like()
                    .with_context(|| format!("{origin}: [toolconfig.{id}] must be a table"))?;
                let mut tool = model::ToolSettings::default();
                for (key, key_repr, entry) in table.entries() {
                    let origin = lines.origin(key_repr.and_then(toml_edit::Key::span));
                    let value = convert(lines, entry, &mut Vec::new(), &mut Vec::new())?;
                    match key {
                        "format" => tool.format = Some(string(&value, &origin, "format")?),
                        "path" => tool.path = Some(string(&value, &origin, "path")?),
                        "skip" => {
                            let Value::Array(values) = value else {
                                bail!("{origin}: skip must be an array of property names");
                            };
                            for value in values {
                                tool.skip.push(string(&value, &origin, "skip")?);
                            }
                        }
                        other => bail!(
                            "{origin}: unknown key {other:?} in [toolconfig.{id}]; expected format, path or skip"
                        ),
                    }
                }
                settings.tool.insert(id.to_owned(), tool);
            }
        }
    }
    Ok(settings)
}

fn file_spec(
    lines: &Lines<'_>,
    label: &str,
    table: TableLike<'_>,
    origin: Origin,
) -> Result<FileSpec> {
    let mut path = None;
    let mut format = None;
    let mut config = None;
    for (key, key_repr, entry) in table.entries() {
        let key_origin = lines.origin(key_repr.and_then(toml_edit::Key::span));
        let value = convert(lines, entry, &mut Vec::new(), &mut Vec::new())?;
        match key {
            "path" => path = Some(string(&value, &key_origin, "path")?),
            "format" => format = Some(string(&value, &key_origin, "format")?),
            "config" => config = Some(value),
            other => bail!(
                "{key_origin}: unknown key {other:?} in file {label:?}; expected path, format or config"
            ),
        }
    }
    Ok(FileSpec {
        label: label.to_owned(),
        path: path.with_context(|| format!("{origin}: file {label:?} needs a path"))?,
        format: format.unwrap_or_else(|| "json".to_owned()),
        config: config.unwrap_or_else(Value::object),
        origin,
    })
}

fn string(value: &Value, origin: &Origin, name: &str) -> Result<String> {
    value
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("{origin}: {name} must be a string"))
}

fn section(lines: &Lines<'_>, glob: &str, table: TableLike<'_>, origin: Origin) -> Result<Section> {
    let mut section = Section::new(glob.to_owned(), origin)?;
    for (key, key_repr, entry) in table.entries() {
        let origin = lines.origin(key_repr.and_then(toml_edit::Key::span));
        if catalog::get(key).is_some() {
            let mut origins = Vec::new();
            let value = convert(lines, entry, &mut Vec::new(), &mut origins)?;
            if !matches!(value, Value::Object(_)) {
                bail!("{origin}: {key} options must be a table, as in {key}.option = value");
            }
            section.tools.push(ToolTable {
                id: key.to_owned(),
                value,
                origins,
            });
            continue;
        }
        if let Some(example) = catalog::reads_editorconfig(key)
            && entry.table_like().is_some()
        {
            bail!(
                "{origin}: {key} reads .editorconfig properties directly; write them as plain keys, such as {example}"
            );
        }
        let mut origins = Vec::new();
        let value = convert(lines, entry, &mut Vec::new(), &mut origins)?;
        match value {
            Value::Object(_) => {
                // Dotted keys such as dotnet_diagnostic.CA1822.severity flatten back to one key.
                for (path, leaf) in value.leaves() {
                    let leaf_origin = origins
                        .iter()
                        .find(|(leaf_path, _)| leaf_path == &path)
                        .map_or_else(|| origin.clone(), |(_, origin)| origin.clone());
                    let mut full = vec![key.to_owned()];
                    full.extend(path);
                    section.properties.push(Property {
                        key: full.join("."),
                        value: leaf.clone(),
                        origin: leaf_origin,
                    });
                }
            }
            value => section.properties.push(Property {
                key: key.to_owned(),
                value,
                origin,
            }),
        }
    }
    Ok(section)
}

/// Converts a TOML entry, recording the origin of each leaf below `path`.
fn convert(
    lines: &Lines<'_>,
    entry: Entry<'_>,
    path: &mut value::Path,
    origins: &mut Vec<(value::Path, Origin)>,
) -> Result<Value> {
    if let Some(table) = entry.table_like() {
        let mut fields = Vec::new();
        for (key, key_repr, entry) in table.entries() {
            path.push(key.to_owned());
            if entry.table_like().is_none() {
                origins.push((
                    path.clone(),
                    lines.origin(
                        key_repr
                            .and_then(toml_edit::Key::span)
                            .or_else(|| entry.span()),
                    ),
                ));
            }
            let value = convert(lines, entry, path, origins)?;
            path.pop();
            fields.push((key.to_owned(), value));
        }
        return Ok(Value::Object(fields));
    }
    match entry {
        Entry::Item(Item::ArrayOfTables(array)) => {
            let mut values = Vec::new();
            for table in array.iter() {
                values.push(convert(
                    lines,
                    Entry::Item(&Item::Table(table.clone())),
                    &mut path.clone(),
                    &mut Vec::new(),
                )?);
            }
            Ok(Value::Array(values))
        }
        Entry::Item(Item::Value(value)) | Entry::Value(value) => scalar(lines, value),
        Entry::Item(Item::None) => Ok(Value::Null),
        Entry::Item(Item::Table(_)) => unreachable!("tables are handled above"),
    }
}

fn scalar(lines: &Lines<'_>, value: &toml_edit::Value) -> Result<Value> {
    Ok(match value {
        toml_edit::Value::String(value) => Value::String(value.value().clone()),
        toml_edit::Value::Integer(value) => Value::Int(*value.value()),
        toml_edit::Value::Float(value) => Value::Float(*value.value()),
        toml_edit::Value::Boolean(value) => Value::Bool(*value.value()),
        toml_edit::Value::Datetime(value) => Value::Datetime(value.value().to_string()),
        toml_edit::Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| scalar(lines, value))
                .collect::<Result<_>>()?,
        ),
        toml_edit::Value::InlineTable(_) => {
            convert(lines, Entry::Value(value), &mut Vec::new(), &mut Vec::new())?
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_text(text: &str) -> Result<Source> {
        parse(Path::new("/project/.toolconfig.toml"), text)
    }

    #[test]
    fn parses_sections_tools_and_properties() {
        let source = parse_text(
            r#"
root = true

[toolconfig]
tools = ["rustfmt", "ruff"]
ruff.skip = ["max_line_length"]

["*"]
indent_style = "space"
indent_size = 2
dotnet_diagnostic.CA1822.severity = "warning"

["*.rs"]
indent_size = 4
rustfmt.edition = "2024"

["*.py".ruff.lint]
select = ["E", "F"]
"#,
        )
        .unwrap();
        assert!(source.root);
        assert_eq!(source.settings.tools.len(), 2);
        assert_eq!(source.settings.tool["ruff"].skip, ["max_line_length"]);
        let globs = source
            .sections
            .iter()
            .map(|section| section.glob.as_str())
            .collect::<Vec<_>>();
        assert_eq!(globs, ["*", "*.rs", "*.py"]);
        let star = &source.sections[0];
        assert_eq!(star.properties[2].key, "dotnet_diagnostic.CA1822.severity");
        assert_eq!(star.properties[2].origin.line, Some(11));
        assert_eq!(source.sections[1].tools[0].id, "rustfmt");
        assert_eq!(
            source.sections[2].tools[0].value.describe(),
            r#"{"lint":{"select":["E","F"]}}"#
        );
    }

    #[test]
    fn rejects_unquoted_headers() {
        let error = parse_text("[Makefile]\nindent_style = \"tab\"\n").unwrap_err();
        assert!(error.to_string().contains("must be quoted"), "{error}");
    }

    #[test]
    fn rejects_split_sections() {
        let error = parse_text(
            "[\"*.py\"]\nindent_size = 4\n\n[\"*.md\"]\nindent_size = 2\n\n[\"*.py\".ruff]\nline-length = 1\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("continues after"), "{error}");
    }

    #[test]
    fn orders_sections_by_first_definition() {
        let source =
            parse_text("[\"*.py\".ruff.lint]\nselect = []\n\n[\"*\"]\nindent_size = 2\n").unwrap();
        assert_eq!(source.sections[0].glob, "*.py");
        assert_eq!(source.sections[1].glob, "*");
    }

    #[test]
    fn rejects_invalid_standard_values() {
        let error = parse_text("[\"*\"]\nindent_style = \"spaces\"\n").unwrap_err();
        assert!(
            format!("{error:#}").contains("indent_style must be"),
            "{error:#}"
        );
        let error = parse_text("[\"*\"]\nIndent_Size = 2\n").unwrap_err();
        assert!(error.to_string().contains("lowercase"), "{error}");
    }

    #[test]
    fn rejects_unknown_tools_with_suggestions() {
        let error = parse_text("[toolconfig]\ntools = [\"rustmft\"]\n").unwrap_err();
        assert!(
            error.to_string().contains("did you mean \"rustfmt\""),
            "{error}"
        );
        let error = parse_text("[toolconfig]\ntools = [\"shfmt\"]\n").unwrap_err();
        assert!(error.to_string().contains("reads .editorconfig"), "{error}");
    }

    #[test]
    fn arrays_of_tables_inside_sections() {
        let source = parse_text(
            "[\"*\".oxlint]\n[[\"*\".oxlint.overrides]]\nfiles = [\"*.razor.js\"]\nrules = { \"unicorn/filename-case\" = \"off\" }\n",
        )
        .unwrap();
        assert_eq!(
            source.sections[0].tools[0].value.describe(),
            r#"{"overrides":[{"files":["*.razor.js"],"rules":{"unicorn/filename-case":"off"}}]}"#
        );
    }
}
