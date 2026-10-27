//! One native configuration file per enabled tool, from the sections that apply to its files.
//!
//! EditorConfig resolves values per file, while native files cover a directory. Each tool is
//! resolved at a probe path per extension it handles; values that agree go at the top level,
//! values that differ go into native overrides. Sections that match only some files are checked
//! through witness paths, so a native file never silently disagrees with `.editorconfig`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::cascade::{self, Provenance, Resolved};
use crate::catalog::{Builtin, Mapping};
use crate::model::{Section, Source, ToolSettings};
use crate::paths;
use crate::project::Project;
use crate::value::{self, Value, display_path};

const PROBE: &str = "__toolconfig_probe__";
const WITNESS: &str = "__w__";

pub(crate) struct ToolOutput {
    pub(crate) path: PathBuf,
    pub(crate) format: String,
    pub(crate) value: Value,
    pub(crate) explanation: Vec<Explained>,
    pub(crate) alternatives: Vec<PathBuf>,
}

/// One native key and where its value came from, for `explain`.
pub(crate) struct Explained {
    pub(crate) key: String,
    pub(crate) value: String,
    pub(crate) from: String,
    /// The files an override applies to; `None` for top-level keys.
    pub(crate) files: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Default,
    /// From a standard property. A file without the property places no constraint.
    Projected,
    /// From a tool table. A file without the key gets the tool's default.
    Tool,
}

#[derive(Clone, Debug)]
struct Leaf {
    value: Value,
    kind: Kind,
    from: String,
}

/// Leaves in the order they will be written.
type Leaves = Vec<(value::Path, Leaf)>;

fn find<'a>(leaves: &'a Leaves, path: &[String]) -> Option<&'a Leaf> {
    leaves
        .iter()
        .find(|(leaf, _)| leaf == path)
        .map(|(_, leaf)| leaf)
}

struct Context<'a> {
    project: &'a Project,
    builtin: &'a Builtin,
    settings: ToolSettings,
    dir: &'a Path,
    filename: String,
}

pub(crate) fn compile(
    project: &Project,
    source: &Source,
    builtin: &Builtin,
    warnings: &mut BTreeSet<String>,
) -> Result<ToolOutput> {
    let settings = source
        .settings
        .tool
        .get(&builtin.id)
        .cloned()
        .unwrap_or_default();
    let format = settings
        .format
        .clone()
        .unwrap_or_else(|| builtin.default.clone());
    let filename = match &settings.path {
        Some(path) => path.clone(),
        None => builtin.formats[&format].clone(),
    };
    let context = Context {
        project,
        builtin,
        settings,
        dir: &source.dir,
        filename: filename.clone(),
    };
    let extensions: Vec<Option<String>> = if builtin.extensions.is_empty() {
        vec![None]
    } else {
        builtin.extensions.iter().cloned().map(Some).collect()
    };
    let mut probes = Vec::new();
    for extension in &extensions {
        let probe = match extension {
            Some(extension) => source.dir.join(format!("{PROBE}.{extension}")),
            None => source.dir.join(PROBE),
        };
        let leaves = context.evaluate(&probe, &describe_files(extension.as_deref()), warnings)?;
        probes.push((extension.clone(), leaves));
    }
    let (base, groups) = context.group(&probes)?;
    context.check_witnesses(source, &probes, &base, &groups, warnings)?;

    let mut value = Value::object();
    let mut explanation = Vec::new();
    for (path, leaf) in &base {
        value.set_path(path, leaf.value.clone()).map_err(|error| {
            anyhow::anyhow!(
                "{}: cannot set {}: {error:#}",
                builtin.id,
                display_path(path)
            )
        })?;
        explanation.push(Explained {
            key: display_path(path),
            value: leaf.value.describe(),
            from: leaf.from.clone(),
            files: None,
        });
    }
    if !groups.is_empty() {
        let mut entries = Vec::new();
        for group in &groups {
            let mut options = Value::object();
            for (path, leaf) in &group.leaves {
                options.set_path(path, leaf.value.clone())?;
                explanation.push(Explained {
                    key: display_path(path),
                    value: leaf.value.describe(),
                    from: leaf.from.clone(),
                    files: Some(group.files.join(", ")),
                });
            }
            entries.push(Value::Object(vec![
                (
                    "files".to_owned(),
                    Value::Array(group.files.iter().cloned().map(Value::String).collect()),
                ),
                ("options".to_owned(), options),
            ]));
        }
        // Source overrides come last so they keep precedence over generated ones.
        if let Some(Value::Array(existing)) = value.get("overrides") {
            entries.extend(existing.iter().cloned());
        }
        value.set_path(&["overrides".to_owned()], Value::Array(entries))?;
    }
    let alternatives = builtin
        .alternatives
        .iter()
        .filter(|alternative| **alternative != filename)
        .map(|alternative| source.dir.join(alternative))
        .collect();
    Ok(ToolOutput {
        path: source.dir.join(&filename),
        format,
        value,
        explanation,
        alternatives,
    })
}

fn describe_files(extension: Option<&str>) -> String {
    match extension {
        Some(extension) => format!("*.{extension} files"),
        None => "every file".to_owned(),
    }
}

struct Group {
    files: Vec<String>,
    extensions: Vec<Option<String>>,
    leaves: Leaves,
}

impl Context<'_> {
    fn root(&self) -> &Path {
        &self.project.root
    }

    fn skipped(&self, property: &str) -> bool {
        self.settings.skip.iter().any(|skip| {
            skip == "*"
                || skip == property
                || (property == "indent_width" && (skip == "indent_size" || skip == "tab_width"))
        })
    }

    fn native_name(&self) -> String {
        let path = self.dir.join(&self.filename);
        paths::relative(self.root(), &path)
    }

    /// The leaves a tool receives for one file: defaults, projected properties, tool tables.
    fn evaluate(
        &self,
        path: &Path,
        files: &str,
        warnings: &mut BTreeSet<String>,
    ) -> Result<Leaves> {
        let resolved = cascade::resolve(self.project, path);
        let mut leaves = Leaves::new();
        if let Some(defaults) = &self.builtin.config {
            for (path, leaf) in Value::from_json(defaults).leaves() {
                leaves.push((
                    path,
                    Leaf {
                        value: leaf.clone(),
                        kind: Kind::Default,
                        from: format!("{} default", self.builtin.id),
                    },
                ));
            }
        }
        let normalized = normalize(&resolved);
        let mut projected = Leaves::new();
        for mapping in &self.builtin.projection {
            if self.skipped(&mapping.property) {
                continue;
            }
            let Some((value, description, provenance)) = normalized.get(mapping.property.as_str())
            else {
                continue;
            };
            let from = format!(
                "{} {description}",
                provenance
                    .iter()
                    .map(|provenance| provenance.display(self.root()))
                    .collect::<Vec<_>>()
                    .join(" + ")
            );
            let Some(native) = self.native(mapping, value, description, &from)? else {
                continue;
            };
            projected.retain(|(existing, _)| existing != &mapping.path);
            projected.push((
                mapping.path.clone(),
                Leaf {
                    value: native,
                    kind: Kind::Projected,
                    from,
                },
            ));
        }
        let mut tool = Leaves::new();
        if let Some(resolution) = resolved.tools.get(&self.builtin.id)
            && let Some(value) = &resolution.value
        {
            for (path, leaf) in value.leaves() {
                let from = resolution
                    .provenance
                    .get(&path)
                    .map(|provenance| provenance.display(self.root()))
                    .unwrap_or_default();
                tool.push((
                    path.clone(),
                    Leaf {
                        value: leaf.clone(),
                        kind: Kind::Tool,
                        from: format!("{from} {}.{}", self.builtin.id, display_path(&path)),
                    },
                ));
            }
        }
        let mut redundant = BTreeSet::new();
        for (projected_path, projected_leaf) in &projected {
            for (tool_path, tool_leaf) in &tool {
                if tool_path == projected_path {
                    if tool_leaf.value == projected_leaf.value {
                        warnings.insert(format!(
                            "{}: {}.{} repeats a value that {} already provides; remove it from the tool table",
                            tool_leaf.from.split(' ').next().unwrap_or_default(),
                            self.builtin.id,
                            display_path(tool_path),
                            projected_leaf.from
                        ));
                        redundant.insert(projected_path.clone());
                    } else {
                        bail!(
                            "{id}.{key} is set twice for {files}\n  {tool} = {tool_value}\n  {projected}\nhelp: change one of them, or add the property to [toolconfig.{id}] skip in {source}",
                            id = self.builtin.id,
                            key = display_path(tool_path),
                            tool = tool_leaf.from,
                            tool_value = tool_leaf.value.describe(),
                            projected = projected_leaf.from,
                            source = self.native_dir_source(),
                        );
                    }
                } else if tool_path.starts_with(projected_path)
                    || projected_path.starts_with(tool_path)
                {
                    bail!(
                        "{id} cannot combine {tool} with {projected}: {tool_key} and {projected_key} overlap\nhelp: change one of them, or add the property to [toolconfig.{id}] skip in {source}",
                        id = self.builtin.id,
                        tool = tool_leaf.from,
                        projected = projected_leaf.from,
                        tool_key = display_path(tool_path),
                        projected_key = display_path(projected_path),
                        source = self.native_dir_source(),
                    );
                }
            }
        }
        for (path, leaf) in projected {
            if !redundant.contains(&path) {
                leaves.retain(|(existing, _)| existing != &path);
                leaves.push((path, leaf));
            }
        }
        for (path, leaf) in tool {
            leaves.retain(|(existing, _)| {
                !(existing == &path || existing.starts_with(&path) || path.starts_with(existing))
            });
            leaves.push((path, leaf));
        }
        Ok(leaves)
    }

    fn native_dir_source(&self) -> String {
        self.project
            .source_in(self.dir)
            .map(|source| paths::relative(self.root(), &source.path))
            .unwrap_or_default()
    }

    fn native(
        &self,
        mapping: &Mapping,
        value: &Value,
        description: &str,
        from: &str,
    ) -> Result<Option<Value>> {
        let native = match &mapping.values {
            Some(values) => {
                let key = match value {
                    Value::String(text) => text.clone(),
                    other => other.describe(),
                };
                match values.get(&key) {
                    None => bail!(
                        "{} cannot express {description} ({from})\nhelp: change the property, or add {:?} to [toolconfig.{}] skip",
                        self.builtin.id,
                        standard_name(&mapping.property),
                        self.builtin.id
                    ),
                    Some(serde_json::Value::Null) => return Ok(None),
                    Some(native) => Value::from_json(native),
                }
            }
            None => value.clone(),
        };
        if let (Some(max), Value::Int(number)) = (mapping.max, &native)
            && *number > max
        {
            bail!(
                "{} accepts at most {max} for {}, but {from} sets {number}",
                self.builtin.id,
                display_path(&mapping.path)
            );
        }
        Ok(Some(native))
    }

    /// Splits probe results into top-level leaves and per-extension override groups.
    ///
    /// A file without a value gets the tool's default. The most common value goes at the top
    /// level, unless some files rely on a default the catalog does not know; files whose value
    /// differs get an override.
    fn group(&self, probes: &[(Option<String>, Leaves)]) -> Result<(Leaves, Vec<Group>)> {
        let mut order = Vec::<value::Path>::new();
        for (_, leaves) in probes {
            for (path, _) in leaves {
                if !order.contains(path) {
                    order.push(path.clone());
                }
            }
        }
        let mut base = Leaves::new();
        let mut overrides = vec![Leaves::new(); probes.len()];
        let mut differing = None;
        for path in order {
            let leaves = probes
                .iter()
                .map(|(_, leaves)| find(leaves, &path))
                .collect::<Vec<_>>();
            let default = self.default_for(&path);
            let effective = leaves
                .iter()
                .map(|leaf| {
                    leaf.map(|leaf| leaf.value.clone())
                        .or_else(|| default.clone())
                })
                .collect::<Vec<_>>();
            let winner = if effective.iter().any(Option::is_none) {
                None
            } else {
                let mut counts = Vec::<(&Value, usize)>::new();
                for value in effective.iter().flatten() {
                    match counts.iter_mut().find(|(known, _)| *known == value) {
                        Some((_, count)) => *count += 1,
                        None => counts.push((value, 1)),
                    }
                }
                counts
                    .iter()
                    .enumerate()
                    .max_by_key(|(index, (_, count))| (*count, std::cmp::Reverse(*index)))
                    .map(|(_, (value, _))| (*value).clone())
            };
            // A winning value that only comes from defaults stays implicit.
            if let Some(winner) = &winner
                && let Some(leaf) = leaves.iter().flatten().find(|leaf| leaf.value == *winner)
            {
                base.push((path.clone(), (*leaf).clone()));
            }
            for (index, leaf) in leaves.iter().enumerate() {
                let value = &effective[index];
                if value.is_none() || *value == winner || (winner.is_none() && leaf.is_none()) {
                    continue;
                }
                let leaf = match leaf {
                    Some(leaf) => (*leaf).clone(),
                    None => Leaf {
                        value: default.clone().unwrap_or(Value::Null),
                        kind: Kind::Default,
                        from: format!("{} default", self.builtin.id),
                    },
                };
                overrides[index].push((path.clone(), leaf));
                differing.get_or_insert_with(|| path.clone());
            }
        }
        let Some(path) = differing else {
            return Ok((base, Vec::new()));
        };
        if !self.builtin.overrides {
            let details = probes
                .iter()
                .map(|(extension, leaves)| {
                    let files = describe_files(extension.as_deref());
                    match find(leaves, &path) {
                        Some(leaf) => {
                            format!("  {files}: {} from {}", leaf.value.describe(), leaf.from)
                        }
                        None => format!("  {files}: the {} default", self.builtin.id),
                    }
                })
                .collect::<Vec<_>>();
            bail!(
                "{} cannot vary {} by file type, but its files disagree:\n{}\nhelp: make the values agree for {}, or add the property to [toolconfig.{}] skip",
                self.native_name(),
                display_path(&path),
                details.join("\n"),
                self.builtin
                    .extensions
                    .iter()
                    .map(|extension| format!("*.{extension}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                self.builtin.id
            );
        }
        let mut groups = Vec::<Group>::new();
        for ((extension, _), leaves) in probes.iter().zip(overrides) {
            if leaves.is_empty() {
                continue;
            }
            let pattern = match extension {
                Some(extension) => format!("**/*.{extension}"),
                None => "**/*".to_owned(),
            };
            let same = |group: &&mut Group| {
                group.leaves.len() == leaves.len()
                    && group
                        .leaves
                        .iter()
                        .zip(&leaves)
                        .all(|(left, right)| left.0 == right.0 && left.1.value == right.1.value)
            };
            match groups.iter_mut().find(same) {
                Some(group) => {
                    group.files.push(pattern);
                    group.extensions.push(extension.clone());
                }
                None => groups.push(Group {
                    files: vec![pattern],
                    extensions: vec![extension.clone()],
                    leaves,
                }),
            }
        }
        Ok((base, groups))
    }

    /// The tool's own default for a native key, when the catalog records it.
    fn default_for(&self, path: &[String]) -> Option<Value> {
        self.builtin
            .projection
            .iter()
            .find(|mapping| mapping.path == path && mapping.default.is_some())
            .and_then(|mapping| mapping.default.as_ref())
            .map(Value::from_json)
    }

    /// The leaves a file with `extension` effectively receives from the native file.
    fn effective(&self, extension: Option<&str>, base: &Leaves, groups: &[Group]) -> Leaves {
        let mut leaves = base.clone();
        let extension = extension.map(str::to_ascii_lowercase);
        if let Some(group) = groups.iter().find(|group| {
            group
                .extensions
                .iter()
                .any(|known| known.as_deref().map(str::to_ascii_lowercase) == extension)
        }) {
            for (path, leaf) in &group.leaves {
                leaves.retain(|(existing, _)| existing != path);
                leaves.push((path.clone(), leaf.clone()));
            }
        }
        leaves
    }

    /// Checks files that sections single out, which a probe per extension cannot see.
    fn check_witnesses(
        &self,
        owner: &Source,
        probes: &[(Option<String>, Leaves)],
        base: &Leaves,
        groups: &[Group],
        warnings: &mut BTreeSet<String>,
    ) -> Result<()> {
        let dir = &owner.dir;
        // Subdirectories with their own native file for this tool cover themselves.
        let covered: Vec<PathBuf> = self
            .project
            .sources
            .iter()
            .filter(|source| {
                source.dir != *dir
                    && source.dir.starts_with(dir)
                    && source
                        .settings
                        .tools
                        .iter()
                        .any(|(id, _)| *id == self.builtin.id)
            })
            .map(|source| source.dir.clone())
            .collect();
        if !self.builtin.nearest
            && let Some(nested) = covered.first()
        {
            bail!(
                "{} reads one configuration file per run, so it cannot be enabled in both {} and {}",
                self.builtin.id,
                paths::relative(self.root(), dir),
                paths::relative(self.root(), nested)
            );
        }
        let mut seen = BTreeSet::new();
        for source in &self.project.sources {
            let relevant = dir.starts_with(&source.dir) || source.dir.starts_with(dir);
            if !relevant
                || covered
                    .iter()
                    .any(|covered| source.dir.starts_with(covered))
            {
                continue;
            }
            for section in &source.sections {
                for witness in witness_paths(source, section, dir) {
                    if covered.iter().any(|covered| witness.starts_with(covered))
                        || !seen.insert(witness.clone())
                    {
                        continue;
                    }
                    let extension = witness
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .map(str::to_owned);
                    if !self.builtin.handles(extension.as_deref()) {
                        continue;
                    }
                    let lookup = if self.builtin.extensions.is_empty() {
                        None
                    } else {
                        extension.clone()
                    };
                    if !probes.iter().any(|(probe, _)| {
                        probe.as_deref().map(str::to_ascii_lowercase)
                            == lookup.as_deref().map(str::to_ascii_lowercase)
                    }) {
                        continue;
                    }
                    let actual = self.evaluate(&witness, "these files", warnings)?;
                    let expected = self.effective(lookup.as_deref(), base, groups);
                    if let Some(problem) = mismatch(&actual, &expected) {
                        bail!(
                            "{native} cannot follow section {glob:?} ({origin}): {problem}\nhelp: {help}",
                            native = self.native_name(),
                            glob = section.glob,
                            origin = section.origin.display(self.root()),
                            help = self.witness_help(source, section),
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn witness_help(&self, source: &Source, section: &Section) -> String {
        let literal = section
            .glob
            .trim_start_matches('/')
            .split(['*', '?', '[', '{'])
            .next()
            .unwrap_or_default();
        let directory = literal.rsplit_once('/').map(|(directory, _)| directory);
        let skip = format!(
            "add the property to [toolconfig.{}] skip in {}",
            self.builtin.id,
            self.native_dir_source()
        );
        match directory {
            Some(directory) if !directory.is_empty() => format!(
                "add a source in {} that lists {} in tools, or {skip}",
                paths::relative(self.root(), &source.dir.join(directory)),
                self.builtin.id
            ),
            _ if source.dir != self.dir && source.dir.starts_with(self.dir) => format!(
                "list {} in tools in {}, or {skip}",
                self.builtin.id,
                paths::relative(self.root(), &source.path)
            ),
            _ => format!("make the values agree for every file the tool handles, or {skip}"),
        }
    }
}

fn mismatch(actual: &Leaves, expected: &Leaves) -> Option<String> {
    for (path, leaf) in actual {
        if leaf.kind == Kind::Default {
            continue;
        }
        match find(expected, path) {
            Some(expected) if expected.value == leaf.value => {}
            Some(expected) => {
                return Some(format!(
                    "those files get {} = {} from {}, but the native file applies {} from {}",
                    display_path(path),
                    leaf.value.describe(),
                    leaf.from,
                    expected.value.describe(),
                    expected.from
                ));
            }
            None => {
                return Some(format!(
                    "those files get {} = {} from {}, but the native file leaves it unset",
                    display_path(path),
                    leaf.value.describe(),
                    leaf.from
                ));
            }
        }
    }
    for (path, leaf) in expected {
        if leaf.kind == Kind::Tool && find(actual, path).is_none() {
            return Some(format!(
                "those files do not get {} = {}, but the native file applies it from {}",
                display_path(path),
                leaf.value.describe(),
                leaf.from
            ));
        }
    }
    None
}

/// Normalized property values with a description and provenance for diagnostics.
fn normalize(resolved: &Resolved) -> BTreeMap<&'static str, (Value, String, Vec<Provenance>)> {
    let get = |key: &str| resolved.properties.get(key);
    let describe = |key: &str| get(key).map(|resolution| format!("{key} = {}", resolution.value));
    let integer = |key: &str| {
        get(key).and_then(|resolution| {
            resolution
                .value
                .parse::<i64>()
                .ok()
                .map(|value| (value, key.to_owned(), resolution.provenance.clone()))
        })
    };
    let mut normalized = BTreeMap::new();
    let style = get("indent_style").map(|resolution| resolution.value.to_ascii_lowercase());
    if let Some(style) = &style {
        normalized.insert(
            "indent_style",
            (
                Value::String(style.clone()),
                describe("indent_style").unwrap_or_default(),
                vec![
                    get("indent_style")
                        .map(|resolution| resolution.provenance.clone())
                        .unwrap_or_else(empty_provenance),
                ],
            ),
        );
    }
    let tab_width = integer("tab_width").or_else(|| integer("indent_size"));
    let indent_size = match get("indent_size") {
        Some(resolution) if resolution.value.eq_ignore_ascii_case("tab") => tab_width.clone(),
        Some(_) => integer("indent_size"),
        None => None,
    };
    let width = if style.as_deref() == Some("tab") {
        tab_width.or(indent_size)
    } else {
        indent_size
    };
    if let Some((value, key, provenance)) = width {
        normalized.insert(
            "indent_width",
            (
                Value::Int(value),
                describe(&key).unwrap_or_default(),
                vec![provenance],
            ),
        );
    }
    if let Some(resolution) = get("max_line_length")
        && let Ok(value) = resolution.value.parse::<i64>()
    {
        normalized.insert(
            "max_line_length",
            (
                Value::Int(value),
                describe("max_line_length").unwrap_or_default(),
                vec![resolution.provenance.clone()],
            ),
        );
    }
    for key in ["end_of_line", "spelling_language"] {
        if let Some(resolution) = get(key) {
            normalized.insert(
                if key == "end_of_line" {
                    "end_of_line"
                } else {
                    "spelling_language"
                },
                (
                    Value::String(resolution.value.to_ascii_lowercase()),
                    describe(key).unwrap_or_default(),
                    vec![resolution.provenance.clone()],
                ),
            );
        }
    }
    for key in ["insert_final_newline", "trim_trailing_whitespace"] {
        if let Some(resolution) = get(key) {
            let value = match resolution.value.to_ascii_lowercase().as_str() {
                "true" => true,
                "false" => false,
                _ => continue,
            };
            normalized.insert(
                if key == "insert_final_newline" {
                    "insert_final_newline"
                } else {
                    "trim_trailing_whitespace"
                },
                (
                    Value::Bool(value),
                    describe(key).unwrap_or_default(),
                    vec![resolution.provenance.clone()],
                ),
            );
        }
    }
    normalized
}

fn empty_provenance() -> Provenance {
    Provenance {
        origin: crate::model::Origin::new(Path::new(""), None),
        glob: String::new(),
    }
}

/// The standard property a normalized property comes from, for `skip` hints.
fn standard_name(property: &str) -> &str {
    if property == "indent_width" {
        "indent_size"
    } else {
        property
    }
}

/// Concrete paths under `dir` that `section` matches, built from its glob.
fn witness_paths(source: &Source, section: &Section, dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for pattern in expand(&section.glob) {
        let concrete = concretize(&pattern);
        let concrete = concrete.trim_start_matches('/');
        let anchored = concrete.contains('/');
        let mut candidates = Vec::new();
        if anchored {
            candidates.push(source.dir.join(concrete));
            if let Some(rest) = concrete.strip_prefix(&format!("{WITNESS}/"))
                && dir.starts_with(&source.dir)
            {
                candidates.push(dir.join(rest));
            }
        } else if dir.starts_with(&source.dir) {
            candidates.push(dir.join(concrete));
        } else {
            candidates.push(source.dir.join(concrete));
        }
        for candidate in candidates {
            let Ok(relative) = candidate.strip_prefix(&source.dir) else {
                continue;
            };
            if candidate.starts_with(dir) && section.applies_to(relative) {
                paths.push(candidate);
            }
        }
    }
    paths
}

/// Expands `{a,b}` alternatives; a numeric range contributes its first number.
fn expand(glob: &str) -> Vec<String> {
    const LIMIT: usize = 64;
    let chars = glob.chars().collect::<Vec<_>>();
    let mut start = None;
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '\\' => index += 2,
            '{' => {
                start = Some(index);
                break;
            }
            _ => index += 1,
        }
    }
    let Some(start) = start else {
        return vec![glob.to_owned()];
    };
    let mut depth = 0;
    let mut parts = Vec::new();
    let mut part_start = start + 1;
    let mut end = None;
    let mut index = start;
    while index < chars.len() {
        match chars[index] {
            '\\' => {
                index += 2;
                continue;
            }
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    parts.push(chars[part_start..index].iter().collect::<String>());
                    end = Some(index);
                    break;
                }
            }
            ',' if depth == 1 => {
                parts.push(chars[part_start..index].iter().collect::<String>());
                part_start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    let prefix = chars[..start].iter().collect::<String>();
    let Some(end) = end else {
        // An unmatched brace is literal.
        return vec![format!(
            "{prefix}\\{{{}",
            expand(&chars[start + 1..].iter().collect::<String>()).join("")
        )];
    };
    let suffix = chars[end + 1..].iter().collect::<String>();
    let alternatives = if parts.len() == 1 {
        match parts[0].split_once("..") {
            Some((low, high)) if low.parse::<i64>().is_ok() && high.parse::<i64>().is_ok() => {
                vec![low.to_owned()]
            }
            _ => vec![format!("\\{{{}\\}}", parts[0])],
        }
    } else {
        parts
    };
    let mut expanded = Vec::new();
    for alternative in alternatives {
        for result in expand(&format!("{prefix}{alternative}{suffix}")) {
            if expanded.len() < LIMIT {
                expanded.push(result);
            }
        }
    }
    expanded
}

/// Replaces wildcards with placeholder names that the glob still matches.
fn concretize(pattern: &str) -> String {
    let mut output = String::new();
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                if let Some(next) = chars.next() {
                    output.push(next);
                }
            }
            '*' => {
                while chars.peek() == Some(&'*') {
                    chars.next();
                }
                output.push_str(WITNESS);
            }
            '?' => output.push('w'),
            '[' => {
                let mut class = Vec::new();
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == ']' && !class.is_empty() {
                        closed = true;
                        break;
                    }
                    class.push(next);
                }
                if !closed {
                    output.push('[');
                    output.extend(class);
                } else if matches!(class.first(), Some('!' | '^')) {
                    output.push(
                        ['w', 'x', 'y', 'z']
                            .into_iter()
                            .find(|candidate| !class.contains(candidate))
                            .unwrap_or('w'),
                    );
                } else {
                    output.push(class[0]);
                }
            }
            other => output.push(other),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_alternatives_and_ranges() {
        assert_eq!(expand("*.{js,ts}"), ["*.js", "*.ts"]);
        assert_eq!(expand("{package.json,*.yml}"), ["package.json", "*.yml"]);
        assert_eq!(expand("file{1..3}.txt"), ["file1.txt"]);
        assert_eq!(expand("a{b,{c,d}}"), ["ab", "ac", "ad"]);
        assert_eq!(concretize(&expand("{single}")[0]), "{single}");
    }

    #[test]
    fn concretizes_wildcards() {
        assert_eq!(concretize("src/legacy/**.rs"), "src/legacy/__w__.rs");
        assert_eq!(concretize("**/test_*.py"), "__w__/test___w__.py");
        assert_eq!(concretize("[!a]?.[ch]"), "ww.c");
        assert_eq!(concretize("a\\*b"), "a*b");
    }
}
