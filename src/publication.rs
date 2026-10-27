//! Journaled publication of owned files and recovery of interrupted generations.
//!
//! Ownership records use paths relative to the project root and contain no machine-specific
//! data, so `.toolconfig-state/state.json` can be committed beside the generated files.

mod identity;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::compiler::{Plan, STAGE_PREFIX};
use crate::ignore;
use crate::inputs::digest;
use crate::model::ADOPTING_GUIDE;
use crate::paths::{self, STATE_DIR};
use identity::Identity;

const STATE_VERSION: u32 = 1;
const STATE_GITIGNORE: &str =
    "# Written by ToolConfig. Commit state.json; the rest is local.\n*\n!.gitignore\n!state.json\n";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Record {
    hash: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    /// Owned outputs by path relative to the project root, with `/` separators.
    outputs: BTreeMap<String, Record>,
}

impl State {
    fn empty() -> Self {
        Self {
            version: STATE_VERSION,
            outputs: BTreeMap::new(),
        }
    }

    /// Identifies a publication without machine-specific data.
    fn generation(&self) -> Result<String> {
        Ok(digest(&serde_json::to_vec(&self.outputs)?))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    content: String,
    hash: String,
    identity: Identity,
    mode: u32,
}

impl Snapshot {
    fn bytes(&self) -> Result<Vec<u8>> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.content)
            .context("invalid publication journal content")?;
        if digest(&bytes) != self.hash {
            bail!("publication journal content hash mismatch");
        }
        Ok(bytes)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Change {
    path: PathBuf,
    before: Option<Snapshot>,
    after: Option<Snapshot>,
    stage: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    root: PathBuf,
    generation: String,
    changes: Vec<Change>,
}

#[must_use = "keep the project lease alive until the operation completes"]
pub(crate) struct Project {
    root: PathBuf,
    state_dir: PathBuf,
    lock: File,
}

impl Project {
    pub(crate) fn acquire(root: &Path) -> Result<Self> {
        check_ancestors(root)?;
        let state_dir = root.join(STATE_DIR);
        check_ancestors(&state_dir)?;
        std::fs::create_dir_all(&state_dir)
            .with_context(|| format!("cannot create {}", state_dir.display()))?;
        let ignore_path = state_dir.join(".gitignore");
        reject_nonregular(&ignore_path)?;
        if std::fs::read(&ignore_path).ok().as_deref() != Some(STATE_GITIGNORE.as_bytes()) {
            write_file(&ignore_path, STATE_GITIGNORE.as_bytes(), 0o644)?;
        }
        let lock_path = state_dir.join("write.lock");
        reject_nonregular(&lock_path)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .context("cannot open the project writer lock")?;
        fs2::FileExt::try_lock_exclusive(&lock).context(
            "another ToolConfig writer is active; stop watch mode before generating or cleaning separately",
        )?;
        let project = Self {
            root: root.to_path_buf(),
            state_dir,
            lock,
        };
        project.recover()?;
        Ok(project)
    }

    fn state(&self) -> Result<State> {
        read_state(&self.root)
    }

    pub(crate) fn generate(&self, plan: &Plan) -> Result<()> {
        self.recover()?;
        let mut state = self.state()?;
        preflight(&self.root, plan, &state)?;
        let mut changes = Vec::new();
        let desired = plan
            .outputs
            .iter()
            .map(|output| output.path.clone())
            .collect::<BTreeSet<_>>();
        // Remove obsolete outputs before introducing an alternative native filename.
        for path in owned_paths(&self.root, &state)? {
            if !desired.contains(&path) {
                let before = owned_snapshot(&self.root, &path, &state)?;
                if before.is_some() {
                    changes.push((path.clone(), before, None));
                }
            }
        }
        let mut outputs = BTreeMap::new();
        for output in &plan.outputs {
            let before = owned_snapshot(&self.root, &output.path, &state)?;
            let hash = digest(&output.bytes);
            if before.as_ref().is_none_or(|old| old.hash != hash) {
                changes.push((output.path.clone(), before, Some(output.bytes.clone())));
            }
            outputs.insert(record_key(&self.root, &output.path)?, Record { hash });
        }
        let ignore_path = self.root.join(".gitignore");
        let before = read_snapshot(&ignore_path)?;
        let bytes = before.as_ref().map(Snapshot::bytes).transpose()?;
        let manage_ignore = ignore::enabled(bytes.as_deref())?;
        let ignored = ignored_outputs(plan);
        if manage_ignore {
            if desired.contains(&ignore_path) {
                bail!(".gitignore cannot be both a generated output and a managed ignore file");
            }
            let content = ignore::update(&self.root, &ignored, bytes.as_deref())?;
            if bytes.as_deref() != Some(&content) {
                changes.push((ignore_path, before, Some(content)));
            }
        }
        if plan.inputs.changed() {
            bail!("ToolConfig sources changed during compilation; save again or retry");
        }
        state.outputs = outputs;
        let count = changes.len();
        self.commit(state, changes)?;
        eprintln!(
            "toolconfig: generated {} files ({count} changed)",
            desired.len()
        );
        if manage_ignore {
            report_tracked(&self.root, &ignored);
        }
        Ok(())
    }

    pub(crate) fn establish_ignore(&self, plan: &Plan) -> Result<()> {
        self.recover()?;
        let state = self.state()?;
        let path = self.root.join(".gitignore");
        if plan.outputs.iter().any(|output| output.path == path) {
            bail!(".gitignore cannot be both a generated output and a managed ignore file");
        }
        let before = read_snapshot(&path)?;
        let bytes = before.as_ref().map(Snapshot::bytes).transpose()?;
        let ignored = ignored_outputs(plan);
        let content = ignore::update(&self.root, &ignored, bytes.as_deref())?;
        if bytes.as_deref() != Some(&content) {
            self.commit(state, vec![(path, before, Some(content))])?;
        }
        report_tracked(&self.root, &ignored);
        Ok(())
    }

    pub(crate) fn clean(&self) -> Result<()> {
        self.recover()?;
        let mut state = self.state()?;
        let mut changes = Vec::new();
        let mut preserved = Vec::new();
        let mut remaining = BTreeMap::new();
        for (key, record) in &state.outputs {
            let path = from_record_key(&self.root, key)?;
            match read_snapshot(&path) {
                Ok(None) => {}
                Ok(Some(before)) if before.hash == record.hash => {
                    changes.push((path, Some(before), None));
                }
                _ => {
                    preserved.push(key.clone());
                    remaining.insert(key.clone(), record.clone());
                }
            }
        }
        state.outputs = remaining;
        let count = changes.len();
        self.commit(state, changes)?;
        eprintln!("toolconfig: removed {count} generated files");
        if !preserved.is_empty() {
            bail!(
                "preserved modified or ambiguous files:\n{}",
                preserved.join("\n")
            );
        }
        Ok(())
    }

    fn commit(
        &self,
        state: State,
        changes: Vec<(PathBuf, Option<Snapshot>, Option<Vec<u8>>)>,
    ) -> Result<()> {
        if self.state_dir.join("journal.json").try_exists()? {
            bail!("a previous publication must be recovered before writing");
        }
        let state_path = self.state_dir.join("state.json");
        if changes.is_empty() {
            let unchanged = if state_path.exists() {
                self.state()?.outputs == state.outputs
            } else {
                state.outputs.is_empty()
            };
            if unchanged {
                return Ok(());
            }
        }
        let mut stages = Vec::new();
        let mut entries = Vec::new();
        for (path, before, content) in changes {
            let stage = content
                .as_ref()
                .map(|bytes| stage_file(&path, bytes, before.as_ref().map(|old| old.mode)))
                .transpose()?;
            let after = stage
                .as_ref()
                .map(|stage| snapshot_file(stage.as_file()))
                .transpose()?;
            let stage_path = stage.as_ref().map(|stage| stage.path().to_path_buf());
            entries.push(Change {
                path,
                before,
                after,
                stage: stage_path,
            });
            stages.push(stage);
        }
        let journal = Journal {
            version: STATE_VERSION,
            root: self.root.clone(),
            generation: state.generation()?,
            changes: entries,
        };
        let journal_path = self.state_dir.join("journal.json");
        write_json(&journal_path, &journal, 0o600)?;
        let result = (|| -> Result<()> {
            for (change, stage) in journal.changes.iter().zip(stages.iter_mut()) {
                verify_current(&change.path, change.before.as_ref())?;
                match stage.take() {
                    Some(stage) => {
                        if change.before.is_some() {
                            stage
                                .persist(&change.path)
                                .map_err(|error| error.error)
                                .with_context(|| {
                                    format!("cannot replace {}", change.path.display())
                                })?;
                        } else {
                            stage
                                .persist_noclobber(&change.path)
                                .map_err(|error| error.error)
                                .with_context(|| {
                                    format!("refusing to overwrite {}", change.path.display())
                                })?;
                        }
                    }
                    None => std::fs::remove_file(&change.path).with_context(|| {
                        format!("cannot remove obsolete output {}", change.path.display())
                    })?,
                }
                sync_parent(&change.path)?;
            }
            write_json(&state_path, &state, 0o644)?;
            std::fs::remove_file(&journal_path)?;
            sync_parent(&journal_path)?;
            Ok(())
        })();
        if let Err(error) = result {
            if let Err(recovery) = self.recover() {
                return Err(error.context(format!(
                    "publication recovery requires attention: {recovery:#}"
                )));
            }
            return Err(error);
        }
        Ok(())
    }

    fn recover(&self) -> Result<()> {
        let journal_path = self.state_dir.join("journal.json");
        reject_nonregular(&journal_path)?;
        if !journal_path.try_exists()? {
            return Ok(());
        }
        let journal: Journal = serde_json::from_slice(&std::fs::read(&journal_path)?)
            .context("cannot decode the publication journal; preserving all outputs")?;
        if journal.version != STATE_VERSION || journal.root != self.root {
            bail!("publication journal belongs to a different project or version");
        }
        let mut destinations = BTreeSet::new();
        for change in &journal.changes {
            if !change.path.is_absolute()
                || !change.path.starts_with(&self.root)
                || change
                    .path
                    .components()
                    .any(|part| matches!(part, Component::ParentDir))
                || change.path.starts_with(&self.state_dir)
                || !destinations.insert(&change.path)
            {
                bail!("invalid destination in publication journal; preserving all outputs");
            }
            if let Some(before) = &change.before {
                before.bytes()?;
            }
            if let Some(after) = &change.after {
                after.bytes()?;
            }
            if change.stage.is_some() != change.after.is_some() {
                bail!("incomplete staging record in publication journal");
            }
            if let Some(stage) = &change.stage
                && (stage.parent() != change.path.parent()
                    || !stage
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with(STAGE_PREFIX)))
            {
                bail!("invalid staging path in publication journal");
            }
        }
        let committed = self.state()?.generation()? == journal.generation;
        if !committed {
            for change in journal.changes.iter().rev() {
                let current = read_snapshot(&change.path)?;
                if same_content(current.as_ref(), change.before.as_ref()) {
                    continue;
                }
                let is_ours = match (&current, &change.after) {
                    (Some(current), Some(after)) => {
                        current.hash == after.hash && current.identity == after.identity
                    }
                    (None, None) => true,
                    _ => false,
                };
                if !is_ours {
                    if change.before.is_none() {
                        // Exclusive creation lost a race, or somebody replaced our new file.
                        // The prior generation owned nothing here, so leave the foreign file alone.
                        eprintln!(
                            "toolconfig: preserved unexpected file {}",
                            change.path.display()
                        );
                        continue;
                    }
                    bail!(
                        "{} changed during an interrupted publication; preserving it and the recovery journal",
                        change.path.display()
                    );
                }
                match &change.before {
                    Some(before) => {
                        let stage = stage_file(&change.path, &before.bytes()?, Some(before.mode))?;
                        if current.is_some() {
                            stage.persist(&change.path).map_err(|error| error.error)?;
                        } else {
                            stage
                                .persist_noclobber(&change.path)
                                .map_err(|error| error.error)?;
                        }
                    }
                    None => {
                        if current.is_some() {
                            std::fs::remove_file(&change.path)?;
                        }
                    }
                }
                sync_parent(&change.path)?;
            }
        }
        for change in &journal.changes {
            if let (Some(stage), Some(after)) = (&change.stage, &change.after)
                && let Some(current) = read_snapshot(stage)?
            {
                if current.identity != after.identity || current.hash != after.hash {
                    bail!(
                        "unexpected replacement of staged file {}; preserving it",
                        stage.display()
                    );
                }
                std::fs::remove_file(stage)?;
            }
        }
        std::fs::remove_file(&journal_path)?;
        sync_parent(&journal_path)?;
        eprintln!("toolconfig: recovered an interrupted publication");
        Ok(())
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        if let Err(error) = fs2::FileExt::unlock(&self.lock) {
            eprintln!("toolconfig: cannot release writer lock: {error}");
        }
    }
}

/// Outputs listed in a managed `.gitignore` block. `.editorconfig` files must be committed.
fn ignored_outputs(plan: &Plan) -> Vec<PathBuf> {
    plan.outputs
        .iter()
        .filter(|output| !output.is_editorconfig())
        .map(|output| output.path.clone())
        .collect()
}

pub(crate) fn validate(plan: &Plan) -> Result<()> {
    if plan
        .root
        .join(STATE_DIR)
        .join("journal.json")
        .try_exists()?
    {
        bail!("an interrupted publication needs recovery; run generate or clean");
    }
    preflight(&plan.root, plan, &read_state(&plan.root)?)?;
    let path = plan.root.join(".gitignore");
    let snapshot = read_snapshot(&path)?;
    let bytes = snapshot.as_ref().map(Snapshot::bytes).transpose()?;
    if ignore::enabled(bytes.as_deref())? {
        if plan.outputs.iter().any(|output| output.path == path) {
            bail!(".gitignore cannot be both a generated output and a managed ignore file");
        }
        ignore::update(&plan.root, &ignored_outputs(plan), bytes.as_deref())?;
    }
    Ok(())
}

/// Every difference between the plan and the files on disk, as `(status, path)` pairs.
pub(crate) fn drift(plan: &Plan) -> Result<Vec<(&'static str, String)>> {
    let state = read_state(&plan.root)?;
    let mut problems = Vec::new();
    for output in &plan.outputs {
        let shown = paths::relative(&plan.root, &output.path);
        let key = record_key(&plan.root, &output.path)?;
        match read_snapshot(&output.path)? {
            None => problems.push(("missing", shown)),
            Some(actual) => match state.outputs.get(&key) {
                None => problems.push(("unowned", shown)),
                Some(record) if record.hash != actual.hash => problems.push(("modified", shown)),
                Some(_) if actual.hash != digest(&output.bytes) => problems.push(("stale", shown)),
                Some(_) => {}
            },
        }
    }
    let desired = plan
        .outputs
        .iter()
        .map(|output| output.path.clone())
        .collect::<BTreeSet<_>>();
    for path in owned_paths(&plan.root, &state)? {
        if !desired.contains(&path) && read_snapshot(&path)?.is_some() {
            problems.push(("obsolete", paths::relative(&plan.root, &path)));
        }
    }
    for path in &plan.unmanaged {
        if !state.outputs.contains_key(&record_key(&plan.root, path)?) {
            problems.push(("hand-written", paths::relative(&plan.root, path)));
        }
    }
    let path = plan.root.join(".gitignore");
    let bytes = read_snapshot(&path)?
        .as_ref()
        .map(Snapshot::bytes)
        .transpose()?;
    if ignore::enabled(bytes.as_deref())?
        && ignore::update(&plan.root, &ignored_outputs(plan), bytes.as_deref())?.as_slice()
            != bytes.as_deref().unwrap_or_default()
    {
        problems.push(("stale", ".gitignore".to_owned()));
    }
    Ok(problems)
}

fn preflight(root: &Path, plan: &Plan, state: &State) -> Result<()> {
    for path in owned_paths(root, state)? {
        if plan.inputs.contains(&path) {
            bail!(
                "owned output {} is now a ToolConfig input; preserving it",
                path.display()
            );
        }
        let key = record_key(root, &path)?;
        if let Some(actual) = read_snapshot(&path)?
            && state
                .outputs
                .get(&key)
                .is_some_and(|record| actual.hash != record.hash)
        {
            bail!(
                "{} was modified outside ToolConfig; edit its source instead, then move the file aside or restore the generated content",
                paths::relative(root, &path)
            );
        }
    }
    for path in &plan.unmanaged {
        if !state.outputs.contains_key(&record_key(root, path)?) {
            bail!(
                "{} is hand-written; ToolConfig generates .editorconfig from sources, so convert it into a .toolconfig.toml beside it (see {ADOPTING_GUIDE})",
                paths::relative(root, path)
            );
        }
    }
    for output in &plan.outputs {
        check_ancestors(&output.path)?;
        if plan.inputs.contains(&output.path) {
            bail!("output collides with an input: {}", output.path.display());
        }
        if read_snapshot(&output.path)?.is_some()
            && !state.outputs.contains_key(&record_key(root, &output.path)?)
        {
            let hint = if output.is_editorconfig() {
                format!(
                    "; move its contents into {} (see {ADOPTING_GUIDE})",
                    paths::relative(root, &output.source)
                )
            } else {
                format!(
                    "; move its options into a [\"*\".{}] table, then delete it (see {ADOPTING_GUIDE})",
                    output.label
                )
            };
            bail!(
                "refusing to overwrite unowned file {}{hint}",
                paths::relative(root, &output.path)
            );
        }
    }
    for alternative in &plan.alternatives {
        reject_nonregular(alternative)?;
        if alternative.try_exists()? && !state.outputs.contains_key(&record_key(root, alternative)?)
        {
            bail!(
                "existing native configuration {} would conflict with a generated file; move its options into the source, then delete it",
                paths::relative(root, alternative)
            );
        }
    }
    Ok(())
}

fn owned_paths(root: &Path, state: &State) -> Result<Vec<PathBuf>> {
    state
        .outputs
        .keys()
        .map(|key| from_record_key(root, key))
        .collect()
}

fn owned_snapshot(root: &Path, path: &Path, state: &State) -> Result<Option<Snapshot>> {
    let snapshot = read_snapshot(path)?;
    if let Some(actual) = &snapshot {
        match state.outputs.get(&record_key(root, path)?) {
            Some(record) if record.hash == actual.hash => {}
            Some(_) => bail!(
                "{} was modified outside ToolConfig",
                paths::relative(root, path)
            ),
            None => bail!(
                "refusing to overwrite unowned file {}",
                paths::relative(root, path)
            ),
        }
    }
    Ok(snapshot)
}

pub(crate) fn status(root: &Path, path: &Path) -> Result<&'static str> {
    let state = read_state(root)?;
    match read_snapshot(path)? {
        None => Ok("missing"),
        Some(actual) => match state.outputs.get(&record_key(root, path)?) {
            None => Ok("unowned"),
            Some(record) if record.hash == actual.hash => Ok("managed"),
            Some(_) => Ok("modified"),
        },
    }
}

fn record_key(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .with_context(|| format!("{} is outside the project", path.display()))?;
    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            bail!("invalid output path {}", path.display());
        };
        parts.push(
            part.to_str()
                .with_context(|| format!("output paths must be UTF-8: {}", path.display()))?,
        );
    }
    if parts.is_empty() {
        bail!("the project root cannot be an output");
    }
    Ok(parts.join("/"))
}

fn from_record_key(root: &Path, key: &str) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for part in key.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains('\\') {
            bail!("invalid path in ownership state: {key:?}");
        }
        path.push(part);
    }
    if path.starts_with(root.join(STATE_DIR)) {
        bail!("invalid path in ownership state: {key:?}");
    }
    Ok(path)
}

fn read_state(root: &Path) -> Result<State> {
    let path = root.join(STATE_DIR).join("state.json");
    check_ancestors(&path)?;
    reject_nonregular(&path)?;
    if !path.try_exists()? {
        return Ok(State::empty());
    }
    let state: State = serde_json::from_slice(&std::fs::read(&path)?)
        .context("cannot decode ownership state; preserving all outputs")?;
    if state.version != STATE_VERSION {
        bail!("ownership state uses an unsupported version; preserving all outputs");
    }
    for key in state.outputs.keys() {
        from_record_key(root, key)?;
    }
    Ok(state)
}

fn read_snapshot(path: &Path) -> Result<Option<Snapshot>> {
    check_ancestors(path)?;
    reject_nonregular(path)?;
    match File::open(path) {
        Ok(file) => Ok(Some(snapshot_file(&file)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn snapshot_file(file: &File) -> Result<Snapshot> {
    use std::io::{Read, Seek};
    let mut file = file.try_clone()?;
    file.rewind()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(Snapshot {
        hash: digest(&bytes),
        content: base64::engine::general_purpose::STANDARD.encode(&bytes),
        identity: Identity::of(&file)?,
        mode: file_mode(&file)?,
    })
}

fn same_content(left: Option<&Snapshot>, right: Option<&Snapshot>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left.hash == right.hash,
        _ => false,
    }
}

fn verify_current(path: &Path, expected: Option<&Snapshot>) -> Result<()> {
    let current = read_snapshot(path)?;
    if !same_content(current.as_ref(), expected)
        || current
            .as_ref()
            .zip(expected)
            .is_some_and(|(current, expected)| current.identity != expected.identity)
    {
        bail!(
            "{} changed while preparing publication; retry after resolving concurrent edits",
            path.display()
        );
    }
    Ok(())
}

fn stage_file(path: &Path, bytes: &[u8], mode: Option<u32>) -> Result<NamedTempFile> {
    check_ancestors(path)?;
    let parent = path.parent().context("output path has no parent")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("cannot create {}", parent.display()))?;
    let mut stage = tempfile::Builder::new()
        .prefix(STAGE_PREFIX)
        .tempfile_in(parent)?;
    stage.write_all(bytes)?;
    set_file_mode(stage.as_file(), mode.unwrap_or(0o644))?;
    stage.as_file().sync_all()?;
    Ok(stage)
}

fn write_file(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    reject_nonregular(path)?;
    let stage = stage_file(path, bytes, Some(mode))?;
    stage
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("cannot update {}", path.display()))?;
    sync_parent(path)?;
    Ok(())
}

fn write_json(path: &Path, value: &impl Serialize, mode: u32) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_file(path, &bytes, mode)
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> Result<()> {
    File::open(path.parent().context("file has no parent directory")?)?.sync_all()?;
    Ok(())
}

// Windows replacement is performed by tempfile's platform implementation.
// File contents are flushed before replacement; directory fsync is not available.
#[cfg(windows)]
fn sync_parent(_path: &Path) -> Result<()> {
    Ok(())
}

fn check_ancestors(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if identity::is_link(&metadata) => {
                bail!("refusing symlink or reparse point {}", ancestor.display())
            }
            Ok(metadata) if ancestor != path && !metadata.is_dir() => {
                bail!("parent is not a directory: {}", ancestor.display())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("cannot inspect {}", ancestor.display()));
            }
        }
    }
    Ok(())
}

fn reject_nonregular(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || identity::is_link(&metadata) => {
            bail!("refusing nonregular file {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("cannot inspect {}", path.display())),
    }
}

#[cfg(unix)]
fn file_mode(file: &File) -> Result<u32> {
    use std::os::unix::fs::PermissionsExt;
    Ok(file.metadata()?.permissions().mode() & 0o777)
}
#[cfg(windows)]
fn file_mode(_file: &File) -> Result<u32> {
    Ok(0o644)
}

#[cfg(unix)]
fn set_file_mode(file: &File, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    Ok(file.set_permissions(std::fs::Permissions::from_mode(mode))?)
}
#[cfg(windows)]
#[expect(
    clippy::permissions_set_readonly_false,
    reason = "On Windows this clears a DOS attribute without changing filesystem ACLs"
)]
fn set_file_mode(file: &File, _mode: u32) -> Result<()> {
    let mut permissions = file.metadata()?.permissions();
    permissions.set_readonly(false);
    Ok(file.set_permissions(permissions)?)
}

fn report_tracked(root: &Path, paths: &[PathBuf]) {
    let result = std::process::Command::new("git")
        .current_dir(root)
        .args(["ls-files", "-z", "--full-name"])
        .output();
    let Ok(output) = result else { return };
    if !output.status.success() {
        return;
    }
    // Ask Git for its prefix so a nested project is compared in repository coordinates.
    let prefix = std::process::Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--show-prefix"])
        .output();
    let prefix = prefix
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .trim_end()
                .to_owned()
        })
        .unwrap_or_default();
    let tracked = output
        .stdout
        .split(|byte| *byte == 0)
        .collect::<BTreeSet<_>>();
    for path in paths {
        if let Ok(relative) = path.strip_prefix(root) {
            let relative = relative.to_string_lossy().replace('\\', "/");
            if tracked.contains(format!("{prefix}{relative}").as_bytes()) {
                eprintln!(
                    "toolconfig: {} is already tracked; .gitignore does not change the index",
                    path.display()
                );
            }
        }
    }
}
