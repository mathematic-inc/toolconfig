//! Lexical path handling shared by discovery, compilation and publication.

use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};

/// Ownership records, committed beside the root source. Not to be confused with the writer lock,
/// which lives in a per-machine directory with the publication journal.
pub(crate) const LOCKFILE: &str = ".toolconfig.lock";
/// Where releases before 2026.11.4 kept ownership records, the writer lock and the journal.
pub(crate) const LEGACY_STATE_DIR: &str = ".toolconfig-state";

pub(crate) fn is_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(unix)]
    {
        metadata.file_type().is_symlink()
    }
}

/// Joins `path` onto `base` and removes `.` and `..` without touching the filesystem.
pub(crate) fn absolute(base: &Path, path: &Path) -> Result<PathBuf> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut result = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    bail!("path escapes its filesystem root: {}", joined.display());
                }
            }
            part => result.push(part.as_os_str()),
        }
    }
    if !result.is_absolute() {
        bail!("path is not absolute: {}", result.display());
    }
    Ok(result)
}

/// A comparison key that treats case-insensitive filesystems as such.
pub(crate) fn key(path: &Path) -> String {
    let key = path.to_string_lossy();
    if cfg!(any(windows, target_os = "macos")) {
        key.to_lowercase()
    } else {
        key.into_owned()
    }
}

/// `path` relative to `root`, with `/` separators, for messages and ownership records.
pub(crate) fn relative(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let text = relative.to_string_lossy().replace('\\', "/");
    if text.is_empty() {
        ".".to_owned()
    } else {
        text
    }
}
