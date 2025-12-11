//! Implementation of `ra agents`.

use std::process::ExitCode;

use crate::cli::args::AgentsCommand;

/// Generates agent instruction files.
pub fn run(cmd: &AgentsCommand) -> ExitCode {
    println!(
        "agents (stdout={}, claude={}, gemini={}, all={})",
        cmd.stdout, cmd.claude, cmd.gemini, cmd.all
    );
    ExitCode::SUCCESS
}
