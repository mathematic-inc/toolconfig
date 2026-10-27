//! Compilation of a project into its complete set of outputs, checked before anything is written.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::inputs::Inputs;
use crate::paths::{self, absolute};
use crate::project::Project;
use crate::projection::{self, Explained};
use crate::{cascade, catalog, render};

pub(crate) const STAGE_PREFIX: &str = ".toolconfig-stage-";

pub(crate) struct Output {
    pub(crate) path: PathBuf,
    /// What produced the output: `.editorconfig`, a tool id, or `files.<label>`.
    pub(crate) label: String,
    pub(crate) format: String,
    pub(crate) bytes: Vec<u8>,
    pub(crate) source: PathBuf,
    pub(crate) explanation: Vec<Explained>,
}

impl Output {
    pub(crate) fn is_editorconfig(&self) -> bool {
        self.label == ".editorconfig"
    }
}

pub(crate) struct Plan {
    pub(crate) root: PathBuf,
    pub(crate) outputs: Vec<Output>,
    pub(crate) alternatives: Vec<PathBuf>,
    pub(crate) inputs: Inputs,
    pub(crate) warnings: Vec<String>,
    /// `.editorconfig` files no source produces; publication decides whether they are ours.
    pub(crate) unmanaged: Vec<PathBuf>,
}

/// Loads and compiles the project at `root`. Inputs are returned even on failure for watch mode.
pub(crate) fn build(root: &Path) -> (Result<Plan>, Inputs) {
    let (project, inputs) = crate::project::load(root);
    match project {
        Ok(project) => (compile(&project, inputs.clone()), inputs),
        Err(error) => (Err(error), inputs),
    }
}

pub(crate) fn compile(project: &Project, mut inputs: Inputs) -> Result<Plan> {
    let mut outputs = Vec::new();
    let mut alternatives = Vec::new();
    let mut warnings = BTreeSet::new();
    for source in &project.sources {
        if let Some(text) = render::editorconfig(source)? {
            outputs.push(Output {
                path: source.dir.join(".editorconfig"),
                label: ".editorconfig".to_owned(),
                format: "editorconfig".to_owned(),
                bytes: text.into_bytes(),
                source: source.path.clone(),
                explanation: Vec::new(),
            });
        }
        for (id, _) in &source.settings.tools {
            let builtin = catalog::get(id).context("validated tool")?;
            let output = projection::compile(project, source, builtin, &mut warnings)
                .with_context(|| {
                    format!("{} in {}", id, paths::relative(&project.root, &source.path))
                })?;
            if let Some(key) = &builtin.manifest_key {
                let manifest = source.dir.join("package.json");
                if let Some(bytes) = inputs.inspect(&manifest)? {
                    let value: serde_json::Value = serde_json::from_slice(&bytes)
                        .with_context(|| format!("cannot inspect {}", manifest.display()))?;
                    if value.get(key).is_some() {
                        bail!(
                            "{} contains {key:?}, which competes with the generated {}",
                            manifest.display(),
                            paths::relative(&project.root, &output.path)
                        );
                    }
                }
            }
            let style = style_at(project, &output.path);
            let text =
                render::render(&output.format, &output.value, &style).with_context(|| {
                    format!("rendering {} as {}", output.path.display(), output.format)
                })?;
            alternatives.extend(output.alternatives);
            outputs.push(Output {
                path: output.path,
                label: id.clone(),
                format: output.format,
                bytes: text.into_bytes(),
                source: source.path.clone(),
                explanation: output.explanation,
            });
        }
        for file in &source.settings.files {
            let path = absolute(&source.dir, Path::new(&file.path))?;
            if !path.starts_with(&project.root) {
                bail!(
                    "{}: file {:?} writes outside the project: {}",
                    file.origin,
                    file.label,
                    file.path
                );
            }
            let text = render::render(&file.format, &file.config, &style_at(project, &path))
                .with_context(|| {
                    format!(
                        "{}: rendering file {:?} as {}",
                        file.origin, file.label, file.format
                    )
                })?;
            outputs.push(Output {
                path,
                label: format!("files.{}", file.label),
                format: file.format.clone(),
                bytes: text.into_bytes(),
                source: source.path.clone(),
                explanation: Vec::new(),
            });
        }
    }
    let enabled = project
        .sources
        .iter()
        .flat_map(|source| source.settings.tools.iter().map(|(id, _)| id.as_str()))
        .collect::<BTreeSet<_>>();
    for source in &project.sources {
        for section in &source.sections {
            for tool in &section.tools {
                if !enabled.contains(tool.id.as_str()) {
                    warnings.insert(format!(
                        "{}: {} options in section {:?} are unused; no source lists {} in tools",
                        section.origin.display(&project.root),
                        tool.id,
                        section.glob,
                        tool.id
                    ));
                }
            }
        }
    }
    check(project, &outputs, &alternatives, &inputs)?;
    let mut plan = Plan {
        root: project.root.clone(),
        outputs,
        alternatives,
        inputs,
        warnings: warnings.into_iter().collect(),
        unmanaged: project.unmanaged.clone(),
    };
    plan.outputs.sort_by(|a, b| a.path.cmp(&b.path));
    plan.alternatives.sort();
    plan.alternatives.dedup();
    Ok(plan)
}

/// The layout the sources prescribe for a generated file at `path`.
fn style_at(project: &Project, path: &Path) -> render::Style {
    let resolved = cascade::resolve(project, path);
    let get = |key: &str| {
        resolved
            .properties
            .get(key)
            .map(|resolution| resolution.value.to_ascii_lowercase())
    };
    let number = |key: &str| get(key).and_then(|value| value.parse::<usize>().ok());
    let mut style = render::Style {
        tabs: get("indent_style").as_deref() == Some("tab"),
        ..render::Style::default()
    };
    if let Some(indent) = number("indent_size").or_else(|| number("tab_width")) {
        style.indent = indent;
    }
    if let Some(width) = number("max_line_length") {
        style.width = width;
    }
    style
}

fn check(
    project: &Project,
    outputs: &[Output],
    alternatives: &[PathBuf],
    inputs: &Inputs,
) -> Result<()> {
    let state = paths::key(&project.root.join(paths::STATE_DIR));
    let mut seen = BTreeMap::new();
    let keys = outputs
        .iter()
        .map(|output| paths::key(&output.path))
        .collect::<BTreeSet<_>>();
    for output in outputs {
        let shown = paths::relative(&project.root, &output.path);
        if let Some(previous) = seen.insert(paths::key(&output.path), &output.label) {
            bail!("{} and {previous} both write {shown}", output.label);
        }
        if inputs.contains(&output.path)
            || project
                .sources
                .iter()
                .any(|source| paths::key(&source.path) == paths::key(&output.path))
        {
            bail!("{shown} collides with a ToolConfig source or input");
        }
        if output
            .path
            .ancestors()
            .any(|ancestor| paths::key(ancestor) == state)
        {
            bail!("{shown} collides with ToolConfig's internal state");
        }
        if output
            .path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(STAGE_PREFIX))
        {
            bail!("{shown} uses ToolConfig's reserved staging prefix");
        }
        for ancestor in output.path.ancestors().skip(1) {
            if keys.contains(&paths::key(ancestor)) {
                bail!(
                    "{} is both an output file and an output directory",
                    paths::relative(&project.root, ancestor)
                );
            }
        }
        // Two enabled tools cannot evade native exclusivity by owning both alternatives.
        if alternatives.contains(&output.path) {
            bail!(
                "{shown} conflicts with another enabled tool's configuration in the same directory"
            );
        }
    }
    Ok(())
}
