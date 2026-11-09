use std::ffi::OsStr;
use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

// Bound unattended credential helpers and network operations, including in watch mode.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
// Keep short commands responsive without busy-waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

pub(super) fn read(directory: &Path, args: &[&OsStr]) -> Result<Vec<u8>> {
    let output = tempfile::tempfile().context("cannot stage Git output")?;
    run(directory, args, output.try_clone()?)?;
    use std::io::Read;
    let mut bytes = Vec::new();
    let mut output = output;
    use std::io::Seek;
    output.rewind()?;
    output.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub(super) fn run(directory: &Path, args: &[&OsStr], output: File) -> Result<()> {
    let mut command = Command::new("git");
    command
        .current_dir(directory)
        .args([
            "-c",
            "core.hooksPath=disabled-hooks",
            "-c",
            "gc.auto=0",
            "-c",
            "maintenance.auto=false",
            "-c",
            "protocol.ext.allow=never",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null())
        .stdout(output)
        .stderr(Stdio::null());
    // Keep credential helpers and SSH configuration; discard repository-local process state.
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_CURL_VERBOSE",
    ] {
        command.env_remove(name);
    }
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_TRACE") {
            command.env_remove(name);
        }
    }
    let mut child = command
        .spawn()
        .context("cannot start Git; install Git and make it available on PATH")?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => bail!(
                "Git failed; check repository access, the commit ID, and your Git credential helper or SSH configuration"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Ok(None) if start.elapsed() < COMMAND_TIMEOUT => std::thread::sleep(POLL_INTERVAL),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                result.context("cannot wait for Git")?;
                bail!(
                    "Git timed out; configure noninteractive credentials or SSH authentication and retry"
                );
            }
        }
    }
}
