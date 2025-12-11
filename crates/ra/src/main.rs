//! Command-line interface for the `ra` research assistant tool.

mod cli;

use std::process::ExitCode;

use cli::{CommandContext, args::parse_cli, commands};

fn main() -> ExitCode {
    let cli = parse_cli();

    match cli.command {
        cli::args::Commands::Init(cmd) => {
            let ctx = match CommandContext::load_cwd_only() {
                Ok(ctx) => ctx,
                Err(code) => return code,
            };
            cli::commands::init::run(&ctx, &cmd)
        }
        cli::args::Commands::Inspect { what } => match what {
            cli::args::InspectWhat::Doc { .. } => {
                let ctx = match CommandContext::load_cwd_only() {
                    Ok(ctx) => ctx,
                    Err(code) => return code,
                };
                cli::commands::inspect::run(&ctx, what)
            }
            _ => {
                let mut ctx = match CommandContext::load() {
                    Ok(ctx) => ctx,
                    Err(code) => return code,
                };
                commands::run(cli::args::Commands::Inspect { what }, &mut ctx)
            }
        },
        cli::args::Commands::Agents(cmd) => cli::commands::agents::run(&cmd),
        other => {
            let mut ctx = match CommandContext::load() {
                Ok(ctx) => ctx,
                Err(code) => return code,
            };
            commands::run(other, &mut ctx)
        }
    }
}
