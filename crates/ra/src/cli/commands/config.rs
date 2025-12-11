//! Implementation of `ra config`.

use std::process::ExitCode;

use ra_highlight::Highlighter;

use crate::cli::context::CommandContext;

/// Shows effective configuration settings.
pub fn run(ctx: &CommandContext) -> ExitCode {
    let config = &ctx.config;
    let highlighter = Highlighter::new();
    print!("{}", highlighter.highlight_toml(&config.settings_to_toml()));
    ExitCode::SUCCESS
}
