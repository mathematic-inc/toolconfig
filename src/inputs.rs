//! Files and directories a compilation read, so watch mode and publication can detect changes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use pklr::{EvalCapabilities, NativeCapabilities};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Default)]
pub(crate) struct Inputs {
    pub(crate) files: BTreeMap<PathBuf, Option<String>>,
    pub(crate) directories: BTreeSet<PathBuf>,
    pub(crate) globs: BTreeMap<(PathBuf, String), Vec<PathBuf>>,
    pub(crate) excluded: BTreeSet<PathBuf>,
}

impl Inputs {
    pub(crate) fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.files.keys()
    }

    pub(crate) fn directories(&self) -> impl Iterator<Item = &PathBuf> {
        self.directories.iter()
    }

    pub(crate) fn changed(&self) -> bool {
        self.files
            .iter()
            .any(|(path, old)| match (old, fingerprint(path)) {
                (Some(old), Ok(current)) => old != &current,
                (None, Err(_)) => path.try_exists().unwrap_or(true),
                _ => true,
            })
            || self.globs.iter().any(|((base, pattern), expected)| {
                NativeCapabilities::new()
                    .glob(base, pattern)
                    .map_or(true, |paths| {
                        let paths = paths
                            .into_iter()
                            .filter(|path| !self.ignores(path))
                            .collect::<Vec<_>>();
                        &paths != expected
                    })
            })
    }

    pub(crate) fn contains(&self, path: &Path) -> bool {
        self.files.contains_key(path)
            || path
                .canonicalize()
                .is_ok_and(|real| self.files.contains_key(&real))
    }

    pub(crate) fn ignores(&self, path: &Path) -> bool {
        self.excluded
            .iter()
            .any(|directory| path.starts_with(directory))
    }

    /// Reads a file and records its fingerprint, or records that it is missing.
    pub(crate) fn inspect(&mut self, path: &Path) -> Result<Option<Vec<u8>>> {
        match std::fs::read(path) {
            Ok(bytes) => {
                self.files.insert(path.to_path_buf(), Some(digest(&bytes)));
                Ok(Some(bytes))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.files.insert(path.to_path_buf(), None);
                Ok(None)
            }
            Err(error) => Err(error).with_context(|| format!("cannot inspect {}", path.display())),
        }
    }

    pub(crate) fn extend(&mut self, other: Inputs) {
        self.files.extend(other.files);
        self.directories.extend(other.directories);
        self.globs.extend(other.globs);
        self.excluded.extend(other.excluded);
    }
}

pub(crate) fn fingerprint(path: &Path) -> Result<String> {
    Ok(digest(&std::fs::read(path).with_context(|| {
        format!("cannot read {}", path.display())
    })?))
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
