//! EditorConfig, extended to your tools: generate `.editorconfig` and native tool configuration
//! from TOML or Pkl sources.

mod cascade;
mod catalog;
mod cli;
mod compiler;
mod ignore;
mod inputs;
mod model;
mod paths;
mod pkl_source;
mod project;
mod projection;
mod publication;
mod render;
mod report;
mod schema;
mod toml_source;
mod value;
mod watch;

fn main() -> std::process::ExitCode {
    match cli::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("toolconfig: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
