//! EditorConfig resolution over sources: closer files win, later sections win, `unset` clears.

use std::collections::BTreeMap;
use std::path::Path;

use crate::model::{self, Origin};
use crate::project::Project;
use crate::value::{self, Value};

/// Where a resolved value came from.
#[derive(Clone, Debug)]
pub(crate) struct Provenance {
    pub(crate) origin: Origin,
    pub(crate) glob: String,
}

impl Provenance {
    pub(crate) fn display(&self, root: &Path) -> String {
        format!("{} [{:?}]", self.origin.display(root), self.glob)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Resolution {
    /// The value as EditorConfig spells it.
    pub(crate) value: String,
    pub(crate) provenance: Provenance,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ToolResolution {
    pub(crate) value: Option<Value>,
    pub(crate) provenance: BTreeMap<value::Path, Provenance>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Resolved {
    /// Properties keyed by their lowercase name.
    pub(crate) properties: BTreeMap<String, Resolution>,
    pub(crate) tools: BTreeMap<String, ToolResolution>,
}

pub(crate) fn resolve(project: &Project, path: &Path) -> Resolved {
    let mut resolved = Resolved::default();
    for source in project.chain(path) {
        let Ok(relative) = path.strip_prefix(&source.dir) else {
            continue;
        };
        for section in &source.sections {
            if !section.applies_to(relative) {
                continue;
            }
            for property in &section.properties {
                // Sources are validated, so every property renders.
                let Ok(value) = model::render_scalar(&property.value) else {
                    continue;
                };
                resolved.properties.insert(
                    property.key.to_ascii_lowercase(),
                    Resolution {
                        value,
                        provenance: Provenance {
                            origin: property.origin.clone(),
                            glob: section.glob.clone(),
                        },
                    },
                );
            }
            for tool in &section.tools {
                let entry = resolved.tools.entry(tool.id.clone()).or_default();
                match &mut entry.value {
                    Some(value) => value.merge(&tool.value),
                    None => entry.value = Some(tool.value.clone()),
                }
                for (path, _) in tool.value.leaves() {
                    let origin = tool
                        .origins
                        .iter()
                        .find(|(leaf, _)| leaf == &path)
                        .map_or_else(|| section.origin.clone(), |(_, origin)| origin.clone());
                    entry.provenance.insert(
                        path,
                        Provenance {
                            origin,
                            glob: section.glob.clone(),
                        },
                    );
                }
            }
        }
    }
    resolved
        .properties
        .retain(|_, resolution| !resolution.value.eq_ignore_ascii_case("unset"));
    resolved
}
