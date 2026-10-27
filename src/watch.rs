//! Watching sources and their dependencies while one writer owns the project.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::compiler::{self, Plan, STAGE_PREFIX};
use crate::inputs::Inputs;
use crate::model::SOURCE_NAMES;
use crate::{publication, report};

/// Coalesces atomic-save rename/write bursts without delaying interactive updates.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(150);
// Some filesystems miss notifications; fingerprint polling bounds detection latency.
const INPUT_POLL_INTERVAL: Duration = Duration::from_secs(1);

enum Event {
    Files(notify::Result<notify::Event>),
    Compiled(Box<Compilation>),
    Stop,
}

struct Compilation {
    inputs: Inputs,
    result: Result<Plan>,
}

pub(crate) fn run(root: &Path) -> Result<()> {
    let project = publication::Project::acquire(root)?;
    let (sender, receiver) = mpsc::channel();
    let stop_sender = sender.clone();
    ctrlc::set_handler(move || {
        let _ = stop_sender.send(Event::Stop);
    })
    .context("cannot install the shutdown handler")?;
    let watch_sender = sender.clone();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |event| {
        let _ = watch_sender.send(Event::Files(event));
    })
    .context("cannot start the filesystem watcher")?;
    let mut inputs = Inputs::default();
    let mut watched = BTreeMap::new();
    update_watches(&mut watcher, &mut watched, root, &inputs)?;
    let mut outputs = Vec::new();
    let mut compiling = false;
    let mut deadline = Some(Instant::now());
    eprintln!("toolconfig: watching {}", root.display());
    loop {
        if !compiling && deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            start_compilation(root.to_path_buf(), sender.clone())?;
            compiling = true;
            deadline = None;
        }
        let event = match deadline.filter(|_| !compiling) {
            Some(deadline) => {
                match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(event) => event,
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            None => match receiver.recv_timeout(INPUT_POLL_INTERVAL) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => {
                    if inputs.changed() {
                        deadline = Some(Instant::now() + SAVE_DEBOUNCE);
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => anyhow::bail!("filesystem watcher stopped"),
            },
        };
        match event {
            Event::Stop => break,
            Event::Files(Err(error)) => eprintln!("toolconfig: watcher error: {error}"),
            Event::Files(Ok(event)) => {
                if relevant(&event, root, &inputs, &outputs) {
                    deadline = Some(Instant::now() + SAVE_DEBOUNCE);
                }
            }
            Event::Compiled(compilation) => {
                compiling = false;
                inputs = compilation.inputs;
                // Attach newly discovered dependencies before announcing publication.
                // A fingerprint check closes the evaluation-to-watch registration gap.
                update_watches(&mut watcher, &mut watched, root, &inputs)?;
                if deadline.is_some() || inputs.changed() {
                    deadline.get_or_insert(Instant::now() + SAVE_DEBOUNCE);
                    continue;
                }
                let result = compilation.result.and_then(|plan| {
                    outputs = plan
                        .outputs
                        .iter()
                        .map(|output| output.path.clone())
                        .collect();
                    report::warnings(&plan, false)?;
                    project.generate(&plan)
                });
                if let Err(error) = result {
                    if inputs.changed() {
                        deadline = Some(Instant::now() + SAVE_DEBOUNCE);
                    } else {
                        eprintln!(
                            "toolconfig: {error:#}\ntoolconfig: keeping the last successful outputs; waiting for a change"
                        );
                    }
                }
            }
        }
    }
    eprintln!("toolconfig: stopped watching; generated files remain in place");
    Ok(())
}

fn start_compilation(root: PathBuf, sender: mpsc::Sender<Event>) -> Result<()> {
    // Only one compiler runs at a time. Filesystem and stop events stay responsive;
    // a pending save supersedes this result and starts another compilation afterward.
    std::thread::Builder::new()
        .name("toolconfig-compiler".into())
        .spawn(move || {
            let (result, inputs) = compiler::build(&root);
            let inputs = match &result {
                Ok(plan) => plan.inputs.clone(),
                Err(_) => inputs,
            };
            let _ = sender.send(Event::Compiled(Box::new(Compilation { inputs, result })));
        })
        .context("cannot start the compiler")?;
    Ok(())
}

fn update_watches(
    watcher: &mut RecommendedWatcher,
    watched: &mut BTreeMap<PathBuf, RecursiveMode>,
    root: &Path,
    inputs: &Inputs,
) -> Result<()> {
    // New sources can appear anywhere in the project.
    let mut desired = BTreeMap::from([(root.to_path_buf(), RecursiveMode::Recursive)]);
    for path in inputs.paths() {
        if path.starts_with(root) {
            continue;
        }
        let mut directory = path.parent().unwrap_or(root);
        while !directory.is_dir() {
            let Some(parent) = directory.parent() else {
                break;
            };
            directory = parent;
        }
        desired
            .entry(directory.to_path_buf())
            .or_insert(RecursiveMode::NonRecursive);
    }
    for base in inputs.directories() {
        if base.starts_with(root) {
            continue;
        }
        let directory = base.ancestors().find(|path| path.is_dir()).unwrap_or(root);
        desired.insert(directory.to_path_buf(), RecursiveMode::Recursive);
    }
    let recursive = desired
        .iter()
        .filter(|(_, mode)| **mode == RecursiveMode::Recursive)
        .map(|(path, _)| path.clone())
        .collect::<Vec<_>>();
    desired.retain(|path, _| {
        !recursive
            .iter()
            .any(|parent| path != parent && path.starts_with(parent))
    });
    for (directory, mode) in &desired {
        if watched.get(directory) == Some(mode) {
            continue;
        }
        if watched.contains_key(directory) {
            watcher.unwatch(directory)?;
        }
        watcher
            .watch(directory, *mode)
            .with_context(|| format!("cannot watch {}", directory.display()))?;
    }
    for directory in watched
        .keys()
        .filter(|directory| !desired.contains_key(*directory))
    {
        watcher
            .unwatch(directory)
            .with_context(|| format!("cannot stop watching {}", directory.display()))?;
    }
    *watched = desired;
    Ok(())
}

fn relevant(event: &notify::Event, root: &Path, inputs: &Inputs, outputs: &[PathBuf]) -> bool {
    if matches!(event.kind, notify::EventKind::Access(_)) {
        return false;
    }
    event.paths.iter().any(|path| {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if inputs.ignores(path)
            || path.starts_with(root.join(crate::paths::STATE_DIR))
            || path.starts_with(root.join(".git"))
            || outputs.contains(path)
            || name.starts_with(STAGE_PREFIX)
        {
            return false;
        }
        SOURCE_NAMES.contains(&name.as_str())
            || name == ".editorconfig"
            || name == "package.json"
            || inputs
                .paths()
                .any(|input| input == path || input.starts_with(path))
            || inputs
                .directories()
                .any(|directory| path.starts_with(directory))
    })
}
