//! Warnings shared by every command that compiles.

use anyhow::{Result, bail};

use crate::compiler::Plan;

/// Prints the plan's warnings; with `strict`, any warning is an error.
pub(crate) fn warnings(plan: &Plan, strict: bool) -> Result<()> {
    for warning in &plan.warnings {
        eprintln!("toolconfig: warning: {warning}");
    }
    if strict && !plan.warnings.is_empty() {
        bail!(
            "{} warning{} with --strict",
            plan.warnings.len(),
            if plan.warnings.len() == 1 { "" } else { "s" }
        );
    }
    Ok(())
}
