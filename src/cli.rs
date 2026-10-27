//! The command workflows and their shared compilation path.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};

use crate::compiler::{self, Plan};
use crate::model::ADOPTING_GUIDE;
use crate::project::{self, GENERATED_HEADER};
use crate::{paths, pkl_source, publication, report, schema, watch};

#[derive(Debug, Parser)]
#[command(
    name = "toolconfig",
    version,
    about = "EditorConfig, extended to your tools",
    disable_help_subcommand = true
)]
struct Arguments {
    /// Use DIR as the project root instead of searching for a source with root = true.
    #[arg(long, global = true, value_name = "DIR", env = "TOOLCONFIG_ROOT")]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a root source in the current directory, preserving an existing one.
    Init {
        /// The source syntax to create.
        #[arg(long, value_enum, default_value_t = Format::Toml)]
        format: Format,
        /// Add a managed .gitignore block for generated files other than .editorconfig.
        #[arg(long)]
        gitignore: bool,
    },
    /// Write .editorconfig files and native tool configuration.
    Generate {
        /// Regenerate whenever a source or its dependencies change.
        #[arg(long)]
        watch: bool,
        /// Treat warnings as errors.
        #[arg(long)]
        strict: bool,
    },
    /// Fail when a generated file is missing, modified or out of date.
    Check {
        /// Treat warnings as errors.
        #[arg(long)]
        strict: bool,
    },
    /// Compile and check ownership without writing anything.
    Validate {
        /// Treat warnings as errors.
        #[arg(long)]
        strict: bool,
    },
    /// List generated files with their format, status and source.
    List,
    /// Show every generated key and where its value came from.
    Explain {
        /// Limit the output to one tool, such as rustfmt.
        tool: Option<String>,
        /// Limit the output to files generated in DIR.
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
    },
    /// Remove generated files that are unchanged since ToolConfig wrote them.
    Clean,
    /// Print the JSON Schema for TOML sources.
    #[command(hide = true)]
    Schema,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Format {
    Toml,
    Pkl,
}

pub(crate) fn run() -> Result<()> {
    let arguments = Arguments::parse();
    let requested = arguments.root.as_deref();
    match arguments.command {
        Command::Init { format, gitignore } => init(requested, format, gitignore),
        Command::Generate { watch: true, .. } => watch::run(&project::find_root(requested)?),
        Command::Generate {
            watch: false,
            strict,
        } => {
            let root = project::find_root(requested)?;
            let project = publication::Project::acquire(&root)?;
            let plan = build(&root)?;
            report::warnings(&plan, strict)?;
            project.generate(&plan)
        }
        Command::Check { strict } => {
            let plan = build(&project::find_root(requested)?)?;
            report::warnings(&plan, strict)?;
            let problems = publication::drift(&plan)?;
            if problems.is_empty() {
                println!("Up to date: {} generated files", plan.outputs.len());
                return Ok(());
            }
            for (status, path) in &problems {
                println!("{status}\t{path}");
            }
            bail!(
                "{} generated file{} out of date; run toolconfig generate",
                problems.len(),
                if problems.len() == 1 { " is" } else { "s are" }
            )
        }
        Command::Validate { strict } => {
            let plan = build(&project::find_root(requested)?)?;
            report::warnings(&plan, strict)?;
            publication::validate(&plan)?;
            println!("Valid: {} generated files", plan.outputs.len());
            Ok(())
        }
        Command::List => {
            let root = project::find_root(requested)?;
            let plan = build(&root)?;
            println!("STATUS\tFORMAT\tOUTPUT\tSOURCE");
            for output in &plan.outputs {
                println!(
                    "{}\t{}\t{}\t{}",
                    publication::status(&root, &output.path)?,
                    output.format,
                    paths::relative(&root, &output.path),
                    paths::relative(&root, &output.source)
                );
            }
            Ok(())
        }
        Command::Explain { tool, dir } => explain(requested, tool.as_deref(), dir.as_deref()),
        Command::Clean => {
            let root = match project::find_root(requested) {
                Ok(root) => root,
                Err(error) => state_root().ok_or(error)?,
            };
            publication::Project::acquire(&root)?.clean()
        }
        Command::Schema => {
            println!("{}", serde_json::to_string_pretty(&schema::generate())?);
            Ok(())
        }
    }
}

fn build(root: &Path) -> Result<Plan> {
    compiler::build(root).0
}

/// For `clean` after the sources are gone: the nearest directory with ownership records.
fn state_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?.canonicalize().ok()?;
    cwd.ancestors()
        .find(|ancestor| ancestor.join(paths::STATE_DIR).join("state.json").is_file())
        .map(Path::to_path_buf)
}

fn init(requested: Option<&Path>, format: Format, gitignore: bool) -> Result<()> {
    let cwd = std::env::current_dir()
        .context("cannot read the current directory")?
        .canonicalize()?;
    let dir = match requested {
        Some(requested) => cwd.join(requested),
        None => cwd,
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let dir = dir.canonicalize()?;
    let existing = project::sources_in(&dir)?;
    let (path, created) = match existing.as_slice() {
        [path] => (path.clone(), false),
        [] => {
            let editorconfig = dir.join(".editorconfig");
            if let Ok(bytes) = std::fs::read(&editorconfig)
                && !bytes.starts_with(GENERATED_HEADER.as_bytes())
            {
                bail!(
                    "{} is hand-written; convert it into a .toolconfig.toml first (see {ADOPTING_GUIDE})",
                    editorconfig.display()
                );
            }
            let (name, text) = match format {
                Format::Toml => (".toolconfig.toml", starter_toml()),
                Format::Pkl => (".toolconfig.pkl", starter_pkl()),
            };
            let path = dir.join(name);
            let mut file = tempfile::Builder::new()
                .prefix(compiler::STAGE_PREFIX)
                .tempfile_in(&dir)?;
            use std::io::Write;
            file.write_all(text.as_bytes())?;
            file.as_file().sync_all()?;
            file.persist_noclobber(&path)
                .map_err(|error| error.error)
                .with_context(|| format!("cannot create {}", path.display()))?;
            (path, true)
        }
        _ => bail!(
            "{} has more than one ToolConfig source; keep exactly one",
            dir.display()
        ),
    };
    if gitignore {
        let plan = build(&dir)?;
        publication::Project::acquire(&dir)?.establish_ignore(&plan)?;
    }
    println!(
        "{} {}",
        if created { "Created" } else { "Using" },
        path.display()
    );
    Ok(())
}

fn starter_toml() -> String {
    format!(
        r#"#:schema {}
root = true

[toolconfig]
# Tools whose configuration is generated in this directory, such as "rustfmt" or "prettier".
tools = []

["*"]
charset = "utf-8"
end_of_line = "lf"
insert_final_newline = true
trim_trailing_whitespace = true
"#,
        schema::url()
    )
}

fn starter_pkl() -> String {
    format!(
        r#"amends "{}#/ToolConfig.pkl"

root = true

toolconfig {{
  // Tools whose configuration is generated in this directory, such as "rustfmt" or "prettier".
  tools = List()
}}

sections {{
  ["*"] {{
    charset = "utf-8"
    end_of_line = "lf"
    insert_final_newline = true
    trim_trailing_whitespace = true
  }}
}}
"#,
        pkl_source::package_uri()
    )
}

fn explain(requested: Option<&Path>, tool: Option<&str>, dir: Option<&Path>) -> Result<()> {
    let root = project::find_root(requested)?;
    let plan = build(&root)?;
    let dir = dir
        .map(|dir| -> Result<PathBuf> {
            let cwd = std::env::current_dir()?;
            Ok(cwd.join(dir).canonicalize()?)
        })
        .transpose()?;
    let mut shown = 0;
    for output in &plan.outputs {
        if output.is_editorconfig() || output.label.starts_with("files.") {
            continue;
        }
        if tool.is_some_and(|tool| tool != output.label)
            || dir
                .as_deref()
                .is_some_and(|dir| output.path.parent() != Some(dir))
        {
            continue;
        }
        if shown > 0 {
            println!();
        }
        shown += 1;
        println!(
            "{} -> {} (from {})",
            output.label,
            paths::relative(&root, &output.path),
            paths::relative(&root, &output.source)
        );
        let width = output
            .explanation
            .iter()
            .map(|entry| entry.key.len())
            .max()
            .unwrap_or(0);
        let value_width = output
            .explanation
            .iter()
            .map(|entry| entry.value.len().min(24))
            .max()
            .unwrap_or(0);
        for entry in &output.explanation {
            let scope = entry
                .files
                .as_ref()
                .map(|files| format!(" (override for {files})"))
                .unwrap_or_default();
            println!(
                "  {:width$}  {:value_width$}  {}{scope}",
                entry.key, entry.value, entry.from
            );
        }
        if output.explanation.is_empty() {
            println!("  (no keys)");
        }
    }
    if shown == 0 {
        match tool {
            Some(tool) => bail!("no generated configuration for {tool}"),
            None => println!("No tool configuration is generated."),
        }
    }
    Ok(())
}
