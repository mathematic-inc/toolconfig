//! Project discovery: the root source, and every source and `.editorconfig` beneath it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::inputs::Inputs;
use crate::model::{self, SOURCE_NAMES, Source, Syntax};
use crate::{pkl_source, toml_source};

pub(crate) struct Project {
    pub(crate) root: PathBuf,
    /// Sources ordered so that every directory precedes its subdirectories.
    pub(crate) sources: Vec<Source>,
    /// Plain `.editorconfig` files in directories without a source.
    pub(crate) unmanaged: Vec<PathBuf>,
}

impl Project {
    pub(crate) fn source_in(&self, dir: &Path) -> Option<&Source> {
        self.sources.iter().find(|source| source.dir == dir)
    }

    /// Sources that apply to `path`, outermost first, as EditorConfig applies them.
    pub(crate) fn chain(&self, path: &Path) -> Vec<&Source> {
        let mut chain = Vec::new();
        for ancestor in path.ancestors().skip(1) {
            if !ancestor.starts_with(&self.root) {
                break;
            }
            if let Some(source) = self.source_in(ancestor) {
                chain.push(source);
                if source.root {
                    break;
                }
            }
        }
        chain.reverse();
        chain
    }
}

/// The source files present in `dir`.
pub(crate) fn sources_in(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for name in SOURCE_NAMES {
        let path = dir.join(name);
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => found.push(path),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("cannot inspect {}", path.display()));
            }
        }
    }
    Ok(found)
}

fn single_source(dir: &Path) -> Result<Option<PathBuf>> {
    let mut found = sources_in(dir)?;
    if found.len() > 1 {
        bail!(
            "{} has more than one ToolConfig source ({}); keep exactly one",
            dir.display(),
            found
                .iter()
                .filter_map(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(found.pop())
}

/// Loads one source, recording what it read.
pub(crate) fn load_source(path: &Path, root: &Path, inputs: &mut Inputs) -> Result<Source> {
    match model::syntax_of(path) {
        Some(Syntax::Toml) => {
            let bytes = inputs
                .inspect(path)?
                .with_context(|| format!("{} disappeared", path.display()))?;
            let text = String::from_utf8(bytes)
                .with_context(|| format!("{} is not UTF-8", path.display()))?;
            toml_source::parse(path, &text)
        }
        Some(Syntax::Pkl) => {
            let (source, recorded) = pkl_source::evaluate(path, root);
            inputs.extend(recorded);
            inputs.inspect(path)?;
            source
        }
        None => bail!("{} is not a ToolConfig source", path.display()),
    }
}

/// Finds the project root: an explicit directory, or the nearest ancestor whose source
/// sets `root = true`.
pub(crate) fn find_root(requested: Option<&Path>) -> Result<PathBuf> {
    let cwd = std::env::current_dir()
        .context("cannot read the current directory")?
        .canonicalize()?;
    if let Some(requested) = requested {
        let root = cwd.join(requested);
        let root = root
            .canonicalize()
            .with_context(|| format!("cannot find the project root {}", root.display()))?;
        if !root.is_dir() {
            bail!("the project root is not a directory: {}", root.display());
        }
        return Ok(root);
    }
    for ancestor in cwd.ancestors() {
        if let Some(path) = single_source(ancestor)? {
            let source = load_source(&path, ancestor, &mut Inputs::default())?;
            if source.root {
                return Ok(ancestor.to_path_buf());
            }
        }
    }
    bail!(
        "no ToolConfig source with root = true in {} or its parents; run toolconfig init",
        cwd.display()
    )
}

/// Loads every source in the project. Inputs are returned even on failure for watch mode.
pub(crate) fn load(root: &Path) -> (Result<Project>, Inputs) {
    let mut inputs = Inputs::default();
    let project = load_into(root, &mut inputs);
    (project, inputs)
}

fn load_into(root: &Path, inputs: &mut Inputs) -> Result<Project> {
    let mut directories = BTreeMap::<PathBuf, (Vec<PathBuf>, bool)>::new();
    let state = root.join(crate::paths::STATE_DIR);
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .ignore(false)
        .git_global(false)
        .require_git(false)
        .filter_entry(move |entry| {
            let name = entry.file_name();
            !(entry.file_type().is_some_and(|kind| kind.is_dir())
                && (name == ".git" || entry.path() == state))
        })
        .build();
    for entry in walker {
        let entry = entry.context("cannot scan the project")?;
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(dir) = path.parent() else { continue };
        if SOURCE_NAMES.contains(&name) {
            directories
                .entry(dir.to_path_buf())
                .or_default()
                .0
                .push(path.to_path_buf());
        } else if name == ".editorconfig" {
            directories.entry(dir.to_path_buf()).or_default().1 = true;
        }
    }
    let mut sources = Vec::new();
    let mut unmanaged = Vec::new();
    let mut nested_roots = Vec::<PathBuf>::new();
    // Parents sort before children, so nested projects are found before their contents.
    for (dir, (mut paths, editorconfig)) in directories {
        if nested_roots.iter().any(|nested| dir.starts_with(nested)) {
            continue;
        }
        if paths.len() > 1 {
            single_source(&dir)?;
        }
        match paths.pop() {
            Some(path) => {
                let source = load_source(&path, root, inputs)?;
                if source.root && dir != root {
                    nested_roots.push(dir);
                    continue;
                }
                sources.push(source);
            }
            None if editorconfig => {
                let path = dir.join(".editorconfig");
                let bytes = inputs.inspect(&path)?.unwrap_or_default();
                let is_root = ec4rs::ConfigParser::new_buffered(bytes.as_slice())
                    .is_ok_and(|parser| parser.is_root);
                if is_root && dir != root && !generated_header(&bytes) {
                    nested_roots.push(dir);
                } else {
                    unmanaged.push(path);
                }
            }
            None => {}
        }
    }
    match sources.first() {
        Some(source) if source.dir == root => {
            if !source.root {
                bail!(
                    "{}: the project's root source must set root = true",
                    source.path.display()
                );
            }
        }
        _ => bail!(
            "{} has no ToolConfig source; run toolconfig init",
            root.display()
        ),
    }
    Ok(Project {
        root: root.to_path_buf(),
        sources,
        unmanaged,
    })
}

pub(crate) const GENERATED_HEADER: &str = "# Generated by ToolConfig from ";

fn generated_header(bytes: &[u8]) -> bool {
    bytes.starts_with(GENERATED_HEADER.as_bytes())
}
