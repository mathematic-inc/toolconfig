use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::model::{Origin, syntax_of};
use crate::value::Value;

#[derive(Clone, Debug)]
pub(crate) struct Repository {
    git: String,
    rev: String,
    path: Option<String>,
}

impl Repository {
    pub(crate) fn parse(value: &Value, origin: &Origin) -> Result<Vec<Self>> {
        let Value::Array(entries) = value else {
            bail!("{origin}: extends must be an array of repositories");
        };
        entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                Self::entry(entry).with_context(|| format!("{origin}: extends entry {}", index + 1))
            })
            .collect()
    }

    fn entry(value: &Value) -> Result<Self> {
        let fields = value
            .as_object()
            .context("a repository must be a table with git, rev and optional path")?;
        for (key, _) in fields {
            if !matches!(key.as_str(), "git" | "rev" | "path") {
                bail!("unknown repository key; expected git, rev or path");
            }
        }
        let git = value
            .get("git")
            .and_then(Value::as_str)
            .context("git must be a repository URL")?;
        validate_url(git)?;
        let rev = value
            .get("rev")
            .and_then(Value::as_str)
            .context("rev must be a full commit ID")?;
        if !matches!(rev.len(), 40 | 64) || !rev.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!(
                "rev must be a full 40- or 64-character commit ID; branches and tags are not supported"
            );
        }
        let path = match value.get("path") {
            None | Some(Value::Null) => None,
            Some(Value::String(path)) => {
                relative_path(path)?;
                if syntax_of(Path::new(path)).is_none() {
                    bail!("path must name a recognized ToolConfig source");
                }
                Some(path.clone())
            }
            _ => bail!("path must be a repository-relative source filename"),
        };
        Ok(Self {
            git: git.to_owned(),
            rev: rev.to_ascii_lowercase(),
            path,
        })
    }

    pub(crate) fn git(&self) -> &str {
        &self.git
    }
    pub(crate) fn rev(&self) -> &str {
        &self.rev
    }
    pub(crate) fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
    pub(crate) fn name(&self) -> String {
        format!("{}@{}", self.git, self.rev)
    }
}

fn validate_url(url: &str) -> Result<()> {
    // Do not echo invalid URLs: userinfo and query strings can contain credentials.
    if url.is_empty()
        || url
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        || url.contains(['\\', '?', '#'])
    {
        bail!("git must be an HTTPS or SSH repository URL without embedded credentials");
    }
    let valid = if let Some(rest) = url.strip_prefix("https://") {
        rest.split_once('/')
            .is_some_and(|(host, path)| !host.is_empty() && !host.contains('@') && !path.is_empty())
    } else if let Some(rest) = url.strip_prefix("ssh://") {
        rest.split_once('/').is_some_and(|(authority, path)| {
            let (user, host) = authority.rsplit_once('@').unwrap_or(("", authority));
            !host.is_empty() && !user.contains([':', '@']) && !path.is_empty()
        })
    } else {
        url.split_once(':').is_some_and(|(authority, path)| {
            !authority.is_empty()
                && !authority.starts_with('-')
                && !authority.contains('/')
                && authority.contains('@')
                && !path.is_empty()
                && !path.starts_with('-')
                && authority.split_once('@').is_some_and(|(user, host)| {
                    !user.is_empty() && !host.is_empty() && !host.contains('@')
                })
        })
    };
    if !valid {
        bail!("git must be an HTTPS or SSH repository URL without embedded credentials");
    }
    Ok(())
}

pub(super) fn relative_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.contains(['\\', ':', '\0'])
        || path.chars().any(char::is_control)
        || path.split('/').any(|part| {
            part.is_empty()
                || matches!(part, "." | "..")
                || part.eq_ignore_ascii_case(".git")
                || part.ends_with(['.', ' '])
                || reserved_name(part)
        })
    {
        bail!(
            "repository paths must be relative, portable paths without traversal or .git components"
        );
    }
    Ok(())
}

fn reserved_name(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
}
