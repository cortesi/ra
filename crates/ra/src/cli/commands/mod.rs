//! Command implementations and dispatch.

pub mod agents;
pub mod config;
pub mod context;
pub mod get;
pub mod init;
pub mod inspect;
pub mod likethis;
pub mod ls;
pub mod search;
mod shared;
pub mod status;
pub mod update;

use std::process::ExitCode;

use super::{args::Commands, context::CommandContext};

/// Dispatches to the selected subcommand.
pub fn run(command: Commands, ctx: &mut CommandContext) -> ExitCode {
    match command {
        Commands::Search(cmd) => search::run(ctx, &cmd),
        Commands::Context(cmd) => context::run(ctx, &cmd),
        Commands::Get(cmd) => get::run(ctx, &cmd),
        Commands::LikeThis(cmd) => likethis::run(ctx, &cmd),
        Commands::Inspect { what } => inspect::run(ctx, what),
        Commands::Init(cmd) => init::run(ctx, &cmd),
        Commands::Update => update::run(ctx),
        Commands::Status => status::run(ctx),
        Commands::Config => config::run(ctx),
        Commands::Ls(cmd) => ls::run(ctx, &cmd),
        Commands::Agents(cmd) => agents::run(&cmd),
    }
}
