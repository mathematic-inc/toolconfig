use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::{Repository, git, spec};
use crate::inputs::{Inputs, digest};
use crate::model::SOURCE_NAMES;
use crate::paths;

// Bound contention while allowing another process time to fetch.
const LOCK_TIMEOUT: Duration = Duration::from_secs(120);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    files: BTreeMap<String, String>,
}

pub(crate) struct Snapshot {
    pub(crate) tree: PathBuf,
    pub(crate) source: PathBuf,
}

pub(crate) fn cache_root(root: &Path) -> Result<Option<PathBuf>> {
    let cache = std::env::var_os("TOOLCONFIG_REPO_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::cache_dir().map(|path| path.join("toolconfig/repos")))
        .map(|path| paths::absolute(root, &path))
        .transpose()?;
    if let Some(cache) = &cache {
        let real = cache.canonicalize().unwrap_or_else(|_| cache.clone());
        if root.starts_with(&real) {
            bail!(
                "TOOLCONFIG_REPO_CACHE_DIR must not contain the project root; choose a separate cache directory"
            );
        }
    }
    Ok(cache)
}

pub(crate) fn fetch(repository: &Repository, root: &Path, inputs: &mut Inputs) -> Result<Snapshot> {
    let cache = cache_root(root)?
        .context("cannot locate a repository cache; set TOOLCONFIG_REPO_CACHE_DIR")?;
    private_directory(&cache)?;
    let cache = cache
        .canonicalize()
        .context("cannot resolve the repository cache")?;
    inputs.immutable.insert(cache.clone());
    let key = digest(format!("v1\n{}\n{}", repository.git(), repository.rev()).as_bytes());
    let destination = cache.join(&key);
    if !destination.try_exists()? {
        let lock_path = cache.join(format!("{key}.lock"));
        regular_file(&lock_path)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        acquire(&lock)?;
        if !destination.try_exists()? {
            populate(repository, &cache, &destination)
                .with_context(|| format!("cannot fetch {}", repository.name()))?;
        }
    }
    let manifest = verify(&destination, inputs).with_context(|| {
        format!(
            "invalid repository cache for {}; remove {} and retry",
            repository.name(),
            destination.display()
        )
    })?;
    let path = match repository.path() {
        Some(path) => path.to_owned(),
        None => {
            let sources = SOURCE_NAMES
                .iter()
                .filter(|name| manifest.files.contains_key(**name))
                .collect::<Vec<_>>();
            match sources.as_slice() {
                [name] => (*name).to_string(),
                [] => bail!(
                    "{} has no ToolConfig source at its root; specify path",
                    repository.name()
                ),
                _ => bail!(
                    "{} has multiple ToolConfig sources at its root; specify path",
                    repository.name()
                ),
            }
        }
    };
    if !manifest.files.contains_key(&path) {
        bail!(
            "{}: source {path:?} is missing or is not a regular file",
            repository.name()
        );
    }
    let tree = destination.join("tree");
    Ok(Snapshot {
        source: tree.join(path),
        tree,
    })
}

fn acquire(lock: &File) -> Result<()> {
    let start = Instant::now();
    loop {
        match fs2::FileExt::try_lock_exclusive(lock) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    && start.elapsed() < LOCK_TIMEOUT =>
            {
                std::thread::sleep(LOCK_RETRY_INTERVAL);
            }
            Err(error) => {
                return Err(error)
                    .context("repository cache is busy; retry after the other fetch finishes");
            }
        }
    }
}

fn populate(repository: &Repository, cache: &Path, destination: &Path) -> Result<()> {
    let stage = tempfile::Builder::new()
        .prefix(".fetch-")
        .tempdir_in(cache)?;
    let objects = stage.path().join("objects");
    fs::create_dir(&objects)?;
    let format = if repository.rev().len() == 64 {
        "--object-format=sha256"
    } else {
        "--object-format=sha1"
    };
    command(
        &objects,
        &["init", "--quiet", "--bare", "--template=", format, "."],
    )?;
    fs::create_dir(objects.join("disabled-hooks"))?;
    command(
        &objects,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-recurse-submodules",
            "--depth=1",
            "--",
            repository.git(),
            repository.rev(),
        ],
    )?;
    let commit = command(
        &objects,
        &[
            "rev-parse",
            "--verify",
            &format!("{}^{{commit}}", repository.rev()),
        ],
    )?;
    if String::from_utf8_lossy(&commit).trim() != repository.rev() {
        bail!("the fetched object is not the requested commit");
    }
    let snapshot = stage.path().join("snapshot");
    let tree = snapshot.join("tree");
    fs::create_dir_all(&tree)?;
    let entries = command(
        &objects,
        &["ls-tree", "-rz", "--full-tree", repository.rev()],
    )?;
    let mut manifest = Manifest {
        version: 1,
        files: BTreeMap::new(),
    };
    let mut names = BTreeSet::new();
    for entry in entries
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let entry = std::str::from_utf8(entry).context("repository filenames must be UTF-8")?;
        let (metadata, path) = entry.split_once('\t').context("invalid Git tree entry")?;
        spec::relative_path(path)?;
        let fields = metadata.split_whitespace().collect::<Vec<_>>();
        let [mode, kind, object] = fields.as_slice() else {
            bail!("invalid Git tree metadata")
        };
        // Never materialize symlinks or recurse into submodules.
        if *mode == "120000" || *kind == "commit" {
            continue;
        }
        if !matches!(*mode, "100644" | "100755") || *kind != "blob" {
            bail!("unsupported Git tree entry");
        }
        let target = tree.join(path);
        if !names.insert(paths::key(&target)) {
            bail!("repository paths collide on this filesystem");
        }
        fs::create_dir_all(
            target
                .parent()
                .context("a repository file needs a parent")?,
        )?;
        let file = File::create(&target)?;
        git::run(
            &objects,
            &[
                OsStr::new("cat-file"),
                OsStr::new("blob"),
                OsStr::new(object),
            ],
            file,
        )?;
        manifest
            .files
            .insert(path.to_owned(), digest(&fs::read(&target)?));
    }
    let manifest_file = File::create(snapshot.join("manifest.json"))?;
    serde_json::to_writer(&manifest_file, &manifest)?;
    manifest_file.sync_all()?;
    fs::rename(&snapshot, destination).context("cannot install the repository snapshot")?;
    Ok(())
}

fn command(directory: &Path, args: &[&str]) -> Result<Vec<u8>> {
    git::read(directory, &args.iter().map(OsStr::new).collect::<Vec<_>>())
}

fn verify(snapshot: &Path, inputs: &mut Inputs) -> Result<Manifest> {
    reject_symlink(snapshot)?;
    let manifest_path = snapshot.join("manifest.json");
    regular_file(&manifest_path)?;
    let bytes = inputs
        .inspect(&manifest_path)?
        .context("repository manifest is missing")?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    if manifest.version != 1 {
        bail!("unsupported repository cache version");
    }
    let tree = snapshot.join("tree");
    reject_symlink(&tree)?;
    let mut seen = BTreeSet::new();
    let mut pending = vec![tree.clone()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            reject_symlink(&path)?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(path);
                continue;
            }
            if !kind.is_file() {
                bail!("repository snapshots may contain only regular files");
            }
            let relative = path
                .strip_prefix(&tree)?
                .to_str()
                .context("repository filenames must be UTF-8")?
                .replace('\\', "/");
            spec::relative_path(&relative)?;
            let expected = manifest
                .files
                .get(&relative)
                .context("unexpected file in repository snapshot")?;
            if digest(&fs::read(&path)?) != *expected {
                bail!("repository snapshot content changed");
            }
            seen.insert(relative);
        }
    }
    if seen.len() != manifest.files.len() {
        bail!("repository snapshot is incomplete");
    }
    Ok(manifest)
}

fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if paths::is_link(&metadata) => {
            bail!("refusing a symlink in the repository cache")
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn regular_file(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || paths::is_link(&metadata) => {
            bail!("refusing a nonregular file in the repository cache")
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn private_directory(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .context("cannot create the repository cache")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
