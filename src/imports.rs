//! Resolve repository declarations into the same source model as local configuration.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Result, bail};

use crate::inputs::Inputs;
use crate::model::{ParsedSettings, ParsedSource, Settings, Source};
use crate::{project, repository};

// A mistaken dependency graph must fail before exhausting the process stack.
const MAX_IMPORT_DEPTH: usize = 64;

pub(crate) fn resolve(source: ParsedSource, root: &Path, inputs: &mut Inputs) -> Result<Source> {
    let mut resolved = Source {
        path: source.path.clone(),
        dir: source.dir.clone(),
        root: source.root,
        settings: Settings::default(),
        sections: Vec::new(),
    };
    collect(source, root, inputs, &mut Vec::new(), &mut resolved)?;
    resolved.validate_settings()?;
    Ok(resolved)
}

fn collect(
    source: ParsedSource,
    root: &Path,
    inputs: &mut Inputs,
    stack: &mut Vec<String>,
    resolved: &mut Source,
) -> Result<()> {
    for repository in &source.settings.extends {
        let snapshot = repository::fetch(repository, root, inputs)?;
        let relative = snapshot
            .source
            .strip_prefix(&snapshot.tree)?
            .to_string_lossy()
            .replace('\\', "/");
        let identity = format!("{}:{relative}", repository.name());
        if stack.contains(&identity) {
            bail!(
                "repository import cycle: {} -> {identity}",
                stack.join(" -> ")
            );
        }
        if stack.len() >= MAX_IMPORT_DEPTH {
            bail!("repository imports exceed {MAX_IMPORT_DEPTH} levels: {identity}");
        }
        stack.push(identity.clone());
        let imported = (|| {
            let mut parsed = project::load_source(&snapshot.source, root, inputs)?;
            parsed.identify_repository(&snapshot.tree, &repository.name());
            collect(parsed, root, inputs, stack, resolved)
        })()
        .map_err(|error| {
            let tree = snapshot.tree.to_string_lossy();
            let label = format!("{}:", repository.name());
            let message = format!("{error:#}")
                .replace(tree.as_ref(), &label)
                .replace(&tree.replace('\\', "/"), &label);
            anyhow::anyhow!("importing {identity}: {message}")
        });
        stack.pop();
        imported?;
    }
    apply(&mut resolved.settings, source.settings);
    resolved.sections.extend(source.sections);
    Ok(())
}

fn apply(target: &mut Settings, incoming: ParsedSettings) {
    if let Some(tools) = incoming.tools {
        target.tools = tools;
        let enabled = target
            .tools
            .iter()
            .map(|(id, _)| id)
            .collect::<BTreeSet<_>>();
        target.tool.retain(|id, _| enabled.contains(id));
    }
    for (id, incoming) in incoming.tool {
        let settings = target.tool.entry(id).or_default();
        if incoming.format.is_some() {
            settings.format = incoming.format;
        }
        if incoming.path.is_some() {
            settings.path = incoming.path;
        }
        if let Some(skip) = incoming.skip {
            settings.skip = skip;
        }
        settings.vale.apply(incoming.vale);
    }
    merge_files(&mut target.files, incoming.files);
}

fn merge_files(target: &mut Vec<crate::model::FileSpec>, files: Vec<crate::model::FileSpec>) {
    for file in files {
        match target
            .iter_mut()
            .find(|existing| existing.label == file.label)
        {
            Some(existing) => *existing = file,
            None => target.push(file),
        }
    }
}
